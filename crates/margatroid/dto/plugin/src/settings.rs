use crate::{
    monitor::{MonitorCompletion, MonitorCompletionGuard},
    types::DtoState,
};
use agent_core::{Agent, AgentObservationWait};
use async_runtime_plugin::AsyncRuntimeHandle;
use axum_plugin::{AxumLifetime, AxumWorkGuard};
use core_plugin::{Entity, Resource, World};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use storage_core::{AgentStorage, StorageOperation};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct SettingsObservations(BTreeMap<Entity, SettingsSlot>);
impl Resource for SettingsObservations {}
struct SettingsSlot {
    storage: AgentStorage,
    path: PathBuf,
    cancel: CancellationToken,
    completion: Arc<MonitorCompletion>,
}
impl Drop for SettingsSlot {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub(crate) fn install(world: &mut World) {
    if !world.contains_resource::<SettingsObservations>() {
        world.insert_resource(SettingsObservations::default());
    }
}

pub(crate) fn refresh_settings_watch(
    world: &mut World,
    entity: Entity,
) -> Vec<AgentObservationWait> {
    install(world);
    let Some(state) = world.get_resource::<DtoState>().cloned() else {
        return stop_settings_watch(world, entity);
    };
    let desired = if state.lifetime.is_cancelled() || !world.contains_component::<Agent>(entity) {
        None
    } else {
        world
            .get_component::<AgentStorage>(entity)
            .filter(|storage| storage.is_open())
            .cloned()
    };
    let same = world
        .get_resource::<SettingsObservations>()
        .unwrap()
        .0
        .get(&entity)
        .is_some_and(|slot| {
            desired.as_ref().is_some_and(|storage| {
                storage.same_database(&slot.storage)
                    && storage.settings_path() == slot.path
                    && !slot.cancel.is_cancelled()
            })
        });
    if same {
        return vec![];
    }
    let waits = stop_settings_watch(world, entity);
    let Some(storage) = desired else {
        return waits;
    };
    let path = storage.settings_path().to_path_buf();
    let cancel = CancellationToken::new();
    let completion = Arc::new(MonitorCompletion::default());
    let context = SettingsContext {
        storage: storage.clone(),
        path: path.clone(),
        cancel: cancel.clone(),
        lifetime: state.lifetime.clone(),
        guard: state.lifetime.track(),
        completion: MonitorCompletionGuard(completion.clone()),
    };
    world
        .get_resource_mut::<SettingsObservations>()
        .unwrap()
        .0
        .insert(
            entity,
            SettingsSlot {
                storage,
                path,
                cancel,
                completion,
            },
        );
    world
        .get_resource::<AsyncRuntimeHandle>()
        .expect("settings monitor requires async runtime")
        .spawn(run_settings_watch(context));
    waits
}

pub(crate) fn stop_settings_watch(world: &mut World, entity: Entity) -> Vec<AgentObservationWait> {
    let slot = world
        .get_resource_mut::<SettingsObservations>()
        .and_then(|slots| slots.0.remove(&entity));
    match slot {
        Some(slot) => {
            let wait = slot.completion.clone().wait();
            drop(slot);
            vec![wait]
        }
        None => vec![],
    }
}

struct SettingsContext {
    storage: AgentStorage,
    path: PathBuf,
    cancel: CancellationToken,
    lifetime: AxumLifetime,
    guard: AxumWorkGuard,
    completion: MonitorCompletionGuard,
}
#[derive(Default)]
struct ChangeSignal {
    dirty: AtomicBool,
    changed: Notify,
    error: Mutex<Option<String>>,
}
impl ChangeSignal {
    fn mark(&self) {
        self.dirty.store(true, Ordering::Release);
        self.changed.notify_one();
    }
    fn event(&self, path: &Path, result: notify::Result<Event>) {
        match result {
            Ok(event) if relevant(path, &event) => self.mark(),
            Ok(_) => {}
            Err(error) => {
                *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error.to_string());
                self.mark();
            }
        }
    }
}
fn relevant(path: &Path, event: &Event) -> bool {
    !matches!(event.kind, EventKind::Access(_))
        && event
            .paths
            .iter()
            .any(|changed| changed == path || path.starts_with(changed))
}
fn create_watcher(path: &Path, signal: Arc<ChangeSignal>) -> Result<RecommendedWatcher, String> {
    let target = path.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |event| signal.event(&target, event))
        .map_err(|e| e.to_string())?;
    let parent = path
        .parent()
        .ok_or_else(|| "settings path has no parent".to_owned())?;
    let mut anchor = parent;
    while !anchor.is_dir() {
        anchor = anchor
            .parent()
            .ok_or_else(|| "settings watch has no existing ancestor".to_owned())?;
    }
    watcher
        .watch(
            anchor,
            if anchor == parent {
                RecursiveMode::NonRecursive
            } else {
                RecursiveMode::Recursive
            },
        )
        .map_err(|e| e.to_string())?;
    // The grandparent observes replacement/deletion of the directory itself.
    if anchor == parent
        && let Some(ancestor) = parent.parent()
    {
        watcher
            .watch(ancestor, RecursiveMode::NonRecursive)
            .map_err(|e| e.to_string())?;
    }
    Ok(watcher)
}
fn validate_settings(storage: &AgentStorage) -> Result<(), String> {
    storage_core::execute_storage(storage, StorageOperation::ReadConfig)
        .map(|_| ())
        .map_err(|e| e.to_string())
}
fn report(path: &Path, error: Option<String>, previous: &mut Option<String>) {
    if let Some(error) = error {
        if previous.as_ref() != Some(&error) {
            tracing::warn!(path=%path.display(),%error,"Agent settings source unavailable");
        }
        *previous = Some(error);
    } else {
        *previous = None;
    }
}

