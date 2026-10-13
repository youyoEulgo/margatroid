#[path = "../../tool_resource_plugin/tests/support/mod.rs"]
mod support;
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity};
use margatroid_types::agent_components::{Agent, WorldAgentExt};
use margatroid_types::block_types::{BlockInner, InnerType};
use margatroid_types::execution_components::{AgentExecutionScope, ExecutionActivityKind};
use margatroid_types::resource_events::PromptResourceTaskError;
use margatroid_types::resource_events::{
    PromptResourceReadOutput, ResourceLoadError, ResourceLoadErrorKind as Kind,
};
use prompt_resource_plugin::PromptResource;
use prompt_resource_plugin::{LoadPromptResource, PromptResourcePlugin};
use resource_id_plugin::{ResourceID, ResourcePlugin, ResourceSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn fixture() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "margatroid-prompt-resource-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let directory = root.join("prompts/local/sample/latest");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("prompt.toml"), "role = \"system\"\n").unwrap();
    fs::write(directory.join("content.md"), "prompt body").unwrap();
    root
}
fn app(root: &Path) -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
        .add_plugin(PromptResourcePlugin::open(root).unwrap());
    app
}
fn request(
    target: Entity,
    alias: &str,
) -> (
    LoadPromptResource,
    tokio::sync::oneshot::Receiver<Result<(), ResourceLoadError>>,
    AgentExecutionScope,
) {
    let scope = AgentExecutionScope::new();
    let activity = Arc::new(scope.register(ExecutionActivityKind::Other).unwrap());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let request = LoadPromptResource {
        id: "request".into(),
        target,
        resource_id: ResourceID::parse("prompt:local/sample").unwrap(),
        alias: alias.into(),
        import_kind: None,
        cancel: activity.cancel.clone(),
        reply: Arc::new(Mutex::new(Some(tx))),
        activity: Some(activity),
    };
    (request, rx, scope)
}
fn wait(
    app: &mut App,
    mut rx: tokio::sync::oneshot::Receiver<Result<(), ResourceLoadError>>,
) -> Result<(), ResourceLoadError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        match rx.try_recv() {
            Ok(result) => return result,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                panic!("reply channel closed")
            }
            Err(_) => {}
        }
        assert!(Instant::now() < deadline, "resource read timed out");
        std::thread::yield_now();
    }
}
fn load(app: &mut App, target: Entity, alias: &str) -> Result<(), ResourceLoadError> {
    let (request, rx, scope) = request(target, alias);
    app.world().send_event(request);
    let result = wait(app, rx);
    assert_eq!(scope.activity_count(), 0);
    result
}
#[test]
fn loads_and_reload_updates_alias_without_adding_keys() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    load(&mut app, target, "first").unwrap();
    load(&mut app, target, "second").unwrap();
    let set = app
        .world()
        .get_component::<ResourceSet<PromptResource>>(target)
        .unwrap();
    assert_eq!(set.len(), 1);
    assert_eq!(set.iter().next().unwrap().1.alias, "second");
}
#[test]
fn failed_reload_preserves_old_entry_and_alias() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    load(&mut app, target, "old").unwrap();
    fs::write(
        root.join("prompts/local/sample/latest/prompt.toml"),
        "invalid definition",
    )
    .unwrap();
    assert!(load(&mut app, target, "new").is_err());
    let set = app
        .world()
        .get_component::<ResourceSet<PromptResource>>(target)
        .unwrap();
    assert_eq!(set.len(), 1);
    assert_eq!(set.iter().next().unwrap().1.alias, "old");
}
#[test]
fn cancelled_request_does_not_mount() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    let (request, rx, scope) = request(target, "local");
    request.cancel.cancel();
    app.world().send_event(request);
    assert_eq!(wait(&mut app, rx).unwrap_err().kind, Kind::Cancelled);
    assert!(app
        .world()
        .get_component::<ResourceSet<PromptResource>>(target)
        .is_none());
    assert_eq!(scope.activity_count(), 0);
}
#[test]
fn cancellation_after_preparation_does_not_mount() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    let (request, rx, scope) = request(target, "local");
    request.activity.as_ref().unwrap().mark_started().unwrap();
    request.cancel.cancel();
    app.world()
        .send_event(Ok::<_, PromptResourceTaskError>(PromptResourceReadOutput {
            request,
            result: Ok(PromptResource {
                alias: "local".into(),
                role: "user".into(),
                content: "prepared".into(),
            }),
        }));
    assert_eq!(wait(&mut app, rx).unwrap_err().kind, Kind::Cancelled);
    assert!(app
        .world()
        .get_component::<ResourceSet<PromptResource>>(target)
        .is_none());
    assert_eq!(scope.activity_count(), 0);
}
#[test]
fn late_output_cannot_mount_into_reused_target_index() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    let (request, rx, scope) = request(target, "local");
    request.activity.as_ref().unwrap().mark_started().unwrap();
    app.world_mut().despawn(target);
    let reused = app.world_mut().spawn();
    assert_eq!(target.index(), reused.index());
    assert_ne!(target.generation(), reused.generation());
    app.world()
        .send_event(Ok::<_, PromptResourceTaskError>(PromptResourceReadOutput {
            request,
            result: Ok(PromptResource {
                alias: "local".into(),
                role: "user".into(),
                content: "prepared".into(),
            }),
        }));
    assert_eq!(wait(&mut app, rx).unwrap_err().kind, Kind::TargetNotAlive);
    assert!(app
        .world()
        .get_component::<ResourceSet<PromptResource>>(reused)
        .is_none());
    assert_eq!(scope.activity_count(), 0);
}
#[test]
fn target_entities_have_independent_aliases() {
    let root = fixture();
    let mut app = app(&root);
    let first = app.world_mut().spawn();
    let second = app.world_mut().spawn();
    load(&mut app, first, "first").unwrap();
    load(&mut app, second, "second").unwrap();
    for (target, alias) in [(first, "first"), (second, "second")] {
        let set = app
            .world()
            .get_component::<ResourceSet<PromptResource>>(target)
            .unwrap();
        assert_eq!(set.iter().next().unwrap().1.alias, alias);
    }
}
#[test]
fn rejects_relative_root_and_parent_traversal() {
    assert!(PromptResourcePlugin::open("relative").is_err());
    assert!(PromptResourcePlugin::open("/tmp/../escape").is_err());
}
#[cfg(unix)]
#[test]
fn necessary_file_symlink_is_rejected() {
    let root = fixture();
    let directory = root.join("prompts/local/sample/latest");
    let path = directory.join("prompt.toml");
    let saved = directory.join("original");
    fs::rename(&path, &saved).unwrap();
    std::os::unix::fs::symlink(&saved, &path).unwrap();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    assert_eq!(
        load(&mut app, target, "alias").unwrap_err().kind,
        Kind::SymlinkNotAllowed
    );
}

