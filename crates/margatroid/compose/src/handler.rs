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
    let tool_limits = resolve_tool_limits(config.config().tools())
        .map_err(|e| ComposeError::new("config", &config_path, e.to_string()))?;
    let inference = inference_plugin::InferencePlugin::open(&models_path)
        .map_err(|e| ComposeError::new("inference", &models_path, e.to_string()))?;
    let workspace = workspace_plugin::WorkspacePlugin {
        root: workspace_path,
    };
    Ok(ApplicationParts {
        options: ApplicationOptions {
            data_root,
            runner_path,
        },
        config,
        tool_limits,
        inference,
        workspace,
    })
}
pub(crate) fn install(app: &mut App, parts: ApplicationParts) -> Result<(), ComposeError> {
    let ApplicationParts {
        options,
        config,
        tool_limits,
        inference,
        workspace,
    } = parts;
    app.world_mut()
        .insert_resource(address::ResourceRoots::new(&options.data_root));
    let clients = client_core::Clients::default();
    let bindings = dto_plugin::sync_bindings(clients.clone());
    let agent_sync = bindings.agent.clone();
    let task_sync = bindings.tasks.clone();
    app.world_mut().insert_resource(bindings.workspace);
    app.world_mut().insert_resource(bindings.workspace_binding);
    app.world_mut().insert_resource(bindings.agent);
    app.world_mut().insert_resource(bindings.models);
    app.world_mut().insert_resource(bindings.sandboxes);
    app.world_mut()
        .insert_resource(dto_plugin::TaskSyncInstaller(task_sync.clone()));
    app.world_mut().insert_resource(clients);
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
        .add_plugin(log);
    app.add_plugin(AxumPlugin::bind(bind))
        .add_plugin(config)
        .add_plugin(client_plugin::ClientPlugin::new())
        .add_plugin(tool_resource_plugin::ToolResourcePlugin::new())
        .add_plugin(prompt_resource_plugin::PromptResourcePlugin::new())
        .add_plugin(sandbox_resource_plugin::SandboxResourcePlugin::new())
        .add_plugin(inference)
        .add_plugin(
            tool_plugin::ToolPlugin::new()
                .with_task_observer(task_sync)
                .with_runner_path(options.runner_path)
                .with_limits(tool_limits),
        )
        .add_plugin(storage_plugin::StoragePlugin::new())
        .add_plugin(lua_runtime_plugin::LuaRuntimePlugin::new())
        .add_plugin(mcl_plugin::MclPlugin::new().with_import_config(imports))
        .add_plugin(agent_plugin::AgentPlugin::new(agent_sync))
        .add_plugin(workspace)
        .add_plugin(dto_plugin::DtoPlugin::new())
        .add_plugin(ui_plugin::UiPlugin::default());
    Ok(())
}

