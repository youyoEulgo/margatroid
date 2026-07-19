use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::async_runtime::{AsyncSystemOptions, AsyncTaskFailed, AsyncTaskStarted, AsyncWorker};
use crate::events::{Event, EventReader, Events};
use crate::plugin::PluginGroup;
use crate::schedule::{Schedule, ScheduleReport};
use crate::system::{System, SystemFailed};
use crate::world::World;

/// 执行阶段，按顺序推进。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stage {
    Startup,  // workspace 初始化时跑一次
    Input,    // 用户输入 → 路由
    Prepare,  // context assembly
    Execute,  // LLM 调用 + 工具执行
    Finalize, // 结果处理 + 记忆写入
    Event,    // SSE 事件推送
}

struct AppControlInner {
    shutdown: AtomicBool,
    pending_wake: Mutex<bool>,
    wake: Condvar,
}

/// 可跨线程唤醒或停止 App 主循环的控制句柄。
#[derive(Clone)]
pub struct AppControl {
    inner: Arc<AppControlInner>,
}

impl AppControl {
    /// 请求主循环尽快执行下一帧。
    pub fn wake(&self) {
        let mut pending = self
            .inner
            .pending_wake
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *pending = true;
        self.inner.wake.notify_one();
    }

    /// 请求 App 停止运行。
    pub fn shutdown(&self) {
        self.inner.shutdown.store(true, Ordering::Release);
        self.wake();
    }

    pub fn is_shutdown(&self) -> bool {
        self.inner.shutdown.load(Ordering::Acquire)
    }

    fn wait(&self) {
        let mut pending = self
            .inner
            .pending_wake
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*pending && !self.is_shutdown() {
            pending = self
                .inner
                .wake
                .wait(pending)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *pending = false;
    }
}

/// 顶层应用。持有 World 和按 stage 分组的 Schedule。
pub struct App {
    world: World,
    schedules: HashMap<Stage, Schedule>,
    event_maintenance: Schedule,
    event_types: HashSet<TypeId>,
    async_completion: Schedule,
    async_dispatch: Schedule,
    async_worker: Option<AsyncWorker>,
    control: AppControl,
    started: bool,
    async_queue_capacity: usize,
    async_max_in_flight: usize,
    event_retention_frames: u64,
}

impl App {
    pub fn new() -> Self {
        let mut schedules = HashMap::new();
        // 预注册所有阶段
        for stage in &[
            Stage::Startup,
            Stage::Input,
            Stage::Prepare,
            Stage::Execute,
            Stage::Finalize,
            Stage::Event,
        ] {
            schedules.insert(*stage, Schedule::new());
        }
        let control = AppControl {
            inner: Arc::new(AppControlInner {
                shutdown: AtomicBool::new(false),
                pending_wake: Mutex::new(false),
                wake: Condvar::new(),
            }),
        };
        App {
            world: World::new(),
            schedules,
            event_maintenance: Schedule::new(),
            event_types: HashSet::new(),
            async_completion: Schedule::new(),
            async_dispatch: Schedule::new(),
            async_worker: None,
            control,
            started: false,
            async_queue_capacity: 1024,
            async_max_in_flight: 256,
            event_retention_frames: 2,
        }
    }

    pub fn add_plugins(&mut self, plugins: impl Into<PluginGroup>) -> &mut Self {
        plugins.into().build_all(self);
        self
    }

    pub fn add_systems(
        &mut self,
        stage: Stage,
        systems: impl IntoIterator<Item = impl System>,
    ) -> &mut Self {
        let schedule = self
            .schedules
            .get_mut(&stage)
            .expect("stage not registered");
        for system in systems {
            schedule.add_system(system);
        }
        self
    }

    /// 注册一种事件类型。重复注册不会重置已有队列。
    pub fn add_event<E: Event>(&mut self) -> &mut Self {
        if self.world.resource::<Events<E>>().is_none() {
            self.world
                .add_resource(Events::<E>::new(self.event_retention_frames));
        }
        if self.event_types.insert(TypeId::of::<E>()) {
            self.event_maintenance.add_system(|world: &mut World| {
                if let Some(events) = world.resource::<Events<E>>() {
                    events.finish_frame();
                }
            });
        }
        self
    }

