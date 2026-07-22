use std::sync::{Mutex, OnceLock};

use core_plugin::{App, Plugin};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::registry::Registry;
use tracing_subscriber::util::SubscriberInitExt;

use crate::options::{
    ConsoleOptions, ConsoleTarget, FileLogOptions, LogFormat, LogLevel, LogOptions, LogRotation,
    LogStreamOptions,
};
use crate::stream::{JsonLayer, LogStream};

type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync>;

static WORKER_GUARDS: OnceLock<Mutex<Vec<WorkerGuard>>> = OnceLock::new();
static MANAGED_STREAM: OnceLock<LogStream> = OnceLock::new();
static MANAGED_OPTIONS: OnceLock<LogOptions> = OnceLock::new();
static INSTALL_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Default)]
pub struct LogPlugin {
    options: LogOptions,
}

impl LogPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_options(options: LogOptions) -> Self {
        Self { options }
    }

    pub fn with_level(mut self, level: LogLevel) -> Self {
        self.options.level = level;
        self
    }

    pub fn with_filter(mut self, filter: impl Into<String>) -> Self {
        self.options.filter = Some(filter.into());
        self
    }

    pub fn with_format(mut self, format: LogFormat) -> Self {
        self.options.format = format;
        self
    }

    pub fn with_console(mut self, options: ConsoleOptions) -> Self {
        self.options.console = Some(options);
        self
    }

    pub fn without_console(mut self) -> Self {
        self.options.console = None;
        self
    }

    pub fn with_file(mut self, options: FileLogOptions) -> Self {
        self.options.file = Some(options);
        self
    }

    pub fn with_stream(mut self, options: LogStreamOptions) -> Self {
        self.options.stream = Some(options);
        self
    }
}

impl Plugin for LogPlugin {
    fn build(&self, app: &mut App) {
        let _install_guard = INSTALL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut layers = Vec::<BoxedLayer>::new();
        if let Some(console) = &self.options.console {
            layers.push(console_layer(console, self.options.format, &self.options));
        }

        let mut worker_guard = None;
        if let Some(file) = &self.options.file {
            match file_layer(file, self.options.format, &self.options) {
                Ok((layer, guard)) => {
                    layers.push(layer);
                    worker_guard = guard;
                }
                Err(error) => {
                    eprintln!("log_plugin: failed to configure file output: {error}");
                    return;
                }
            }
        }

        let stream = self
            .options
            .stream
            .as_ref()
            .map(|options| LogStream::new(options.capacity()));
        if let Some(stream) = &stream {
            layers.push(
                stream
                    .layer()
                    .with_filter(build_filter(&self.options))
                    .boxed(),
            );
        }

        if let Err(error) = tracing_subscriber::registry().with(layers).try_init() {
            if let Some(installed) = MANAGED_OPTIONS.get() {
                if installed != &self.options {
                    eprintln!(
                        "log_plugin: requested options differ from the process-level options; \
                         the first installation remains active"
                    );
                }
                if self.options.stream.is_some() {
                    if let Some(stream) = MANAGED_STREAM.get() {
                        app.add_resource(stream.clone());
                    }
                }
            }
            eprintln!("log_plugin: global tracing subscriber already exists: {error}");
            return;
        }

        let _ = MANAGED_OPTIONS.set(self.options.clone());
        if let Some(guard) = worker_guard {
            WORKER_GUARDS
                .get_or_init(|| Mutex::new(Vec::new()))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(guard);
        }
        if let Some(stream) = stream {
            let _ = MANAGED_STREAM.set(stream.clone());
            app.add_resource(stream);
        }
    }
}

fn build_filter(options: &LogOptions) -> EnvFilter {
    let value = options
        .filter
        .as_deref()
        .unwrap_or_else(|| options.level.directive());
    EnvFilter::try_new(value).unwrap_or_else(|error| {
        eprintln!("log_plugin: invalid filter `{value}`: {error}; using info");
        EnvFilter::new("info")
    })
}

