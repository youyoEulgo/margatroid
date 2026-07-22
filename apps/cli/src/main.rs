use anyhow::{Context, Result, bail};

const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:3939";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        print_usage();
        return Ok(());
    };

    match command.as_str() {
        "status" => status(&daemon_url()).await,
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        unknown => bail!("unknown command `{unknown}`; run `margatroid help`"),
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
    println!();
    println!("Environment:");
    println!("  MARGATROID_URL  daemon base URL (default: {DEFAULT_DAEMON_URL})");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_daemon_url_uses_product_port() {
        assert_eq!(DEFAULT_DAEMON_URL, "http://127.0.0.1:3939");
    }
}