    /// 为已注册的事件类型创建独立 reader。
    pub fn event_reader<E: Event>(&self) -> EventReader<E> {
        self.world
            .resource::<Events<E>>()
            .unwrap_or_else(|| panic!("event `{}` is not registered", std::any::type_name::<E>()))
            .reader()
    }

    /// 注册同步派发、异步执行、同步回收结果的 system。
    ///
    /// Request 事件应在 Input 到 Finalize 阶段产生。每个请求会在专用 Tokio
    /// 线程中独立 spawn，Output 会在后续帧作为事件回到主线程。Future 不应
    /// 执行阻塞调用；CPU 或阻塞 I/O 应在 Future 内使用 `tokio::task::spawn_blocking`。
    pub fn add_async_system<Request, Output, Handler, Fut>(&mut self, handler: Handler) -> &mut Self
    where
        Request: Event,
        Output: Event,
        Handler: FnMut(Request) -> Fut + Send + 'static,
        Fut: Future<Output = Output> + Send + 'static,
    {
        self.add_async_system_with_options(handler, AsyncSystemOptions::default())
    }

    pub fn add_async_system_with_options<Request, Output, Handler, Fut>(
        &mut self,
        mut handler: Handler,
        options: AsyncSystemOptions,
    ) -> &mut Self
    where
        Request: Event,
        Output: Event,
        Handler: FnMut(Request) -> Fut + Send + 'static,
        Fut: Future<Output = Output> + Send + 'static,
    {
        assert_ne!(
            TypeId::of::<Request>(),
            TypeId::of::<Output>(),
            "async request and output event types must be different"
        );
        self.add_event::<Request>();
        self.add_event::<Output>();
        self.add_event::<AsyncTaskStarted>();
        self.add_event::<AsyncTaskFailed>();

        let mut reader = self.event_reader::<Request>();
        let spawner = self.async_spawner();
        let request_type = std::any::type_name::<Request>();
        self.async_dispatch.add_system(move |world: &mut World| {
            for request in world.read_events(&mut reader) {
                match spawner.spawn(handler(request), request_type, options) {
                    Ok(task_id) => world.send_event(AsyncTaskStarted {
                        task_id,
                        request_type,
                    }),
                    Err(failure) => world.send_event(failure),
                }
            }
        });
        self
    }

    /// 设置异步请求队列容量。必须在注册首个异步 system 前调用。
    pub fn set_async_queue_capacity(&mut self, capacity: usize) -> &mut Self {
        assert!(capacity > 0, "async queue capacity must be positive");
        assert!(
            self.async_worker.is_none(),
            "async queue capacity cannot change after the worker starts"
        );
        self.async_queue_capacity = capacity;
        self
    }

    /// 设置同时运行的异步任务上限。必须在注册首个异步 system 前调用。
    pub fn set_async_max_in_flight(&mut self, limit: usize) -> &mut Self {
        assert!(limit > 0, "max in-flight tasks must be positive");
        assert!(
            self.async_worker.is_none(),
            "max in-flight tasks cannot change after the worker starts"
        );
        self.async_max_in_flight = limit;
        self
    }

    /// 设置事件保留帧数。必须在注册首个事件类型前调用。
    pub fn set_event_retention_frames(&mut self, frames: u64) -> &mut Self {
        assert!(frames > 0, "event retention must be positive");
        assert!(
            self.event_types.is_empty(),
            "event retention cannot change after events are registered"
        );
        self.event_retention_frames = frames;
        self
    }