#[test]
fn task_failure_releases_activity_and_preserves_resources() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    load(&mut app, target, "old").unwrap();
    let (request, rx, scope) = request(target, "new");
    request.activity.as_ref().unwrap().mark_started().unwrap();
    let task = PromptResourceTaskError {
        request: Some(request),
        message: "injected supervision failure".into(),
    };
    app.world()
        .send_event(Err::<PromptResourceReadOutput, _>(task));
    assert_eq!(wait(&mut app, rx).unwrap_err().kind, Kind::TaskFailed);
    let set = app
        .world()
        .get_component::<ResourceSet<PromptResource>>(target)
        .unwrap();
    assert_eq!(set.iter().next().unwrap().1.alias, "old");
    assert_eq!(scope.activity_count(), 0);
}
#[test]
fn repeated_result_does_not_mount_after_reply_was_taken() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    let (request, rx, scope) = request(target, "first");
    let duplicate = request.clone();
    app.world().send_event(request);
    wait(&mut app, rx).unwrap();
    assert_eq!(scope.activity_count(), 0);
    let existing = app
        .world()
        .get_component::<ResourceSet<PromptResource>>(target)
        .unwrap()
        .iter()
        .next()
        .unwrap()
        .1
        .clone();
    let mut updated = existing;
    updated.alias = "should not replace".into();
    app.world()
        .send_event(Ok::<_, PromptResourceTaskError>(PromptResourceReadOutput {
            request: duplicate,
            result: Ok(updated),
        }));
    app.tick();
    assert_eq!(
        app.world()
            .get_component::<ResourceSet<PromptResource>>(target)
            .unwrap()
            .iter()
            .next()
            .unwrap()
            .1
            .alias,
        "first"
    );
}
#[test]
fn byte_limits_reject_load_without_mount() {
    let root = fixture();
    let mut app = App::new();
    let mut limits = prompt_resource_plugin::PromptResourceLoadLimits::default();
    limits.max_definition_bytes = 1;
    limits.max_content_bytes = 1;
    limits.max_policy_bytes = 1;
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
        .add_plugin(
            PromptResourcePlugin::open(&root)
                .unwrap()
                .with_limits(limits),
        );
    let target = app.world_mut().spawn();
    assert_eq!(
        load(&mut app, target, "alias").unwrap_err().kind,
        Kind::LimitExceeded
    );
    assert!(app
        .world()
        .get_component::<ResourceSet<PromptResource>>(target)
        .is_none());
}

