use crate::{ApplicationOptions, ComposeError, types::ApplicationParts};
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use axum_plugin::AxumPlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use config_core::LogLevel as ConfigLogLevel;
use core_plugin::App;
use log_plugin::{LogLevel, LogPlugin};

pub(crate) fn prepare(options: &ApplicationOptions) -> Result<ApplicationParts, ComposeError> {
    let data_root = std::fs::canonicalize(&options.data_root)
        .map_err(|e| ComposeError::new("root", &options.data_root, e.to_string()))?;
    let runner_path = std::fs::canonicalize(&options.runner_path)
        .map_err(|e| ComposeError::new("runner", &options.runner_path, e.to_string()))?;
    if !runner_path.is_file() {
        return Err(ComposeError::new(
            "runner",
            runner_path,
            "runner must be an executable file",
        ));
    }
    let config_path = data_root.join("config.toml");
    let models_path = data_root.join("models.toml");
    let workspace_path = data_root.join("workspaces");
    let config = config_plugin::ConfigPlugin::open(&config_path)
        .map_err(|e| ComposeError::new("config", &config_path, e.to_string()))?;
    let inference = inference_plugin::InferencePlugin::open(&models_path)
        .map_err(|e| ComposeError::new("inference", &models_path, e.to_string()))?;
    let workspace = workspace_plugin::WorkspacePlugin::open(&workspace_path)
        .map_err(|e| ComposeError::new("workspace", &workspace_path, e.to_string()))?;
    Ok(ApplicationParts {
        options: ApplicationOptions {
            data_root,
            runner_path,
        },
        config,
        inference,
        workspace,
    })
}
pub(crate) fn install(app: &mut App, parts: ApplicationParts) -> Result<(), ComposeError> {
    let ApplicationParts {
        options,
        config,
        inference,
        workspace,
    } = parts;
    let bind = config.config().server_bind();
    let level = match config.config().log_level() {
        ConfigLogLevel::Off => LogLevel::Off,
        ConfigLogLevel::Error => LogLevel::Error,
        ConfigLogLevel::Warn => LogLevel::Warn,
        ConfigLogLevel::Info => LogLevel::Info,
        ConfigLogLevel::Debug => LogLevel::Debug,
        ConfigLogLevel::Trace => LogLevel::Trace,
    };
    let mut log = LogPlugin::default().with_stream(256).with_level(level);
    if let Some(filter) = config.config().log_filter() {
        log = log.with_filter(filter);
    }
    let imports = mcl_plugin::MclImportConfig {
        prompt_root: options.data_root.join("prompts"),
        tool_root: options.data_root.join("tools"),
        sandbox_root: options.data_root.join("sandboxes"),
    };
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(log)
        .add_plugin(AxumPlugin::bind(bind))
        .add_plugin(config)
        .add_plugin(client_plugin::ClientPlugin::new())
        .add_plugin(tool_resource_plugin::ToolResourcePlugin::new())
        .add_plugin(prompt_resource_plugin::PromptResourcePlugin::new())
        .add_plugin(sandbox_resource_plugin::SandboxResourcePlugin::new())
        .add_plugin(inference)
        .add_plugin(tool_plugin::ToolPlugin::new().with_runner_path(options.runner_path))
        .add_plugin(storage_plugin::StoragePlugin::new())
        .add_plugin(lua_runtime_plugin::LuaRuntimePlugin::new())
        .add_plugin(mcl_plugin::MclPlugin::new().with_import_config(imports))
        .add_plugin(agent_plugin::AgentPlugin::new().with_resource_roots(options.data_root))
        .add_plugin(workspace)
        .add_plugin(dto_plugin::DtoPlugin::new())
        .add_plugin(ui_plugin::UiPlugin::default());
    Ok(())
}

pub(crate) async fn shutdown_application(app: &mut App) -> Result<(), ComposeError> {
    use app_runtime_plugin::WorldEventExt;
    use std::collections::BTreeMap;
    use workspace_core::{StopWorkspace, StopWorkspaceResult, Workspace, WorkspaceState};

    let server = app
        .world()
        .get_resource::<axum_plugin::AxumHandle>()
        .ok_or_else(|| ComposeError::new("axum", "", "Axum service is missing"))?
        .clone();
    server.request_shutdown();
    let mut pending: BTreeMap<String, (core_plugin::Entity, std::path::PathBuf)> = BTreeMap::new();
    let mut next = 0u64;
    loop {
        app.tick();
        for response in app.world().event_reader::<StopWorkspaceResult>() {
            if let Some(path) = pending.remove(&response.id)
                && let Err(error) = &response.result
            {
                return Err(ComposeError::new("workspace", path.1, error.to_string()));
            }
        }
        let workspaces = app.world().query_with::<Workspace>().result();
        for entity in &workspaces {
            let workspace = app.world().get_component::<Workspace>(*entity).unwrap();
            if workspace.state == WorkspaceState::Running
                && !pending.values().any(|(current, _)| current == entity)
            {
                next += 1;
                let id = format!("application-shutdown-{next}");
                pending.insert(id.clone(), (*entity, workspace.definition_path.clone()));
                app.world().send_event(StopWorkspace {
                    id,
                    workspace: *entity,
                });
            }
        }
        if workspaces.is_empty() && pending.is_empty() && server.is_stopped() {
            return server
                .result()
                .unwrap_or(Ok(()))
                .map_err(|error| ComposeError::new("axum", "", error.to_string()));
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
}
