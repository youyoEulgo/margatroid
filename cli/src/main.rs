//! Margatroid CLI
//!
//! 用法:
//!   margatroid serve                         启动 HTTP 服务
//!   margatroid compose validate <file>       校验 compose 文件
//!   margatroid compose roster <file>         打印团队成员目录
//!   margatroid compose load <file>           解析并以 JSON 输出
//!   margatroid workspace create <file>       从 compose 文件创建 Workspace
//!   margatroid workspace list                列出所有 Workspace

use anyhow::Result;

fn usage() -> ! {
    eprintln!("用法:");
    eprintln!("  margatroid serve");
    eprintln!("  margatroid compose validate <file>");
    eprintln!("  margatroid compose roster <file>");
    eprintln!("  margatroid compose load <file>");
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

// ── serve ────────────────────────────────────────────────────

async fn cmd_serve() -> Result<()> {
    server::serve().await
}

// ── compose subcommands ──────────────────────────────────────

fn cmd_compose_validate(path: &str) -> Result<()> {
    let compose = compose::load(path)?;
    println!("✓ compose 文件有效");
    println!(
        "  workspace: {} (v{})",
        compose.workspace.name, compose.workspace.version
    );
    println!("  agents: {}", compose.agents.len());
    for agent in &compose.agents {
        println!("    - {} ({})", agent.id, agent.model);
    }
    Ok(())
}

fn cmd_compose_roster(path: &str) -> Result<()> {
    let compose = compose::load(path)?;
    let roster = compose::roster::generate(&compose);
    print!("{}", roster);
    Ok(())
}

fn cmd_compose_load(path: &str) -> Result<()> {
    let compose = compose::load(path)?;
    let json = serde_json::to_string_pretty(&compose)?;
    println!("{}", json);
    Ok(())
}

// ── workspace subcommands ────────────────────────────────────

fn cmd_workspace_create(path: &str) -> Result<()> {
    let compose = compose::load(path)?;
    let mut mgr = assets::Manager::bootstrap()?;
    mgr.create_workspace(&compose)?;
    println!("✓ workspace '{}' 创建成功", compose.workspace.name);
    Ok(())
}

fn cmd_workspace_list() -> Result<()> {
    let mgr = assets::Manager::bootstrap()?;
    let list = mgr.list_workspaces();
    if list.is_empty() {
        println!("(暂无 workspace)");
    } else {
        for name in list {
            println!("  {}", name);
        }
    }
    Ok(())
}
