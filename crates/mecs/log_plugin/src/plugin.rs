use std::sync::{Mutex, OnceLock};

use app_runtime_plugin::RuntimePlugin;
use core_plugin::{App, CoreError, Plugin, Resource};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::registry::Registry;
use tracing_subscriber::util::SubscriberInitExt;

use crate::event::event_log_system;
use crate::stream::{JsonLayer, TracingStream};
use crate::{ConsoleTarget, FileLogOptions, LogError, LogFormat, LogLevel, LogRotation};

type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync>;

static INSTALL_LOCK: Mutex<()> = Mutex::new(());
static INSTALLED_TRACING: OnceLock<InstalledTracing> = OnceLock::new();

struct InstalledTracing {
    configuration: TracingConfiguration,
    stream: Option<TracingStream>,
    _worker_guards: Vec<WorkerGuard>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TracingConfiguration {
    level: LogLevel,
    filter: Option<String>,
    format: LogFormat,
    console: Option<ConsoleTarget>,
    file: Option<FileLogOptions>,
    stream_capacity: Option<usize>,
}

struct LogPluginInstalled;
impl Resource for LogPluginInstalled {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogPlugin {
    level: LogLevel,
    filter: Option<String>,
    format: LogFormat,
    console: Option<ConsoleTarget>,
    file: Option<FileLogOptions>,
    stream_capacity: Option<usize>,
    schedule: String,
}

impl LogPlugin {
    pub fn with_level(mut self, level: LogLevel) -> Self {
        self.level = level;
        self
    }

    pub fn with_filter<Filter>(mut self, filter: Filter) -> Self
    where
        Filter: Into<String>,
    {
        self.filter = Some(filter.into());
        self
    }

    pub fn with_format(mut self, format: LogFormat) -> Self {
        self.format = format;
        self
    }

    pub fn with_console(mut self, target: ConsoleTarget) -> Self {
        self.console = Some(target);
        self
    }

    pub fn without_console(mut self) -> Self {
        self.console = None;
        self
    }

    pub fn with_file(mut self, options: FileLogOptions) -> Self {
        self.file = Some(options);
        self
    }

    pub fn with_stream(mut self, capacity: usize) -> Self {
        if capacity == 0 {
            LogError::InvalidStreamCapacity { capacity }.panic();
        }
        self.stream_capacity = Some(capacity);
        self
    }

    pub fn in_schedule<ScheduleName>(mut self, schedule: ScheduleName) -> Self
    where
        ScheduleName: Into<String>,
    {
        self.schedule = schedule.into();
        self
    }
}

impl Default for LogPlugin {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            filter: None,
            format: LogFormat::Compact,
            console: Some(ConsoleTarget::Stderr),
            file: None,
            stream_capacity: None,
            schedule: RuntimePlugin::POST_UPDATE.into(),
        }
    }
}

impl Plugin for LogPlugin {
    fn build(self, app: &mut App) {
        if app.world().contains_resource::<LogPluginInstalled>() {
            LogError::LogPluginAlreadyInstalled.panic();
        }
        assert!(
            app.contains_schedule(&self.schedule),
            "{}",
            CoreError::ScheduleNotFound {
                name: self.schedule.clone()
            }
        );
        let _ = build_filter(&self);

        install_tracing(&self, app);
        app.add_system(&self.schedule, event_log_system);
        app.world_mut().insert_resource(LogPluginInstalled);
    }
}

fn install_tracing(plugin: &LogPlugin, app: &mut App) {
    let configuration = TracingConfiguration {
        level: plugin.level,
        filter: plugin.filter.clone(),
        format: plugin.format,
        console: plugin.console,
        file: plugin.file.clone(),
        stream_capacity: plugin.stream_capacity,
    };
    let _install_guard = INSTALL_LOCK
        .lock()
        .expect("tracing installation lock poisoned");

    if let Some(installed) = INSTALLED_TRACING.get() {
        if installed.configuration != configuration {
            LogError::ConflictingConfiguration.panic();
        }
        if let Some(stream) = &installed.stream {
            app.world_mut().insert_resource(stream.clone());
        }
        return;
    }

    let mut layers = Vec::<BoxedLayer>::new();
    if let Some(target) = plugin.console {
        layers.push(console_layer(target, plugin.format, plugin));
    }

    let mut worker_guards = Vec::new();
    if let Some(options) = &plugin.file {
        let (layer, worker_guard) = file_layer(options, plugin.format, plugin);
        layers.push(layer);
        if let Some(worker_guard) = worker_guard {
            worker_guards.push(worker_guard);
        }
    }

    let stream = plugin.stream_capacity.map(TracingStream::new);
    if let Some(stream) = &stream {
        layers.push(stream.layer().with_filter(build_filter(plugin)).boxed());
    }

    if tracing_subscriber::registry()
        .with(layers)
        .try_init()
        .is_err()
    {
        LogError::SubscriberAlreadyInstalled.panic();
    }

    if let Some(stream) = &stream {
        app.world_mut().insert_resource(stream.clone());
    }
    if INSTALLED_TRACING
        .set(InstalledTracing {
            configuration,
            stream,
            _worker_guards: worker_guards,
        })
        .is_err()
    {
        panic!("installed tracing state must be empty while holding the installation lock");
    }
}

