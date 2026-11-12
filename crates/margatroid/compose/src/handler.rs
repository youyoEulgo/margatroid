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
    let workspace = workspace_plugin::WorkspacePlugin::open(&workspace_path)
        .map_err(|e| ComposeError::new("workspace", &workspace_path, e.to_string()))?;
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
        .add_plugin(
            tool_plugin::ToolPlugin::new()
                .with_runner_path(options.runner_path)
                .with_limits(tool_limits),
        )
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
    use workspace_core::{StopWorkspace, Workspace, WorkspaceState};

    let server = app
        .world()
        .get_resource::<axum_plugin::AxumHandle>()
        .ok_or_else(|| ComposeError::new("axum", "", "Axum service is missing"))?
        .clone();
    server.request_shutdown();
    let mut pending: BTreeMap<
        core_plugin::Entity,
        (
            std::path::PathBuf,
            tokio::sync::oneshot::Receiver<
                Result<workspace_core::WorkspaceOperationOutput, workspace_core::WorkspaceError>,
            >,
        ),
    > = BTreeMap::new();
    let mut next = 0u64;
    loop {
        app.tick();
        let mut finished = Vec::new();
        for (entity, (path, receiver)) in &mut pending {
            match receiver.try_recv() {
                Ok(Ok(_)) => finished.push(*entity),
                Ok(Err(error)) => {
                    return Err(ComposeError::new(
                        "workspace",
                        path.clone(),
                        error.to_string(),
                    ));
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    return Err(ComposeError::new(
                        "workspace",
                        path.clone(),
                        "stop reply lost",
                    ));
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => (),
            }
        }
        for entity in finished {
            pending.remove(&entity);
        }
        let workspaces = app.world().query_with::<Workspace>().result();
        for entity in &workspaces {
            let workspace = app.world().get_component::<Workspace>(*entity).unwrap();
            if workspace.state == WorkspaceState::Running && !pending.contains_key(entity) {
                next += 1;
                let id = format!("application-shutdown-{next}");
                let (sender, receiver) = tokio::sync::oneshot::channel();
                pending.insert(*entity, (workspace.definition_path.clone(), receiver));
                app.world().send_event(StopWorkspace {
                    id,
                    workspace: *entity,
                    reply: std::sync::Arc::new(std::sync::Mutex::new(Some(sender))),
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

fn resolve_tool_limits(
    config: &config_core::FileToolConfig,
) -> Result<tool_core::ToolExecutionLimits, config_core::ConfigError> {
    config.validate()?;
    let defaults = tool_core::ToolExecutionLimits::default();
    let limits = tool_core::ToolExecutionLimits {
        total_memory_bytes: config
            .total_task_memory_limit
            .unwrap_or(defaults.total_memory_bytes),
        memory_bytes: config.task_memory_limit.unwrap_or(defaults.memory_bytes),
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
                    total_task_memory_limit: Some(17),
                    ..Default::default()
                },
                tool_core::ToolExecutionLimits {
                    total_memory_bytes: 17,
                    ..defaults
                },
            ),
            (
                config_core::FileToolConfig {
                    task_memory_limit: Some(19),
                    ..Default::default()
                },
                tool_core::ToolExecutionLimits {
                    memory_bytes: 19,
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
        let source = "[server]\nbind = '127.0.0.1:0'\n[outbound]\nlogs = ['broadcast']\nbackend_state = ['broadcast']\n[tools]\ntotal_task_memory_limit = '100B'\ntask_memory_limit = '106B'\ntask_output_buffer_window = '108B'\n";
        let path = root.path().join("config.toml");
        std::fs::write(&path, source).unwrap();
        let config = config_plugin::ConfigPlugin::open(path).unwrap();
        let effective = resolve_tool_limits(config.config().tools()).unwrap();
        assert_eq!(effective.total_memory_bytes, 100);
        assert_eq!(effective.memory_bytes, 106);
        assert_eq!(effective.cache_bytes, 108);
    }

    #[test]
    fn zero_budget_is_configuration_error_before_app_installation() {
        for field in [
            "total_task_memory_limit",
            "task_memory_limit",
            "task_output_buffer_window",
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("config.toml");
            std::fs::write(&path, format!("[server]\nbind = '127.0.0.1:0'\n[outbound]\nlogs = ['broadcast']\nbackend_state = ['broadcast']\n[tools]\n{field} = '0'\n")).unwrap();
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
                    total_task_memory_limit: Some(0),
                    ..Default::default()
                },
                "tools.total_task_memory_limit",
            ),
            (
                config_core::FileToolConfig {
                    task_memory_limit: Some(0),
                    ..Default::default()
                },
                "tools.task_memory_limit",
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
