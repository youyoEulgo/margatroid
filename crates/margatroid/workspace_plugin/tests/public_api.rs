use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_image_loader_plugin::AgentImageLoaderPlugin;
use agent_plugin::{AgentContext, AgentPlugin, AgentWorkspaceId};
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use inference_plugin::{AgentInferenceSnapshot, InferencePlugin, WorkspaceModelRoutes};
use margatroid_types::{
    AgentImageReference, ResourceName, ResourceRef, WorkspaceAgentDefinition, WorkspaceDefinition,
};
use memory_plugin::{AgentMemory, MemoryPlugin};
use tempfile::tempdir;
use tool_plugin::{AgentToolEnvironment, ToolPlugin};
use workspace_plugin::{
    ReloadWorkspaceResult, StartWorkspaceResult, StopWorkspaceResult, WorkspaceAgents,
    WorkspaceIdentity, WorkspacePlugin, WorldWorkspaceExt,
};

fn unique_directory(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "margatroid-workspace-{prefix}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn write_image(library: &Path, model: &str) {
    let image = library.join("local/coder/latest");
    fs::create_dir_all(image.join("skills/local/review")).unwrap();
    fs::create_dir_all(image.join("workflows/local/check")).unwrap();
    fs::write(
        image.join("agent.toml"),
        format!("schema_version = 1\n[inference]\nmodel = \"{model}\"\n"),
    )
    .unwrap();
    fs::write(image.join("SOUL.md"), "You are a test agent.\n").unwrap();
}

fn write_routes(path: &Path) {
    fs::write(
        path,
        r#"[[models]]
id = "test-model"
model = "test-model"
base_url = "https://example.test/v1"
api_key = "secret"
api_type = "openai"
"#,
    )
    .unwrap();
}

fn definition(project_root: &Path) -> WorkspaceDefinition {
    WorkspaceDefinition {
        name: "demo".into(),
        project_root: project_root.to_path_buf(),
        manager: "manager".into(),
        agents: vec![WorkspaceAgentDefinition {
            name: "manager".into(),
            image: AgentImageReference::new("local/coder").unwrap(),
            resources: vec![ResourceRef::new(
                "workflow",
                ResourceName::new("local/extra").unwrap(),
            )
            .unwrap()],
            disable_resources: Vec::new(),
            memory_path: None,
        }],
    }
}

fn app(library: &Path, routes: &Path) -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(AgentImageLoaderPlugin::open(library).unwrap())
        .add_plugin(InferencePlugin::default().with_config_path(routes))
        .add_plugin(ToolPlugin::default())
        .add_plugin(MemoryPlugin::default())
        .add_plugin(AgentPlugin::default())
        .add_plugin(WorkspacePlugin::open(library).unwrap());
    app
}

fn wait_start(
    app: &mut App,
    id: &str,
) -> Result<core_plugin::Entity, workspace_plugin::WorkspaceError> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        app.tick();
        if let Some(result) = app
            .world()
            .event_reader::<StartWorkspaceResult>()
            .into_iter()
            .find(|result| result.id == id)
        {
            return result.result.clone();
        }
        assert!(Instant::now() < deadline, "workspace start timed out");
        std::thread::yield_now();
    }
}