    fn async_spawner(&mut self) -> crate::async_runtime::AsyncSpawner {
        if self.async_worker.is_none() {
            let (worker, completion_system) = AsyncWorker::start(
                self.control.clone(),
                self.async_queue_capacity,
                self.async_max_in_flight,
            );
            self.async_completion.add_system(completion_system);
            self.world.add_resource(worker.task_control());
            self.async_worker = Some(worker);
        }
        self.async_worker
            .as_ref()
            .expect("async worker not initialized")
            .spawner()
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    pub fn schedule_mut(&mut self, stage: Stage) -> &mut Schedule {
        self.schedules
            .get_mut(&stage)
            .expect("stage not registered")
    }

    pub fn control(&self) -> AppControl {
        self.control.clone()
    }

    /// 运行到收到 shutdown 请求。没有工作时阻塞当前线程，不会忙轮询。
    pub fn run(&mut self) {
        while !self.control.is_shutdown() {
            self.tick();
            self.control.wait();
        }
        self.async_worker.take();
    }

    /// 执行一轮主循环。第一次调用时先执行一次 Startup。
    pub fn tick(&mut self) {
        self.add_event::<SystemFailed>();
        if !self.started {
            let report = self
                .schedules
                .get_mut(&Stage::Startup)
                .expect("stage not registered")
                .run(&mut self.world);
            self.handle_schedule_report("Startup", report);
            self.started = true;
        }
        let report = self.async_completion.run(&mut self.world);
        self.handle_schedule_report("AsyncCompletion", report);
        for stage in [
            Stage::Input,
            Stage::Prepare,
            Stage::Execute,
            Stage::Finalize,
        ] {
            let report = self
                .schedules
                .get_mut(&stage)
                .expect("stage not registered")
                .run(&mut self.world);
            self.handle_schedule_report(stage.name(), report);
        }
        let report = self.async_dispatch.run(&mut self.world);
        self.handle_schedule_report("AsyncDispatch", report);
        let report = self
            .schedules
            .get_mut(&Stage::Event)
            .expect("stage not registered")
            .run(&mut self.world);
        self.handle_schedule_report("Event", report);
        let report = self.event_maintenance.run(&mut self.world);
        self.handle_schedule_report("EventMaintenance", report);
    }

    fn handle_schedule_report(&mut self, schedule: &'static str, report: ScheduleReport) {
        if let Some(message) = report.ordering_error {
            tracing::error!(schedule, %message, "schedule configuration failed");
            self.world.send_event(SystemFailed {
                schedule,
                system: None,
                message,
            });
            self.control.shutdown();
        }
        for failure in report.failures {
            tracing::error!(
                schedule,
                system = failure.system.unwrap_or("<anonymous>"),
                message = %failure.message,
                "system execution failed"
            );
            self.world.send_event(SystemFailed {
                schedule,
                system: failure.system,
                message: failure.message,
            });
        }
    }
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Stage::Startup => "Startup",
            Stage::Input => "Input",
            Stage::Prepare => "Prepare",
            Stage::Execute => "Execute",
            Stage::Finalize => "Finalize",
            Stage::Event => "Event",
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Counter(i32);

    #[test]
    fn app_stages_run() {
        let mut app = App::new();
        app.world_mut().add_resource(Counter(0));

        app.add_systems(
            Stage::Execute,
            [|world: &mut World| {
                let c = world.resource_mut::<Counter>().unwrap();
                c.0 += 1;
            }],
        );

        app.tick();
        assert_eq!(app.world().resource::<Counter>().unwrap().0, 1);
    }

    #[test]
    fn startup_runs_once_when_ticking_manually() {
        let mut app = App::new();
        app.world_mut().add_resource(Counter(0));
        app.add_systems(
            Stage::Startup,
            [|world: &mut World| world.resource_mut::<Counter>().unwrap().0 += 1],
        );

        app.tick();
        app.tick();

        assert_eq!(app.world().resource::<Counter>().unwrap().0, 1);
    }

    #[test]
    fn run_waits_for_wake_and_stops_on_shutdown() {
        let mut app = App::new();
        let ticks = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let system_ticks = ticks.clone();
        app.add_systems(
            Stage::Execute,
            [move |_world: &mut World| {
                system_ticks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }],
        );
        let control = app.control();
        let thread = std::thread::spawn(move || app.run());

        wait_until(|| ticks.load(std::sync::atomic::Ordering::SeqCst) == 1);
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert_eq!(ticks.load(std::sync::atomic::Ordering::SeqCst), 1);

        control.wake();
        wait_until(|| ticks.load(std::sync::atomic::Ordering::SeqCst) == 2);
        control.shutdown();
        thread.join().unwrap();
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        for _ in 0..100 {
            if condition() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("condition was not met before timeout");
    }

    #[test]
    fn event_stage_reads_events_sent_earlier_in_the_same_tick() {
        let mut app = App::new();
        app.add_event::<i32>();
        let mut reader = app.event_reader::<i32>();
        let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let received_by_system = received.clone();
        app.add_systems(
            Stage::Execute,
            [|world: &mut World| {
                world.send_event(7_i32);
            }],
        );
        app.add_systems(
            Stage::Event,
            [move |world: &mut World| {
                received_by_system
                    .lock()
                    .unwrap()
                    .extend(world.read_events(&mut reader));
            }],
        );

        app.tick();

        assert_eq!(*received.lock().unwrap(), vec![7]);
    }

    #[derive(Clone)]
    struct DoubleRequest(i32);

    #[derive(Clone, Debug, PartialEq)]
    struct DoubleCompleted(i32);

    #[test]
    fn async_system_spawns_each_request_and_returns_output_events() {
        let mut app = App::new();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        app.add_async_system(move |request: DoubleRequest| {
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                DoubleCompleted(request.0 * 2)
            }
        });

        let mut sent = false;
        app.add_systems(
            Stage::Execute,
            [move |world: &mut World| {
                if !sent {
                    world.send_event(DoubleRequest(21));
                    world.send_event(DoubleRequest(22));
                    sent = true;
                }
            }],
        );

        let completed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let completed_by_system = completed.clone();
        let mut reader = app.event_reader::<DoubleCompleted>();
        app.add_systems(
            Stage::Event,
            [move |world: &mut World| {
                completed_by_system
                    .lock()
                    .unwrap()
                    .extend(world.read_events(&mut reader));
            }],
        );

        for _ in 0..100 {
            app.tick();
            if completed.lock().unwrap().len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        let mut completed = completed.lock().unwrap().clone();
        completed.sort_by_key(|event| event.0);
        assert_eq!(completed, [DoubleCompleted(42), DoubleCompleted(44)]);
    }

    #[derive(Clone)]
    struct SlowRequest;

    #[derive(Clone)]
    struct SlowCompleted;

    #[test]
    fn async_timeout_becomes_a_failure_event() {
        let mut app = App::new();
        app.add_async_system_with_options(
            |_request: SlowRequest| async {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                SlowCompleted
            },
            AsyncSystemOptions {
                timeout: Some(std::time::Duration::from_millis(2)),
            },
        );
        let failures = collect_events::<AsyncTaskFailed>(&mut app);
        send_once(&mut app, SlowRequest);

        tick_until(&mut app, || !failures.lock().unwrap().is_empty());

        assert_eq!(
            failures.lock().unwrap()[0].kind,
            crate::AsyncTaskFailureKind::Timeout
        );
    }

    #[derive(Clone)]
    struct PanicRequest;

    #[derive(Clone)]
    struct PanicCompleted;

    #[test]
    fn async_panic_becomes_a_failure_event() {
        let mut app = App::new();
        app.add_async_system::<PanicRequest, PanicCompleted, _, _>(|_request| async {
            panic!("async boom")
        });
        let failures = collect_events::<AsyncTaskFailed>(&mut app);
        send_once(&mut app, PanicRequest);

        tick_until(&mut app, || !failures.lock().unwrap().is_empty());

        assert_eq!(
            failures.lock().unwrap()[0].kind,
            crate::AsyncTaskFailureKind::Panic
        );
    }

    #[derive(Clone)]
    struct PendingRequest;

    #[derive(Clone)]
    struct PendingCompleted;

    #[test]
    fn async_task_can_be_cancelled_from_the_main_thread() {
        let mut app = App::new();
        app.add_async_system_with_options(
            |_request: PendingRequest| std::future::pending::<PendingCompleted>(),
            AsyncSystemOptions { timeout: None },
        );
        let failures = collect_events::<AsyncTaskFailed>(&mut app);
        let mut started_reader = app.event_reader::<AsyncTaskStarted>();
        app.add_systems(
            Stage::Event,
            [move |world: &mut World| {
                for started in world.read_events(&mut started_reader) {
                    assert!(world.cancel_async_task(started.task_id));
                }
            }],
        );
        send_once(&mut app, PendingRequest);

        tick_until(&mut app, || !failures.lock().unwrap().is_empty());

        assert_eq!(
            failures.lock().unwrap()[0].kind,
            crate::AsyncTaskFailureKind::Cancelled
        );
    }

    #[test]
    fn async_backpressure_reports_queue_full() {
        let mut app = App::new();
        app.set_async_queue_capacity(1)
            .set_async_max_in_flight(1)
            .add_async_system_with_options(
                |_request: PendingRequest| std::future::pending::<PendingCompleted>(),
                AsyncSystemOptions { timeout: None },
            );
        let failures = collect_events::<AsyncTaskFailed>(&mut app);
        let mut sent = false;
        app.add_systems(
            Stage::Execute,
            [move |world: &mut World| {
                if !sent {
                    for _ in 0..100 {
                        world.send_event(PendingRequest);
                    }
                    sent = true;
                }
            }],
        );

        app.tick();

        assert!(failures
            .lock()
            .unwrap()
            .iter()
            .any(|failure| failure.kind == crate::AsyncTaskFailureKind::QueueFull));
    }

    #[test]
    fn system_panic_becomes_an_app_event() {
        let mut app = App::new();
        app.add_event::<SystemFailed>();
        let failures = collect_events::<SystemFailed>(&mut app);
        app.schedule_mut(Stage::Execute)
            .add_system(crate::named_system("broken", |_world| {
                panic!("broken system")
            }));

        app.tick();

        assert_eq!(failures.lock().unwrap().len(), 1);
        assert_eq!(failures.lock().unwrap()[0].schedule, "Execute");
        assert_eq!(failures.lock().unwrap()[0].system, Some("broken"));
    }

    #[derive(Clone)]
    struct StressRequest(u64);

    #[derive(Clone)]
    struct StressCompleted(u64);

    #[test]
    fn async_worker_completes_many_tasks_with_unique_ids() {
        const TASKS: u64 = 200;
        let mut app = App::new();
        app.set_async_queue_capacity(TASKS as usize)
            .set_async_max_in_flight(32)
            .add_async_system(|request: StressRequest| async move {
                tokio::task::yield_now().await;
                StressCompleted(request.0)
            });
        let completed = collect_events::<StressCompleted>(&mut app);
        let started = collect_events::<AsyncTaskStarted>(&mut app);
        let failures = collect_events::<AsyncTaskFailed>(&mut app);
        let mut sent = false;
        app.add_systems(
            Stage::Execute,
            [move |world: &mut World| {
                if !sent {
                    for id in 0..TASKS {
                        world.send_event(StressRequest(id));
                    }
                    sent = true;
                }
            }],
        );

        tick_until(&mut app, || {
            completed.lock().unwrap().len() == TASKS as usize
        });

        let completed_ids: std::collections::HashSet<_> = completed
            .lock()
            .unwrap()
            .iter()
            .map(|event| event.0)
            .collect();
        let task_ids: std::collections::HashSet<_> = started
            .lock()
            .unwrap()
            .iter()
            .map(|event| event.task_id)
            .collect();
        assert_eq!(completed_ids.len(), TASKS as usize);
        assert_eq!(task_ids.len(), TASKS as usize);
        assert!(failures.lock().unwrap().is_empty());
    }

    #[test]
    fn dropping_app_cancels_pending_async_tasks() {
        let start = std::time::Instant::now();
        {
            let mut app = App::new();
            app.add_async_system_with_options(
                |_request: PendingRequest| std::future::pending::<PendingCompleted>(),
                AsyncSystemOptions { timeout: None },
            );
            send_once(&mut app, PendingRequest);
            app.tick();
        }

        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }

    fn send_once<E: Event>(app: &mut App, event: E) {
        let mut event = Some(event);
        app.add_systems(
            Stage::Execute,
            [move |world: &mut World| {
                if let Some(event) = event.take() {
                    world.send_event(event);
                }
            }],
        );
    }

    fn collect_events<E: Event>(app: &mut App) -> std::sync::Arc<std::sync::Mutex<Vec<E>>> {
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let system_events = events.clone();
        let mut reader = app.event_reader::<E>();
        app.add_systems(
            Stage::Event,
            [move |world: &mut World| {
                system_events
                    .lock()
                    .unwrap()
                    .extend(world.read_events(&mut reader));
            }],
        );
        events
    }

    fn tick_until(app: &mut App, mut condition: impl FnMut() -> bool) {
        for _ in 0..200 {
            app.tick();
            if condition() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("condition was not met before timeout");
    }
}
