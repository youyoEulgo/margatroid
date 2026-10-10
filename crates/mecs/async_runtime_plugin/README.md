# AsyncRuntimePlugin

## 介绍

`AsyncRuntimePlugin` 提供专用异步执行线程，不把 Tokio 引入
`core_plugin`。同步 System 在主线程访问 `World`，异步任务处理拥有所有权的数据，通过事件把结果交回。

支持两种任务入口：

| 用法                    | 提交内容               | 任务处理方式                              |
| ----------------------- | ---------------------- | ----------------------------------------- |
| 异步 System（事件模式） | 请求数据               | 预先注册、可复用的 handler                |
| 异步闭包（闭包模式）    | 一次性闭包及其捕获数据 | 通过 `ClosurePlugin` 在指定 Schedule 分发 |

**两种入口的自动结果都是 `Result<T, E>` 事件。** 发送任务不会直接返回 `T`，接收方必须注册同步
System，使用 `world.event_reader::<Result<T, E>>()` 读取结果。

## 异步 System：注册、发送、接收

下面是完整的事件模式示例。请求包含关联 ID；异步函数计算结果，接收 System 把成功结果保存到
`World` 的 Resource 中，并处理错误。任务调度与帧推进由 `app.run()` 驱动。

```rust
use std::collections::BTreeMap;

use app_runtime_plugin::{AppRunExt, RuntimePlugin};
use async_runtime_plugin::{AppAsyncExt, AsyncRuntimePlugin, AsyncTaskError, WorldAsyncExt};
use core_plugin::{App, Event, Resource, World};

struct Calculate {
    id: u64,
    value: u32,
}
impl Event for Calculate {}

struct Calculated {
    id: u64,
    value: u32,
}

#[derive(Debug)]
enum CalculateError {
    Overflow { id: u64 },
    Task(AsyncTaskError),
}

impl From<AsyncTaskError> for CalculateError {
    fn from(error: AsyncTaskError) -> Self {
        Self::Task(error)
    }
}

#[derive(Default)]
struct Answers(BTreeMap<u64, u32>);
impl Resource for Answers {}

async fn calculate(request: Calculate) -> Result<Calculated, CalculateError> {
    let value = request
        .value
        .checked_mul(2)
        .ok_or(CalculateError::Overflow { id: request.id })?;
    Ok(Calculated { id: request.id, value })
}

fn receive_calculations(world: &mut World) {
    let mut answers = Vec::new();
    for result in world.event_reader::<Result<Calculated, CalculateError>>() {
        match result {
            Ok(result) => answers.push((result.id, result.value)),
            Err(CalculateError::Overflow { id }) => {
                eprintln!("calculation {id} overflowed");
            }
            Err(CalculateError::Task(error)) => {
                eprintln!("calculation task failed: {error}");
            }
        }
    }
    let stored = world.get_resource_mut::<Answers>().unwrap();
    stored.0.extend(answers);
}

fn main() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_async_system(RuntimePlugin::UPDATE, calculate)
        .add_system(RuntimePlugin::UPDATE, receive_calculations);
    app.world_mut().insert_resource(Answers::default());

    app.world().send_async_event(Calculate { id: 1, value: 21 });
    app.run();
}
```

注意：

- `Calculate` 必须实现 `Event`。`Calculated` 不必单独实现，因为框架为满足线程安全约束的
  `Result<T, E>` 提供了 `Event` 实现。
- 同一请求类型最多注册一个异步 handler，不按 Schedule 区分多次注册。
- `send_async_event(request)` 必须与 `add_async_system` 配合使用。普通 `send_event(request)`
  不会触发异步 handler，因为异步入口读取的是内部包装类型 `AsyncEventRequest<Request>`。
- 读取事件时持有 `World` 的不可变借用，示例先提取结果，再修改 `Answers`，避免借用冲突。
- `app.run()` 是持续运行的事件循环，不会因为这一批任务完成就自动退出。需要退出时由应用自行控制。

## 异步闭包：提交、接收

闭包模式不需要为任务注册请求类型。下面提交一个一次性任务，并通过同步 System 接收其结果。