fn console_layer(
    options: &ConsoleOptions,
    format: LogFormat,
    log_options: &LogOptions,
) -> BoxedLayer {
    match (format, options.target()) {
        (LogFormat::Compact, ConsoleTarget::Stdout) => tracing_subscriber::fmt::layer()
            .compact()
            .with_writer(std::io::stdout)
            .with_filter(build_filter(log_options))
            .boxed(),
        (LogFormat::Compact, ConsoleTarget::Stderr) => tracing_subscriber::fmt::layer()
            .compact()
            .with_writer(std::io::stderr)
            .with_filter(build_filter(log_options))
            .boxed(),
        (LogFormat::Pretty, ConsoleTarget::Stdout) => tracing_subscriber::fmt::layer()
            .pretty()
            .with_writer(std::io::stdout)
            .with_filter(build_filter(log_options))
            .boxed(),
        (LogFormat::Pretty, ConsoleTarget::Stderr) => tracing_subscriber::fmt::layer()
            .pretty()
            .with_writer(std::io::stderr)
            .with_filter(build_filter(log_options))
            .boxed(),
        (LogFormat::Json, ConsoleTarget::Stdout) => JsonLayer::new(std::io::stdout)
            .with_filter(build_filter(log_options))
            .boxed(),
        (LogFormat::Json, ConsoleTarget::Stderr) => JsonLayer::new(std::io::stderr)
            .with_filter(build_filter(log_options))
            .boxed(),
    }
}

fn file_layer(
    options: &FileLogOptions,
    format: LogFormat,
    log_options: &LogOptions,
) -> Result<(BoxedLayer, Option<WorkerGuard>), tracing_appender::rolling::InitError> {
    let rotation = match options.rotation() {
        LogRotation::Minutely => Rotation::MINUTELY,
        LogRotation::Hourly => Rotation::HOURLY,
        LogRotation::Daily => Rotation::DAILY,
        LogRotation::Never => Rotation::NEVER,
    };
    let mut builder = RollingFileAppender::builder()
        .rotation(rotation)
        .filename_prefix(options.file_name_prefix());
    if let Some(max_files) = options.max_files() {
        builder = builder.max_log_files(max_files);
    }
    let appender = builder.build(options.directory())?;

    if options.is_non_blocking() {
        let (writer, guard) = tracing_appender::non_blocking(appender);
        Ok((format_file_layer(format, writer, log_options), Some(guard)))
    } else {
        Ok((format_file_layer(format, appender, log_options), None))
    }
}

fn format_file_layer<W>(format: LogFormat, writer: W, options: &LogOptions) -> BoxedLayer
where
    W: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    match format {
        LogFormat::Compact => tracing_subscriber::fmt::layer()
            .compact()
            .with_writer(writer)
            .with_ansi(false)
            .with_filter(build_filter(options))
            .boxed(),
        LogFormat::Pretty => tracing_subscriber::fmt::layer()
            .pretty()
            .with_writer(writer)
            .with_ansi(false)
            .with_filter(build_filter(options))
            .boxed(),
        LogFormat::Json => JsonLayer::new(writer)
            .with_filter(build_filter(options))
            .boxed(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    #[test]
    fn default_is_console_info_compact() {
        let plugin = LogPlugin::default();
        assert_eq!(plugin.options.level, LogLevel::Info);
        assert_eq!(plugin.options.format, LogFormat::Compact);
        assert!(plugin.options.console.is_some());
        assert!(plugin.options.file.is_none());
        assert!(plugin.options.stream.is_none());
    }

    #[test]
    fn writes_structured_json_to_rolling_file() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("mecs-log-plugin-{suffix}"));
        let file = FileLogOptions::daily(&directory, "test")
            .with_rotation(LogRotation::Never)
            .blocking();
        let options = LogOptions::default()
            .without_console()
            .with_format(LogFormat::Json)
            .with_file(file.clone());
        let (layer, guard) = file_layer(&file, LogFormat::Json, &options).unwrap();
        let subscriber = tracing_subscriber::registry().with(vec![layer]);

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(request_id = 42, "written to file");
        });
        drop(guard);

        let path = std::fs::read_dir(&directory)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("written to file"), "{content}");
        assert!(content.contains("request_id"), "{content}");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
