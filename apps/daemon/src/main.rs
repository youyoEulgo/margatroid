mod instance;
mod options;

use std::process::ExitCode;

use anyhow::{Result, bail};
use app_runtime_plugin::AppRunExt;
use core_plugin::App;
use http_server_plugin::HttpServerFailed;
use margatroid_defaults::MargatroidDaemonPlugins;
use margatroid_protocol::API_VERSION;
use paths::DaemonPaths;
use signal_plugin::SignalListenerFailed;

use crate::instance::DaemonInstanceGuard;
use crate::options::{DaemonOptions, OptionsOutcome};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("margatroidd: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let options = match DaemonOptions::load(std::env::args_os().skip(1))? {
        OptionsOutcome::Run(options) => options,
        OptionsOutcome::Help => {
            print_help();
            return Ok(());
        }
    };
    let paths = DaemonPaths::new(&options.data_dir);
    paths.prepare()?;
    let _instance = DaemonInstanceGuard::acquire(paths.lock())?;

    let mut plugins = MargatroidDaemonPlugins::default().with_bind_address(options.bind_address);
    if let Some(token) = options.log_stream_bearer_token {
        plugins = plugins.with_log_stream_bearer_token(token);
    }

    let mut app = App::new();
    app.add_plugins(plugins);
    let mut http_failure_reader = app.event_reader::<HttpServerFailed>();
    let mut signal_failure_reader = app.event_reader::<SignalListenerFailed>();
    tracing::info!(
        bind_address = %options.bind_address,
        data_dir = %paths.data_dir().display(),
        config = ?options.config_path,
        api_version = API_VERSION,
        "margatroidd starting"
    );
    app.run();
    if let Some(failure) = app
        .world()
        .read_events(&mut signal_failure_reader)
        .into_iter()
        .next()
    {
        bail!("signal listener failed to start: {}", failure.message);
    }
    if let Some(failure) = app
        .world()
        .read_events(&mut http_failure_reader)
        .into_iter()
        .next()
    {
        bail!("HTTP server failed to start: {}", failure.message);
    }
    Ok(())
}

fn print_help() {
    println!(
        "margatroidd\n\n\
         Usage: margatroidd [OPTIONS]\n\n\
         Options:\n  \
           --bind <ADDRESS>   HTTP bind address\n  \
           --data-dir <PATH>  daemon data directory\n  \
           --config <PATH>    daemon TOML configuration\n  \
         -h, --help             print help\n\n\
         Precedence: CLI > environment > configuration file > defaults"
    );
}