```rust
use app_runtime_plugin::{AppRunExt, RuntimePlugin};
use async_runtime_plugin::{AsyncRuntimePlugin, AsyncTaskError, WorldAsyncExt};
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, World};

struct SumCompleted {
    id: u64,
    value: u64,
}

#[derive(Debug)]
enum SumError {
    Overflow { id: u64 },
    Task(AsyncTaskError),
}

impl From<AsyncTaskError> for SumError {
    fn from(error: AsyncTaskError) -> Self {
        Self::Task(error)
    }
}

fn receive_sums(world: &mut World) {
    for result in world.event_reader::<Result<SumCompleted, SumError>>() {
        match result {
            Ok(result) => println!("sum {} = {}", result.id, result.value),
            Err(SumError::Overflow { id }) => eprintln!("sum {id} overflowed"),
            Err(SumError::Task(error)) => eprintln!("sum task failed: {error}"),
        }
    }
}

fn main() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_plugin(AsyncRuntimePlugin)
        .add_system(RuntimePlugin::UPDATE, receive_sums);

    let id = 7;
    let values = vec![10_u64, 20, 30];
    app.world().send_async_closure(
        RuntimePlugin::PRE_UPDATE,
        move || async move {
            let value = values
                .into_iter()
                .try_fold(0_u64, |sum, value| sum.checked_add(value))
                .ok_or(SumError::Overflow { id })?;
            Ok::<SumCompleted, SumError>(SumCompleted { id, value })
        },
    );
    app.run();
}
```

闭包模式必须安装 `ClosurePlugin`，并在目标 Schedule 注册 `add_closure_system`。
异步插件复用它的 `ClosureRequest` 和调度逻辑，不额外安装异步闭包 Dispatcher。

异步闭包接收的不是 `&mut World`。它捕获的是可以移入异步线程的数据；结果接收 System
才在主线程访问 `World`。不要把借用的组件引用捕获进任务。

## 事件通道与 pending 升变

两种入口在通道里的变化如下：

```text
事件模式：
    send_async_event(Request)
        -> 普通事件 AsyncEventRequest<Request> 入队
        -> 后续事件帧可读，注册的 AsyncEventSystem 取出 Request
        -> 创建另一个 Pending 响应节点
        -> 提交异步任务

闭包模式：
    send_async_closure(schedule, task)
        -> 普通事件 ClosureRequest 入队
        -> 后续事件帧可读，目标 ClosureSystem 执行同步分发包装
        -> 创建另一个 Pending 响应节点
        -> 提交异步任务

共同的响应过程：
    Pending
        -> 任务得到 Result<T, E>
        -> EventHandle::complete 将同一个响应节点升变为 Normal<Result<T, E>>
        -> 后续事件帧把它移入对应类型的读取存储
        -> 同步 System 通过 event_reader::<Result<T, E>>() 读取
```

请求节点和响应节点是两个不同节点。**不是 `Request` 自动变成 `T`，也不是 Rust 类型在运行时改变。**
响应句柄从创建时就是 `EventHandle<Result<T, E>>`，升变是内部状态由“尚无结果”变为“普通结果事件”。

- Pending 节点保留在队列中，但不会进入普通事件读取器。
- 任务完成只使结果可以在后续事件帧分发，不会让当前帧的读取器立刻看到它。
- 手动使用 `app.tick()` 时，必须继续推进事件帧；只调用一次通常只能启动任务，不能保证读到结果。
- 事件读取存储每帧更新，接收 System 应持续注册运行；不主动复制保存的结果不会永久保留。
- 同一帧多个 System 可以读取同一种结果事件；普通读取不会独占消费。业务若只需要一个处理者，应自行约束注册。

## Async 与 Await

事件模式和闭包模式各有两个提交方法：

| 模式  | 事件入口           | 闭包入口             | 帧推进                         |
| ----- | ------------------ | -------------------- | ------------------------------ |
| Async | `send_async_event` | `send_async_closure` | 不因该任务阻止下一帧           |
| Await | `send_await_event` | `send_await_closure` | 任务实际分发时关阀，完成后开阀 |

Await 不是在发送调用处同步等待，也不直接返回计算结果。当前帧会继续运行，运行时在后续帧开始前
等待所有 Await 任务释放阀；结果仍以同样的 `Result<T, E>` 事件接收。

使用 `app.run()` 时运行阀由 Runtime 处理。直接调用 `app.tick()` 只推进一次 ECS 更新，不能把它当作
验证 Await 门控行为的完整 Runtime 事件循环。

任务 panic 或内层任务取消会由监督逻辑转换为 `AsyncTaskError`，再通过 `E::from` 产生错误结果。
应用或异步执行器整体关闭时，不保证接收 System 还能继续运行并处理结果。

## AsyncContext：发送进度与额外事件

固定 handler 和一次性闭包都可声明 `AsyncContext` 参数。它只提供事件发送能力，不持有 `World`。
下面示例基于事件模式中的 `Calculate`、`Calculated`、`CalculateError`、`Answers` 和
`receive_calculations` 定义；用带上下文的 handler 替换原 handler，不要对 `Calculate` 重复注册。

