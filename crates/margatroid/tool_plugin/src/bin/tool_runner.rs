#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(tool_plugin::runner_main().await as u8)
}