#[test]
fn documented_public_api_starts_queries_reloads_and_stops_workspace() {
    let library = unique_directory("success-library");
    let project = tempdir().unwrap();
    let routes = project.path().join("models.toml");
    fs::create_dir_all(&library).unwrap();
    write_image(&library, "test-model");
    write_routes(&routes);
    let project_routes = project.path().join(".margatroid/models.toml");
    fs::create_dir_all(project_routes.parent().unwrap()).unwrap();
    write_routes(&project_routes);

    let mut app = app(&library, &routes);
    let definition = definition(project.path());
    app.world().start_workspace("start-1", definition.clone());
    let workspace = wait_start(&mut app, "start-1").unwrap();

    assert_eq!(
        app.world()
            .get_component::<WorkspaceIdentity>(workspace)
            .unwrap()
            .name(),
        "demo"
    );
    assert_eq!(
        app.world().workspace(project.path(), "demo"),
        Some(workspace)
    );
    let manager = app.world().workspace_manager(workspace).unwrap();
    assert_eq!(
        app.world().workspace_agent(workspace, "manager"),
        Some(manager)
    );
    assert_eq!(app.world().workspace_of(manager), Some(workspace));
    assert_eq!(
        app.world()
            .get_component::<AgentWorkspaceId>(manager)
            .unwrap()
            .workspace_id(),
        workspace
    );
    assert_eq!(
        app.world()
            .get_component::<AgentContext>(manager)
            .unwrap()
            .system_prompt(),
        "You are a test agent.\n"
    );
    assert!(app
        .world()
        .get_component::<AgentInferenceSnapshot>(manager)
        .is_some());
    assert!(app
        .world()
        .get_component::<WorkspaceModelRoutes>(workspace)
        .is_some());
    assert!(app
        .world()
        .get_component::<AgentToolEnvironment>(manager)
        .is_some());
    assert!(app.world().get_component::<AgentMemory>(manager).is_some());
    assert_eq!(
        app.world()
            .get_component::<WorkspaceAgents>(workspace)
            .unwrap()
            .iter()
            .count(),
        1
    );

    app.world()
        .reload_workspace("reload-1", workspace, definition);
    let reloaded = wait_reload(&mut app, "reload-1", workspace).unwrap();
    assert_ne!(workspace, reloaded);
    assert!(!app.world().is_alive(workspace));
    assert_eq!(
        app.world().workspace_manager(reloaded),
        Some(app.world().workspace_agent(reloaded, "manager").unwrap())
    );

    app.world().stop_workspace("stop-1", reloaded);
    wait_stop(&mut app, "stop-1", reloaded).unwrap();
    assert!(!app.world().is_alive(reloaded));
    assert!(app.world().workspace(project.path(), "demo").is_none());

    let _ = fs::remove_dir_all(library);
}

fn wait_reload(
    app: &mut App,
    id: &str,
    previous: core_plugin::Entity,
) -> Result<core_plugin::Entity, workspace_plugin::WorkspaceError> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        app.tick();
        if let Some(result) = app
            .world()
            .event_reader::<ReloadWorkspaceResult>()
            .into_iter()
            .find(|result| result.id == id && result.previous == previous)
        {
            return result.result.clone();
        }
        assert!(Instant::now() < deadline, "workspace reload timed out");
        std::thread::yield_now();
    }
}

fn wait_stop(
    app: &mut App,
    id: &str,
    workspace: core_plugin::Entity,
) -> Result<(), workspace_plugin::WorkspaceError> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        app.tick();
        if let Some(result) = app
            .world()
            .event_reader::<StopWorkspaceResult>()
            .into_iter()
            .find(|result| result.id == id && result.workspace == workspace)
        {
            return result.result.clone();
        }
        assert!(Instant::now() < deadline, "workspace stop timed out");
        std::thread::yield_now();
    }
}

#[test]
fn invalid_model_route_fails_before_agent_creation() {
    let library = unique_directory("failure-library");
    let project = tempdir().unwrap();
    let routes = project.path().join("models.toml");
    fs::create_dir_all(&library).unwrap();
    write_image(&library, "missing-model");
    write_routes(&routes);

    let mut app = app(&library, &routes);
    app.world()
        .start_workspace("start-failure", definition(project.path()));
    let error = wait_start(&mut app, "start-failure").unwrap_err();

    assert_eq!(
        error.kind(),
        workspace_plugin::WorkspaceErrorKind::InferenceSetupFailed
    );
    assert_eq!(app.world().entity_count(), 1);
    let _ = fs::remove_dir_all(library);
}
