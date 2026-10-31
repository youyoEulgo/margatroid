#[tokio::main(flavor = "current_thread")]
async fn main() {
    std::process::exit(tool_plugin::runner_main().await);
}