async fn run_settings_watch(context: SettingsContext) {
    let SettingsContext {
        storage,
        path,
        cancel,
        lifetime,
        guard,
        completion,
    } = context;
    // Completion must be published only after the work guard and every worker have ended.
    let _completion = completion;
    let _guard = guard;
    let signal = Arc::new(ChangeSignal::default());
    let mut watcher = None;
    let mut previous_error = None;
    signal.dirty.store(true, Ordering::Release);
    loop {
        if cancel.is_cancelled() || lifetime.is_cancelled() {
            break;
        }
        if watcher.is_none() {
            let target = path.clone();
            let events = signal.clone();
            let created =
                tokio::task::spawn_blocking(move || create_watcher(&target, events)).await;
            match created {
                Ok(Ok(value)) => {
                    watcher = Some(value);
                    signal.dirty.store(true, Ordering::Release);
                }
                Ok(Err(error)) => report(&path, Some(error), &mut previous_error),
                Err(error) => report(&path, Some(error.to_string()), &mut previous_error),
            }
            if cancel.is_cancelled() || lifetime.is_cancelled() {
                break;
            }
        }
        if signal.dirty.swap(false, Ordering::AcqRel) {
            let watch_error = signal
                .error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            if watch_error.is_some() {
                report(&path, watch_error, &mut previous_error);
                watcher = None;
            }
            // Never select cancellation against this worker: it owns a live database access.
            let source = storage.clone();
            let result = tokio::task::spawn_blocking(move || validate_settings(&source)).await;
            let read_error = match result {
                Ok(result) => result.err(),
                Err(error) => Some(error.to_string()),
            };
            if cancel.is_cancelled() || lifetime.is_cancelled() {
                break;
            }
            let read_error = read_error.or_else(|| {
                if watcher.is_none() {
                    previous_error.clone()
                } else {
                    None
                }
            });
            report(&path, read_error, &mut previous_error);
        }
        // Missing/failed watch anchors are retried; a healthy idle monitor only waits for events.
        tokio::select! {
            biased;
            _=cancel.cancelled()=>break,
            _=lifetime.cancelled()=>break,
            _=signal.changed.notified()=>{
                tokio::select!{
                    biased;
                    _=cancel.cancelled()=>break,
                    _=lifetime.cancelled()=>break,
                    _=tokio::time::sleep(Duration::from_millis(100))=>{},
                }
                // Re-arm the parent watch after a directory replacement.
                watcher=None;
            },
            _=tokio::time::sleep(Duration::from_secs(1)),if watcher.is_none()=>{},
        }
    }
    drop(watcher);
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    #[test]
    fn filter_ignores_unrelated_files_and_access_but_observes_replacement() {
        let target = Path::new("/data/agent/setting.toml");
        let modify = Event::new(EventKind::Modify(notify::event::ModifyKind::Any));
        assert!(relevant(
            target,
            &modify.clone().add_path(target.to_path_buf())
        ));
        assert!(relevant(
            target,
            &modify.clone().add_path(PathBuf::from("/data/agent"))
        ));
        assert!(!relevant(
            target,
            &modify.add_path(PathBuf::from("/data/agent/other.toml"))
        ));
        assert!(!relevant(
            target,
            &Event::new(EventKind::Access(notify::event::AccessKind::Any))
                .add_path(target.to_path_buf())
        ));
    }
    #[tokio::test]
    async fn actual_parent_watch_observes_atomic_replace_and_delete() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("setting.toml");
        std::fs::write(&target, "value=1\n").unwrap();
        let signal = Arc::new(ChangeSignal::default());
        let _watcher = create_watcher(&target, signal.clone()).unwrap();
        let replacement = root.path().join("replacement.toml");
        std::fs::write(&replacement, "value=2\n").unwrap();
        std::fs::rename(&replacement, &target).unwrap();
        tokio::time::timeout(Duration::from_secs(3), signal.changed.notified())
            .await
            .unwrap();
        assert!(signal.dirty.swap(false, Ordering::AcqRel));
        while signal.changed.notified().now_or_never().is_some() {}
        std::fs::remove_file(&target).unwrap();
        tokio::time::timeout(Duration::from_secs(3), signal.changed.notified())
            .await
            .unwrap();
        assert!(signal.dirty.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn lifecycle_refresh_is_idempotent_and_stop_returns_real_completion() {
        use app_runtime_plugin::RuntimePlugin;
        use closure_plugin::{AppClosureExt, ClosurePlugin, WorldClosureExt};
        let root = tempfile::tempdir().unwrap();
        let storage = storage_core::open_storage(
            &root.path().join("state.sqlite"),
            &root.path().join("setting.toml"),
            "value=1\n",
        )
        .unwrap();
        let mut app = core_plugin::App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(async_runtime_plugin::AsyncRuntimePlugin)
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(axum_plugin::AxumPlugin::new("127.0.0.1:0".parse().unwrap()));
        let lifetime = app
            .world()
            .get_resource::<axum_plugin::AxumHandle>()
            .unwrap()
            .lifetime();
        let world = app.world().closure_sender(RuntimePlugin::UPDATE);
        app.world_mut().insert_resource(DtoState {
            world,
            ingress: config_core::IngressPolicy::Localhost,
            events: Arc::default(),
            clients: client_core::Clients::default(),
            lifetime,
        });
        let entity = app.world_mut().spawn();
        app.world_mut().insert_component(
            entity,
            Agent::new(entity, entity, "task".into(), root.path().into()),
        );
        app.world_mut().insert_component(entity, storage.clone());
        assert!(refresh_settings_watch(app.world_mut(), entity).is_empty());
        let first = app
            .world()
            .get_resource::<SettingsObservations>()
            .unwrap()
            .0
            .get(&entity)
            .unwrap()
            .completion
            .clone();
        assert!(refresh_settings_watch(app.world_mut(), entity).is_empty());
        assert!(Arc::ptr_eq(
            &first,
            &app.world()
                .get_resource::<SettingsObservations>()
                .unwrap()
                .0
                .get(&entity)
                .unwrap()
                .completion
        ));
        assert!(first.clone().wait().now_or_never().is_none());
        let waits = stop_settings_watch(app.world_mut(), entity);
        assert_eq!(waits.len(), 1);
        for wait in waits {
            tokio::time::timeout(Duration::from_secs(3), wait)
                .await
                .unwrap()
                .unwrap();
        }
        assert!(stop_settings_watch(app.world_mut(), entity).is_empty());
        assert!(first.wait().now_or_never().unwrap().is_ok());
        storage_core::close_storage(&storage).unwrap();
    }

    #[test]
    fn settings_validation_uses_current_file_and_does_not_cache_body() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("setting.toml");
        let storage =
            storage_core::open_storage(&root.path().join("state.sqlite"), &path, "name='initial'")
                .unwrap();
        validate_settings(&storage).unwrap();
        std::fs::write(&path, "invalid = [").unwrap();
        assert!(validate_settings(&storage).is_err());
        let replacement = root.path().join("replacement.toml");
        std::fs::write(&replacement, "[custom]\nanything=true\n").unwrap();
        std::fs::rename(replacement, &path).unwrap();
        validate_settings(&storage).unwrap();
        storage_core::close_storage(&storage).unwrap();
        assert!(validate_settings(&storage).is_err());
    }
}