fn build_filter(plugin: &LogPlugin) -> EnvFilter {
    let filter = plugin
        .filter
        .as_deref()
        .unwrap_or_else(|| plugin.level.directive());
    EnvFilter::try_new(filter).unwrap_or_else(|source| {
        LogError::InvalidFilter {
            filter: filter.into(),
            source: Box::new(source),
        }
        .panic()
    })
}

fn console_layer(target: ConsoleTarget, format: LogFormat, plugin: &LogPlugin) -> BoxedLayer {
    match (format, target) {
        (LogFormat::Compact, ConsoleTarget::Stdout) => tracing_subscriber::fmt::layer()
            .compact()
            .with_writer(std::io::stdout)
            .with_filter(build_filter(plugin))
            .boxed(),
        (LogFormat::Compact, ConsoleTarget::Stderr) => tracing_subscriber::fmt::layer()
            .compact()
            .with_writer(std::io::stderr)
            .with_filter(build_filter(plugin))
            .boxed(),
        (LogFormat::Pretty, ConsoleTarget::Stdout) => tracing_subscriber::fmt::layer()
            .pretty()
            .with_writer(std::io::stdout)
            .with_filter(build_filter(plugin))
            .boxed(),
        (LogFormat::Pretty, ConsoleTarget::Stderr) => tracing_subscriber::fmt::layer()
            .pretty()
            .with_writer(std::io::stderr)
            .with_filter(build_filter(plugin))
            .boxed(),
        (LogFormat::Json, ConsoleTarget::Stdout) => JsonLayer::new(std::io::stdout)
            .with_filter(build_filter(plugin))
            .boxed(),
        (LogFormat::Json, ConsoleTarget::Stderr) => JsonLayer::new(std::io::stderr)
            .with_filter(build_filter(plugin))
            .boxed(),
    }
}

fn file_layer(
    options: &FileLogOptions,
    format: LogFormat,
    plugin: &LogPlugin,
) -> (BoxedLayer, Option<WorkerGuard>) {
    let rotation = match options.rotation {
        LogRotation::Minutely => Rotation::MINUTELY,
        LogRotation::Hourly => Rotation::HOURLY,
        LogRotation::Daily => Rotation::DAILY,
        LogRotation::Never => Rotation::NEVER,
    };
    let mut builder = RollingFileAppender::builder()
        .rotation(rotation)
        .filename_prefix(&options.file_name_prefix);
    if let Some(max_files) = options.max_files {
        builder = builder.max_log_files(max_files);
    }
    let appender = builder.build(&options.directory).unwrap_or_else(|source| {
        LogError::FileOutputInitFailed {
            directory: options.directory.clone(),
            source: Box::new(source),
        }
        .panic()
    });

    if options.non_blocking {
        let (writer, guard) = tracing_appender::non_blocking(appender);
        (format_file_layer(format, writer, plugin), Some(guard))
    } else {
        (format_file_layer(format, appender, plugin), None)
    }
}

fn format_file_layer<Writer>(format: LogFormat, writer: Writer, plugin: &LogPlugin) -> BoxedLayer
where
    Writer: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    match format {
        LogFormat::Compact => tracing_subscriber::fmt::layer()
            .compact()
            .with_ansi(false)
            .with_writer(writer)
            .with_filter(build_filter(plugin))
            .boxed(),
        LogFormat::Pretty => tracing_subscriber::fmt::layer()
            .pretty()
            .with_ansi(false)
            .with_writer(writer)
            .with_filter(build_filter(plugin))
            .boxed(),
        LogFormat::Json => JsonLayer::new(writer)
            .with_filter(build_filter(plugin))
            .boxed(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    #[test]
    fn default_configuration_matches_the_documented_surface() {
        let plugin = LogPlugin::default();

        assert_eq!(plugin.level, LogLevel::Info);
        assert_eq!(plugin.format, LogFormat::Compact);
        assert_eq!(plugin.console, Some(ConsoleTarget::Stderr));
        assert_eq!(plugin.file, None);
        assert_eq!(plugin.stream_capacity, None);
        assert_eq!(plugin.schedule, RuntimePlugin::POST_UPDATE);
    }

    #[test]
    #[should_panic(expected = "tracing stream capacity must be greater than zero")]
    fn zero_stream_capacity_is_rejected() {
        let _ = LogPlugin::default().with_stream(0);
    }

    #[test]
    #[should_panic(expected = "invalid tracing filter")]
    fn invalid_filter_is_rejected() {
        let _ = build_filter(&LogPlugin::default().with_filter("["));
    }

    #[test]
    fn structured_json_is_written_to_a_rolling_file() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("mecs-log-plugin-{suffix}"));
        let file = FileLogOptions::daily(&directory, "test")
            .with_rotation(LogRotation::Never)
            .blocking();
        let plugin = LogPlugin::default()
            .without_console()
            .with_format(LogFormat::Json)
            .with_file(file.clone());
        let (layer, guard) = file_layer(&file, LogFormat::Json, &plugin);
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
