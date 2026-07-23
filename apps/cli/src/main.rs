use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use margatroid_compose::{ALTERNATE_COMPOSE_FILE, CompileOptions, Compiler, DEFAULT_COMPOSE_FILE};

const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:3939";
const API_VERSION: &str = margatroid_protocol::API_VERSION;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        print_usage();
        return Ok(());
    };

    match command.as_str() {
        "status" => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(status(&daemon_url())),
        "workspace" => workspace_command(args.collect()),
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        unknown => bail!("unknown command `{unknown}`; run `margatroid help`"),
    }
}

fn workspace_command(args: Vec<String>) -> Result<()> {
    let Some(command) = args.first() else {
        bail!("missing workspace command; run `margatroid help`");
    };
    match command.as_str() {
        "config" => workspace_config(&args[1..]),
        unknown => bail!("unknown workspace command `{unknown}`; run `margatroid help`"),
    }
}

fn workspace_config(args: &[String]) -> Result<()> {
    let mut compose_path = None;
    let mut workspace_name = None;
    let mut format = "yaml";
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-f" | "--file" => {
                index += 1;
                compose_path = Some(PathBuf::from(
                    args.get(index).context("--file requires a path")?,
                ));
            }
            "-n" | "--name" => {
                index += 1;
                workspace_name = Some(
                    args.get(index)
                        .context("--name requires a workspace name")?
                        .clone(),
                );
            }
            "--format" => {
                index += 1;
                format = args.get(index).context("--format requires yaml or json")?;
                if format != "yaml" && format != "json" {
                    bail!("unsupported output format `{format}`; expected yaml or json");
                }
            }
            unknown => bail!("unknown workspace config option `{unknown}`"),
        }
        index += 1;
    }

    let compose_path = match compose_path {
        Some(path) => path,
        None => discover_compose(Path::new("."))?,
    };
    let mut options = CompileOptions::default();
    if let Some(name) = workspace_name {
        options = options.with_workspace_name(name);
    }
    let output = Compiler::new(options)
        .compile(&compose_path)
        .with_context(|| {
            let file = compose_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("compose file");
            format!("cannot compile {file}")
        })?;
    let rendered = if format == "json" {
        output.normalized().to_json()?
    } else {
        output.normalized().to_yaml()?
    };
    print!("{rendered}");
    if !rendered.ends_with('\n') {
        println!();
    }
    Ok(())
}

fn discover_compose(directory: &Path) -> Result<PathBuf> {
    let primary = directory.join(DEFAULT_COMPOSE_FILE);
    let alternate = directory.join(ALTERNATE_COMPOSE_FILE);
    match (primary.is_file(), alternate.is_file()) {
        (true, false) => Ok(primary),
        (false, true) => Ok(alternate),
        (true, true) => bail!(
            "both {DEFAULT_COMPOSE_FILE} and {ALTERNATE_COMPOSE_FILE} exist; select one with --file"
        ),
        (false, false) => bail!(
            "cannot find {DEFAULT_COMPOSE_FILE} or {ALTERNATE_COMPOSE_FILE} in {}",
            directory.display()
        ),
    }
}

async fn status(base_url: &str) -> Result<()> {
    let endpoint = format!("{}/health", base_url.trim_end_matches('/'));
    let response = reqwest::get(&endpoint)
        .await
        .with_context(|| format!("cannot reach margatroidd at {base_url}"))?
        .error_for_status()
        .with_context(|| format!("margatroidd health check failed at {endpoint}"))?;
    let body = response
        .text()
        .await
        .context("cannot read health response")?;
    if body.trim() != "ok" {
        bail!("unexpected health response from margatroidd: {body:?}");
    }
    println!("margatroidd is running at {base_url}");
    Ok(())
}

fn daemon_url() -> String {
    std::env::var("MARGATROID_URL")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_DAEMON_URL.into())
        .trim_end_matches('/')
        .to_string()
}

fn print_usage() {
    println!("Usage:");
    println!("  margatroid status");
    println!("  margatroid workspace config [-f file] [-n name] [--format yaml|json]");
    println!();
    println!("Environment:");
    println!("  MARGATROID_URL  daemon base URL (default: {DEFAULT_DAEMON_URL})");
    println!();
    println!("Protocol: {API_VERSION}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_daemon_url_uses_product_port() {
        assert_eq!(DEFAULT_DAEMON_URL, "http://127.0.0.1:3939");
        assert_eq!(API_VERSION, "v1");
    }

    #[test]
    fn compose_discovery_rejects_ambiguous_default_files() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(DEFAULT_COMPOSE_FILE), "").unwrap();
        std::fs::write(directory.path().join(ALTERNATE_COMPOSE_FILE), "").unwrap();
        assert!(discover_compose(directory.path()).is_err());
    }
}
