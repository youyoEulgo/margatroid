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
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
    }

    match args[1].as_str() {
        "serve" => cmd_serve().await,
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
                    cmd_compose_up(&args[3]).await
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

async fn cmd_serve() -> Result<()> {
    server::serve().await
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
    let compose = compose::load(path)?;
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

async fn cmd_compose_up(path: &str) -> Result<()> {
    use std::sync::Arc;

    let compose = compose::load(path)?;
    let mgr = assets::Manager::bootstrap()?;
    let lib = assets::MemberLibrary::load()?;

    let api_key = mgr
        .app_config()
        .ai
        .providers
        .iter()
        .find(|p| p.name == "OpenRouter" && p.enabled)
        .map(|p| p.api_key.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no enabled OpenRouter provider, check ~/.margatroid/margatroid.toml"
            )
        })?;

    let provider = Arc::new(providers::OpenRouterProvider::new(api_key));
    let sandbox = Arc::new(tokio::sync::RwLock::new(sandbox::SandboxManager::new()));
    let temp_board = Arc::new(runtime::DelegationBoard::new(Arc::new(
        runtime::SqliteMemory::open(":memory:").unwrap(),
    )));

    let mut entries = Vec::new();
    for agent_ref in &compose.agents {
        let def = lib
            .get(&agent_ref.id)
            .ok_or_else(|| anyhow::anyhow!("member '{}' not found in library", agent_ref.id))?;

        if def.identity == types::Identity::User {
            continue;
        }

        let is_manager = def.identity == types::Identity::Manager;
        let member = Arc::new(runtime::Member::new(
            &def.id,
            def.identity.clone(),
            &def.model,
            provider.clone(),
            sandbox.clone(),
            temp_board.clone(),
        ));

        entries.push(runtime::AgentEntry {
            agent: member,
            system_prompt: def.soul.clone(),
            tools: if is_manager {
                runtime::manager_tools()
            } else {
                runtime::base_tools()
            },
        });
    }

    let workspace = Arc::new(runtime::Workspace::start(&compose, entries, provider).await?);

    tracing::info!(
        "Workspace '{}' started, {} members",
        compose.workspace.name,
        compose.agents.len()
    );

    // HTTP endpoint
    let ws = workspace.clone();
    tokio::spawn(async move {
        use axum::{Json, Router, extract::State, routing::post};

        #[derive(serde::Deserialize)]
        struct ChatMsg {
            brief: String,
            #[serde(default)]
            detail: String,
        }

        async fn chat(
            State(ws): State<Arc<runtime::Workspace>>,
            Json(payload): Json<ChatMsg>,
        ) -> Json<serde_json::Value> {
            let mgr_id = "manager";
            match ws
                .send_user_message("user", mgr_id, &payload.brief, &payload.detail)
                .await
            {
                Ok(task_id) => Json(serde_json::json!({"ok": true, "task_id": task_id})),
                Err(e) => Json(serde_json::json!({"ok": false, "error": e.to_string()})),
            }
        }

        async fn status(
            State(ws): State<Arc<runtime::Workspace>>,
        ) -> Json<serde_json::Value> {
            let s = ws.board.status().await;
            Json(serde_json::json!({
                "publish": s.publish_count,
                "exec": s.exec_count,
                "returned": s.returned_count,
            }))
        }

        async fn tasks(
            State(ws): State<Arc<runtime::Workspace>>,
        ) -> Json<serde_json::Value> {
            Json(serde_json::json!(ws.board.status().await))
        }

        let app = Router::new()
            .route("/chat", post(chat))
            .route("/status", axum::routing::get(status))
            .route("/tasks", axum::routing::get(tasks))
            .with_state(ws);

        let _ = axum::serve(
            tokio::net::TcpListener::bind("127.0.0.1:3456")
                .await
                .unwrap(),
            app,
        )
        .await;
    });

    tracing::info!("HTTP endpoint: http://127.0.0.1:3456");

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let status = workspace.board.status().await;
        tracing::info!(
            "board: publish={} exec={} returned={}",
            status.publish_count,
            status.exec_count,
            status.returned_count
        );
    }
}