#[test]
fn reload_preserves_other_resource_keys() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    load(&mut app, target, "old").unwrap();
    let second_id = ResourceID::parse("prompt:local/second").unwrap();
    let set = app
        .world_mut()
        .get_component_mut::<ResourceSet<PromptResource>>(target)
        .unwrap();
    let mut second = set.iter().next().unwrap().1.clone();
    second.alias = "untouched".into();
    set.insert(second_id.clone(), second);
    load(&mut app, target, "updated").unwrap();
    let set = app
        .world()
        .get_component::<ResourceSet<PromptResource>>(target)
        .unwrap();
    assert_eq!(set.len(), 2);
    assert_eq!(set.get(&second_id).unwrap().alias, "untouched");
}
#[test]
fn invalid_utf8_cannot_mount() {
    let root = fixture();
    fs::write(root.join("prompts/local/sample/latest/content.md"), [255u8]).unwrap();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    assert_eq!(
        load(&mut app, target, "alias").unwrap_err().kind,
        Kind::DecodeFailed
    );
    assert!(app
        .world()
        .get_component::<ResourceSet<PromptResource>>(target)
        .is_none());
}

#[test]
fn typed_prompt_import_faithfully_preserves_system_user_and_assistant_roles() {
    use margatroid_types::message_types::AgentMessage;
    let root = fixture();
    let mut app = app(&root);
    let (target, id) = support::agent(&mut app, &["prompt:local/sample"]);
    for role in ["system", "user", "assistant"] {
        fs::write(
            root.join("prompts/local/sample/latest/prompt.toml"),
            format!("role=\"{role}\"\n"),
        )
        .unwrap();
        let (mut request, rx, scope) = request(target, role);
        request.import_kind = Some(InnerType::Message);
        app.world().send_event(request);
        wait(&mut app, rx).unwrap();
        assert_eq!(scope.activity_count(), 0);
        let BlockInner::Message(messages) = app.world().agent_get_import(&id, role).unwrap() else {
            panic!("not messages")
        };
        assert_eq!(
            messages,
            vec![match role {
                "system" => AgentMessage::System {
                    content: "prompt body".into()
                },
                "user" => AgentMessage::User {
                    content: "prompt body".into()
                },
                _ => AgentMessage::Assistant {
                    reasoning: None,
                    content: Some("prompt body".into()),
                    tool_calls: Vec::new(),
                    usage: None
                },
            }]
        );
    }
    assert!(app
        .world()
        .get_component::<Agent>(target)
        .unwrap()
        .sandbox
        .is_none());
}