pub(crate) async fn shutdown_application(app: &mut App) -> Result<(), ComposeError> {
    use workspace_core::{Workspace, WorkspaceState, WorkspaceUpdateContext, WorkspaceUpdateKind};
    let server = app
        .world()
        .get_resource::<axum_plugin::AxumHandle>()
        .ok_or_else(|| ComposeError::new("axum", "", "Axum service missing"))?
        .clone();
    server.request_shutdown();
    type DrainReply =
        tokio::sync::oneshot::Receiver<Result<workspace_core::WorkspaceDatabase, String>>;
    let mut pending: Vec<(core_plugin::Entity, std::path::PathBuf, DrainReply)> = Vec::new();
    loop {
        app.tick();
        for entity in app.world().query_with::<Workspace>().result() {
            if pending.iter().any(|(current, _, _)| *current == entity) {
                continue;
            }
            let workspace = app.world().get_component::<Workspace>(entity).unwrap();
            if workspace.state == WorkspaceState::Deleting
                && !app
                    .world()
                    .contains_component::<crate::types::ApplicationDraining>(entity)
            {
                continue;
            }
            let database = workspace.database.clone();
            database.close_admission();
            let tasks = app
                .world()
                .get_component::<tool_core::ToolTasks>(entity)
                .cloned();
            if let Some(tasks) = &tasks {
                tasks.close_admission();
                for (_, task) in tasks.list() {
                    task.cancel();
                }
            }
            app.world_mut()
                .get_component_mut::<Workspace>(entity)
                .unwrap()
                .set_state(WorkspaceState::Deleting);
            app.world_mut()
                .insert_component(entity, crate::types::ApplicationDraining);
            let (tx, rx) = tokio::sync::oneshot::channel();
            let path = database.path().to_path_buf();
            app.world()
                .get_resource::<async_runtime_plugin::AsyncRuntimeHandle>()
                .unwrap()
                .spawn(async move {
                    let mut failure = None;
                    if let Some(tasks) = tasks {
                        for (_, task) in tasks.list() {
                            if let Err(e) = task.finish().await {
                                failure = Some(e.to_string());
                            }
                        }
                    }
                    if failure.is_none() {
                        database.wait_idle().await;
                    }
                    let _ = tx.send(failure.map_or(Ok(database), Err));
                });
            pending.push((entity, path, rx));
        }
        let mut next = Vec::new();
        for (entity, path, mut receiver) in pending {
            match receiver.try_recv() {
                Ok(Ok(database)) => {
                    if let Some(context) = app
                        .world()
                        .get_resource::<WorkspaceUpdateContext>()
                        .cloned()
                    {
                        context.notify(app.world_mut(), entity, WorkspaceUpdateKind::Closing);
                    }
                    database
                        .close()
                        .map_err(|e| ComposeError::new("workspace", &path, e.to_string()))?;
                    app.world_mut().despawn(entity);
                    if let Some(context) = app
                        .world()
                        .get_resource::<WorkspaceUpdateContext>()
                        .cloned()
                    {
                        context.notify(app.world_mut(), entity, WorkspaceUpdateKind::Removed);
                    }
                }
                Ok(Err(e)) => return Err(ComposeError::new("workspace", path, e)),
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    return Err(ComposeError::new("workspace", path, "shutdown reply lost"));
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    next.push((entity, path, receiver))
                }
            }
        }
        pending = next;
        let producers_idle = app
            .world()
            .get_resource::<workspace_core::WorkspacePendingWork>()
            .is_none_or(|work| work.is_idle());
        if pending.is_empty()
            && app.world().query_with::<Workspace>().result().is_empty()
            && producers_idle
            && server.is_stopped()
        {
            return server
                .result()
                .unwrap_or(Ok(()))
                .map_err(|e| ComposeError::new("axum", "", e.to_string()));
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
}

fn resolve_tool_limits(
    config: &config_core::FileToolConfig,
) -> Result<tool_core::ToolExecutionLimits, config_core::ConfigError> {
    config.validate()?;
    let defaults = tool_core::ToolExecutionLimits::default();
    let limits = tool_core::ToolExecutionLimits {
        lua_vm_memory_bytes: config
            .lua_vm_memory_limit
            .unwrap_or(defaults.lua_vm_memory_bytes),
        cache_bytes: config
            .task_output_buffer_window
            .unwrap_or(defaults.cache_bytes),
    };
    limits
        .validate()
        .map_err(|error| config_core::ConfigError::InvalidToolLimit {
            field: "tools",
            reason: error.to_string(),
        })?;
    Ok(limits)
}

#[cfg(test)]
mod tool_limit_tests {
    use super::*;

    #[test]
    fn omitted_tool_limits_use_runtime_defaults() {
        assert_eq!(
            resolve_tool_limits(&config_core::FileToolConfig::default()).unwrap(),
            tool_core::ToolExecutionLimits::default()
        );
    }

    #[test]
    fn partial_tool_configuration_overrides_only_the_selected_limit() {
        let defaults = tool_core::ToolExecutionLimits::default();
        for (config, expected) in [
            (
                config_core::FileToolConfig {
                    lua_vm_memory_limit: Some(19),
                    ..Default::default()
                },
                tool_core::ToolExecutionLimits {
                    lua_vm_memory_bytes: 19,
                    ..defaults
                },
            ),
            (
                config_core::FileToolConfig {
                    task_output_buffer_window: Some(23),
                    ..Default::default()
                },
                tool_core::ToolExecutionLimits {
                    cache_bytes: 23,
                    ..defaults
                },
            ),
        ] {
            assert_eq!(resolve_tool_limits(&config).unwrap(), expected);
        }
    }

    #[test]
    fn file_budget_overrides_reach_effective_tool_limits() {
        let root = tempfile::tempdir().unwrap();
        let source = "[server]\nbind = '127.0.0.1:0'\n[tools]\nlua_vm_memory_limit = '106B'\ntask_output_buffer_window = '108B'\n";
        let path = root.path().join("config.toml");
        std::fs::write(&path, source).unwrap();
        let config = config_plugin::ConfigPlugin::open(path).unwrap();
        let effective = resolve_tool_limits(config.config().tools()).unwrap();
        assert_eq!(effective.lua_vm_memory_bytes, 106);
        assert_eq!(effective.cache_bytes, 108);
    }

    #[test]
    fn zero_budget_is_configuration_error_before_app_installation() {
        for field in ["lua_vm_memory_limit", "task_output_buffer_window"] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("config.toml");
            std::fs::write(
                &path,
                format!("[server]\nbind = '127.0.0.1:0'\n[tools]\n{field} = '0'\n"),
            )
            .unwrap();
            let error = crate::build_app(ApplicationOptions {
                data_root: root.path().to_owned(),
                runner_path: std::env::current_exe().unwrap(),
            })
            .err()
            .expect("must reject invalid budget before creating app");
            assert_eq!(error.domain, "config");
            assert_eq!(error.path, path);
        }
    }

    #[test]
    fn programmatic_zero_budget_reports_the_configuration_field() {
        for (config, field) in [
            (
                config_core::FileToolConfig {
                    lua_vm_memory_limit: Some(0),
                    ..Default::default()
                },
                "tools.lua_vm_memory_limit",
            ),
            (
                config_core::FileToolConfig {
                    task_output_buffer_window: Some(0),
                    ..Default::default()
                },
                "tools.task_output_buffer_window",
            ),
        ] {
            assert!(
                matches!(resolve_tool_limits(&config), Err(config_core::ConfigError::InvalidToolLimit { field: actual, .. }) if actual == field)
            );
        }
    }
}