```rust
use async_runtime_plugin::AsyncContext;

struct Progress {
    id: u64,
    percent: u8,
}
impl Event for Progress {}

async fn calculate_with_progress(
    request: Calculate,
    context: AsyncContext,
) -> Result<Calculated, CalculateError> {
    context.send_event(Progress { id: request.id, percent: 0 });
    let result = calculate(request).await?;
    context.send_event(Progress { id: result.id, percent: 100 });
    Ok(result)
}

fn receive_progress(world: &mut World) {
    for progress in world.event_reader::<Progress>() {
        println!("calculation {}: {}%", progress.id, progress.percent);
    }
}

fn run_with_progress() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_async_system(RuntimePlugin::UPDATE, calculate_with_progress)
        .add_system(RuntimePlugin::UPDATE, receive_progress)
        .add_system(RuntimePlugin::UPDATE, receive_calculations);
    app.world_mut().insert_resource(Answers::default());
    app.world().send_async_event(Calculate { id: 2, value: 30 });
    app.run();
}
```

闭包对应的参数形状是：

```rust
move |context: AsyncContext| async move {
    context.send_event(Progress { id, percent: 100 });
    Ok::<SumCompleted, SumError>(SumCompleted { id, value: 60 })
}
```

显式 `context.send_event(Progress { ... })` 创建的是**额外的普通事件**，不会替代自动生成的
`Result<T, E>` 响应。多个进度事件可能在同一事件帧一起被读取，进度发送并不要求每次都触发独立一帧。

## 结果类型和请求关联

自动结果按完整 Rust 类型 `Result<T, E>` 分通道，而不是按 handler 名、Schedule 或请求类型分通道。

- 两个任务都返回 `Result<String, anyhow::Error>` 时，结果进入同一通道。
- 框架不会自动附带请求 ID；需要关联时把 ID 放入成功值和可预期错误中。
- 框架生成的 panic/cancel 错误通过 `From<AsyncTaskError>` 转换，没有自动注入该请求的 ID。
  若必须把此类失败也精确关联到请求，应用需要另行设计关联机制，不能假定错误转换拥有请求上下文。
- 推荐使用具名结果结构体，例如 `Calculated`，不要让不同操作都返回无关联信息的裸 `String` 或 `()`。
- 异步函数返回 `Result<Completed, TaskError>` 时，自动事件是整个 `Result<Completed, TaskError>`，
  不是裸 `Completed`。如需公开独立的完成事件，由同步接收 System 显式转换并发送。

## 文件读取与 anyhow

使用 `anyhow::Error` 时无需定义专门的错误枚举。下面是独立可运行的文件读取示例，包含结果接收。
使用此例的项目需要声明 `anyhow` 和启用文件系统功能的 `tokio` 依赖。

```rust
use app_runtime_plugin::{AppRunExt, RuntimePlugin};
use async_runtime_plugin::{AppAsyncExt, AsyncRuntimePlugin, WorldAsyncExt};
use core_plugin::{App, Event, World};

struct ReadText {
    id: u64,
    path: String,
}
impl Event for ReadText {}

struct TextRead {
    id: u64,
    content: String,
}

async fn read_text(request: ReadText) -> anyhow::Result<TextRead> {
    use anyhow::Context;
    let content = tokio::fs::read_to_string(&request.path)
        .await
        .with_context(|| format!("request {}: cannot read {}", request.id, request.path))?;
    Ok(TextRead { id: request.id, content })
}

fn receive_text(world: &mut World) {
    for result in world.event_reader::<Result<TextRead, anyhow::Error>>() {
        match result {
            Ok(text) => println!("request {}: {} bytes", text.id, text.content.len()),
            Err(error) => eprintln!("read failed: {error:#}"),
        }
    }
}

fn main() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_async_system(RuntimePlugin::UPDATE, read_text)
        .add_system(RuntimePlugin::UPDATE, receive_text);
    app.world().send_async_event(ReadText {
        id: 3,
        path: "input.txt".to_owned(),
    });
    app.run();
}
```

## 长期服务

`spawn_async_service` 直接提交 `Future<Output = ()>`，供调用方自行管理服务生命周期。
它不创建请求事件、Pending 响应或自动完成事件，也不操作运行阀。若服务需要通知应用，必须自行发送事件。

## 公开 API

- `AsyncRuntimePlugin`：启动并管理专用异步执行线程。
- `AppAsyncExt::add_async_system`：绑定请求类型、固定异步 handler 与 Schedule。
- `WorldAsyncExt::send_async_event`、`send_await_event`：发送事件模式请求。
- `WorldAsyncExt::send_async_closure`、`send_await_closure`：提交闭包模式任务。
- `WorldAsyncExt::spawn_async_service`：提交调用方自行管理的长期 Future。
- `AsyncEventHandler`：适配有或没有 `AsyncContext` 的可复用 handler。
- `AsyncTask`：适配有或没有 `AsyncContext` 的一次性任务。
- `AsyncContext`：在异步任务中发送普通事件或带帧延迟的事件。
- `AsyncTaskError`：描述任务 panic 或取消。

请求、返回值、错误及其关联方式都由使用者定义；本设施不依赖任何特定业务领域。
