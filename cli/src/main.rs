//! Margatroid CLI
//!
//! 用法:
//!   margatroid serve                         启动 HTTP 服务
//!   margatroid compose validate <file>       校验 compose 文件
//!   margatroid compose roster <file>         打印团队成员目录
//!   margatroid compose load <file>           解析并以 JSON 输出
//!   margatroid compose up <file>             启动 Workspace
//!   margatroid workspace create <file>       从 compose 文件创建 Workspace
//!   margatroid workspace list                列出所有 Workspace

use anyhow::Result;
use std::sync::Arc;
use tracing::level_filters::LevelFilter;

use server::state::AppState;

fn config_log_level() -> LevelFilter {
    let path = paths::margatroid_root()
        .unwrap_or_else(|| std::path::PathBuf::from(".margatroid"))
        .join("margatroid.toml");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return LevelFilter::INFO,
    };
    let config: toml::Value = match toml::from_str(&content) {
        Ok(v) => v,
        Err(_) => return LevelFilter::INFO,
    };
    match config
        .get("logging")
        .and_then(|l| l.get("level"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_lowercase())
        .as_deref()
    {
        Some("error") => LevelFilter::ERROR,
        Some("warn") => LevelFilter::WARN,
        Some("debug") => LevelFilter::DEBUG,
        Some("trace") => LevelFilter::TRACE,
        _ => LevelFilter::INFO,
    }
}

fn usage() -> ! {
    eprintln!("用法:");
    eprintln!("  margatroid serve");
    eprintln!("  margatroid compose validate <file>");
    eprintln!("  margatroid compose roster <file>");
    eprintln!("  margatroid compose load <file>");
    eprintln!("  margatroid compose up <file>");
    eprintln!("  margatroid workspace create <file>");
    eprintln!("  margatroid workspace list");
    std::process::exit(1);
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let verbose = args.iter().any(|a| a == "--verbose");
    let args: Vec<_> = args.into_iter().filter(|a| a != "--verbose").collect();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(config_log_level().into())
                .from_env_lossy(),
        )
        .init();

    if args.len() < 2 {
        usage();
    }

    match args[1].as_str() {
        "serve" => cmd_serve(verbose).await,
        "compose" => {
            if args.len() < 4 {
                usage();
            }
            match args[2].as_str() {
                "validate" => cmd_compose_validate(&args[3]),
                "roster" => cmd_compose_roster(&args[3]),
                "load" => cmd_compose_load(&args[3]),
                "up" => {
                    if args.len() < 4 {
                        usage();
                    }
                    cmd_compose_up(&args[3], verbose).await
                }
                _ => usage(),
            }
        }
        "workspace" => {
            if args.len() < 3 {
                usage();
            }
            match args[2].as_str() {
                "create" => {
                    if args.len() < 4 {
                        usage();
                    }
                    cmd_workspace_create(&args[3])
                }
                "list" => cmd_workspace_list(),
                _ => usage(),
            }
        }
        _ => usage(),
    }
}

async fn cmd_serve(verbose: bool) -> Result<()> {
    let _ = verbose;
    let config_mgr = assets::Manager::bootstrap()?;
    let state = AppState::new(config_mgr).await?;
    server::serve(state).await
}

fn cmd_compose_validate(path: &str) -> Result<()> {
    let compose = compose::load(path)?;
    println!("  valid compose file");
    println!(
        "  workspace: {} (v{})",
        compose.workspace.name, compose.workspace.version
    );
    println!("  agents: {}", compose.agents.len());
    for agent in &compose.agents {
        println!("    - {}", agent.id);
    }
    Ok(())
}

fn cmd_compose_roster(path: &str) -> Result<()> {
    let _compose = compose::load(path)?;
    let lib = assets::MemberLibrary::load()?;
    let defs: Vec<_> = lib.all().collect();
    let roster = compose::roster::generate(&defs);
    print!("{}", roster);
    Ok(())
}

fn cmd_compose_load(path: &str) -> Result<()> {
    let compose = compose::load(path)?;
    let json = serde_json::to_string_pretty(&compose)?;
    println!("{}", json);
    Ok(())
}

fn cmd_workspace_create(path: &str) -> Result<()> {
    let compose = compose::load(path)?;
    let mut mgr = assets::Manager::bootstrap()?;
    mgr.create_workspace(&compose)?;
    println!("  workspace '{}' created", compose.workspace.name);
    Ok(())
}

fn cmd_workspace_list() -> Result<()> {
    let mgr = assets::Manager::bootstrap()?;
    let list = mgr.list_workspaces();
    if list.is_empty() {
        println!("(no workspaces)");
    } else {
        for name in list {
            println!("  {}", name);
        }
    }
    Ok(())
}

// ── compose up ────────────────────────────────────────────────

async fn cmd_compose_up(path: &str, verbose: bool) -> Result<()> {
    let mut compose = compose::load(path)?;
    let mgr = assets::Manager::bootstrap()?;
    let lib = assets::MemberLibrary::load()?;

    // 确保 workspace 有系统提示词
    if compose.workspace.system_prompt.is_empty() {
        let prompt = mgr.ensure_system_prompt(&compose.workspace.name)?;
        compose.workspace.system_prompt = prompt;
    }

    let app_config = mgr.app_config();

    let sandbox = Arc::new(tokio::sync::RwLock::new(sandbox::SandboxManager::new()));

    let mut entries = Vec::new();
    for agent_ref in &compose.agents {
        let def = lib
            .get(&agent_ref.id)
            .ok_or_else(|| anyhow::anyhow!("member '{}' not found in library", agent_ref.id))?;

        if def.identity == types::Identity::User {
            continue;
        }

        let provider = providers::resolve(&def.provider, &app_config.ai)?;
        let client = runtime::Client::new(def.model.clone(), provider, verbose);

        let is_manager = def.identity == types::Identity::Manager;
        let member = Arc::new(runtime::Member::new(
            &def.id,
            def.soul.clone(),
            def.identity.clone(),
            client,
            sandbox.clone(),
        ));

        entries.push(runtime::AgentEntry {
            agent: member,
            soul: def.soul.clone(),
            tools: if is_manager {
                runtime::manager_tools()
            } else {
                runtime::base_tools()
            },
            skills: def.skills.clone(),
        });
    }

    let ws_name = compose.workspace.name.clone();

    let state = AppState::new(mgr).await?;
    state.start_workspace(&compose, entries).await?;

    tracing::info!(
        "Workspace '{}' started, {} members",
        ws_name,
        compose.agents.len()
    );

    server::serve(state).await
}
