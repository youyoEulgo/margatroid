use config_core::{ConfigError, IngressPolicy, LogLevel, MargatroidConfig, OutboundTarget};
use config_plugin::ConfigPlugin;
use core_plugin::App;
use std::fs;
use std::path::PathBuf;

const BASE: &str = "[server]\nbind = '127.0.0.1:3939'\n[outbound]\nlogs = ['type:cli', 'type:webui']\nbackend_state = ['type:webui']\n";

struct File {
    directory: tempfile::TempDir,
    path: PathBuf,
}

impl File {
    fn new(source: impl AsRef<[u8]>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, source).unwrap();
        Self { directory, path }
    }

    fn open(&self) -> Result<ConfigPlugin, ConfigError> {
        ConfigPlugin::open(&self.path)
    }
}

fn error(source: &str) -> ConfigError {
    File::new(source).open().unwrap_err()
}

#[test]
fn opens_configuration_with_default_log_and_ingress() {
    let file = File::new(BASE);
    let plugin = file.open().unwrap();
    let config = plugin.config();
    assert_eq!(config.server_bind(), "127.0.0.1:3939".parse().unwrap());
    assert_eq!(config.server_ingress(), &IngressPolicy::Localhost);
    assert_eq!(config.log_level(), LogLevel::Info);
    assert_eq!(config.log_filter(), None);
    assert_eq!(
        config.logs(),
        [
            OutboundTarget::Type("cli".into()),
            OutboundTarget::Type("webui".into())
        ]
    );
    assert_eq!(
        config.backend_state(),
        [OutboundTarget::Type("webui".into())]
    );
}

#[test]
fn installs_validated_configuration_without_a_file() {
    let config = MargatroidConfig::new(
        "0.0.0.0:4000".parse().unwrap(),
        LogLevel::Debug,
        Some("application=trace".into()),
        vec![OutboundTarget::Broadcast],
        vec![OutboundTarget::Name("frontend".into())],
    )
    .unwrap()
    .with_ingress(IngressPolicy::Any)
    .unwrap();
    let plugin = ConfigPlugin::new(config.clone());
    assert_eq!(plugin.config(), &config);
    let mut app = App::new();
    app.add_plugin(plugin);
    assert_eq!(
        app.world().get_resource::<MargatroidConfig>(),
        Some(&config)
    );
}

#[test]
fn build_installs_loaded_snapshot_without_reading_again() {
    let file = File::new(BASE);
    let plugin = file.open().unwrap();
    let expected = plugin.config().clone();
    assert_eq!(file.path.parent(), Some(file.directory.path()));
    fs::remove_file(&file.path).unwrap();
    let mut app = App::new();
    app.add_plugin(plugin);
    assert_eq!(
        app.world().get_resource::<MargatroidConfig>(),
        Some(&expected)
    );
    app.tick();
}

#[test]
#[should_panic(expected = "ConfigPlugin is already installed")]
fn refuses_duplicate_installation() {
    let file = File::new(BASE);
    let mut app = App::new();
    app.add_plugin(file.open().unwrap());
    app.add_plugin(file.open().unwrap());
}

#[test]
fn bounded_read_accepts_exact_limit_and_rejects_one_byte_more() {
    let mut exact = BASE.to_owned();
    exact.push_str(&" ".repeat(64 * 1024 - exact.len()));
    assert!(File::new(&exact).open().is_ok());
    exact.push(' ');
    assert_eq!(File::new(exact).open().unwrap_err(), ConfigError::TooLarge);
}

#[test]
fn distinguishes_read_failures_from_invalid_utf8_or_toml() {
    let file = File::new(BASE);
    let missing = file.directory.path().join("missing.toml");
    assert_eq!(
        ConfigPlugin::open(&missing).unwrap_err(),
        ConfigError::ReadFailed(missing)
    );
    assert_eq!(
        ConfigPlugin::open(file.directory.path()).unwrap_err(),
        ConfigError::ReadFailed(file.directory.path().to_path_buf())
    );
    assert_eq!(
        File::new([0xff, 0xfe]).open().unwrap_err(),
        ConfigError::DecodeFailed
    );
    assert_eq!(error("[broken"), ConfigError::DecodeFailed);
    assert_eq!(error(""), ConfigError::DecodeFailed);
}

#[test]
fn rejects_unknown_fields_at_every_document_level() {
    for source in [
        format!("unknown = 1\n{BASE}"),
        BASE.replace("bind =", "unknown = 1\nbind ="),
        format!("{BASE}[log]\nlevel = 'info'\nunknown = 1\n"),
        BASE.replace("logs =", "unknown = 1\nlogs ="),
        format!("{BASE}[inference]\nmodel = 'removed'\n"),
        BASE.replace("logs =", "member_messages = []\nlogs ="),
    ] {
        assert_eq!(error(&source), ConfigError::DecodeFailed, "{source}");
    }
}

#[test]
fn validates_bind_and_all_log_levels() {
    for bind in [
        "not-an-address",
        "localhost:3939",
        "127.0.0.1",
        "127.0.0.1:99999",
    ] {
        assert_eq!(
            error(&BASE.replace("127.0.0.1:3939", bind)),
            ConfigError::InvalidBind
        );
    }
    for (level, expected) in [
        ("off", LogLevel::Off),
        ("error", LogLevel::Error),
        ("warn", LogLevel::Warn),
        ("info", LogLevel::Info),
        ("debug", LogLevel::Debug),
        ("trace", LogLevel::Trace),
    ] {
        let source = format!("{BASE}[log]\nlevel = '{level}'\nfilter = 'module=debug'\n");
        let plugin = File::new(source).open().unwrap();
        assert_eq!(plugin.config().log_level(), expected);
        assert_eq!(plugin.config().log_filter(), Some("module=debug"));
    }
    assert_eq!(
        error(&format!("{BASE}[log]\nlevel = 'verbose'\n")),
        ConfigError::InvalidLogLevel
    );
    for filter in ["", "   "] {
        assert_eq!(
            error(&format!("{BASE}[log]\nfilter = '{filter}'\n")),
            ConfigError::InvalidLogFilter
        );
    }
    assert_eq!(
        File::new(format!("{BASE}[log]\nfilter = 'info,application=debug'\n"))
            .open()
            .unwrap()
            .config()
            .log_level(),
        LogLevel::Info
    );
}

#[test]
fn validates_both_outbound_groups_and_preserves_target_kinds() {
    for field in ["logs", "backend_state"] {
        let baseline = if field == "logs" {
            "logs = ['type:cli', 'type:webui']"
        } else {
            "backend_state = ['type:webui']"
        };
        for targets in ["[]", "['']", "['client:cli']", "['type:']", "['name:   ']"] {
            assert_eq!(
                error(&BASE.replace(baseline, &format!("{field} = {targets}"))),
                ConfigError::InvalidTarget,
                "{field} {targets}"
            );
        }
        for targets in [
            "['broadcast', 'broadcast']",
            "['type:cli', 'type:cli']",
            "['name:client', 'name:client']",
        ] {
            assert_eq!(
                error(&BASE.replace(baseline, &format!("{field} = {targets}"))),
                ConfigError::DuplicateTarget
            );
        }
    }
    let plugin = File::new(BASE.replace(
        "['type:cli', 'type:webui']",
        "['broadcast', 'name:local', 'type:local']",
    ))
    .open()
    .unwrap();
    assert_eq!(
        plugin.config().logs(),
        [
            OutboundTarget::Broadcast,
            OutboundTarget::Name("local".into()),
            OutboundTarget::Type("local".into())
        ]
    );
}

#[test]
fn accepts_supported_ingress_forms_and_cidr_lists() {
    for (allow, expected) in [
        ("'localhost'", IngressPolicy::Localhost),
        ("'127.0.0.1'", IngressPolicy::Localhost),
        ("'all'", IngressPolicy::Any),
        ("'0.0.0.0'", IngressPolicy::Any),
    ] {
        let source = BASE
            .replace("127.0.0.1:3939", "0.0.0.0:3939")
            .replace("[server]", &format!("[server]\nallow = {allow}"));
        assert_eq!(
            File::new(source).open().unwrap().config().server_ingress(),
            &expected
        );
    }
    let source = BASE.replace("127.0.0.1:3939", "[::]:3939").replace(
        "[server]",
        "[server]\nallow = ['10.0.0.0/8', '2001:db8::/32']",
    );
    let plugin = File::new(source).open().unwrap();
    let ingress = plugin.config().server_ingress();
    assert!(ingress.allows_peer("10.2.3.4".parse().unwrap()));
    assert!(ingress.allows_peer("2001:db8::5".parse().unwrap()));
    assert!(!ingress.allows_peer("192.168.0.1".parse().unwrap()));
    let source = BASE.replace("[server]", "[server]\nallow = ['127.0.0.0/8', '::1']");
    assert!(File::new(source).open().is_ok());
}

#[test]
fn rejects_invalid_ingress_and_policies_wider_than_bind() {
    for allow in [
        "true",
        "42",
        "'example.com'",
        "[7]",
        "['invalid']",
        "['10.0.0.0/33']",
        "['0.0.0.0/0']",
        "['::/0']",
        "['0.0.0.0']",
        "['::']",
    ] {
        let source = BASE
            .replace("127.0.0.1:3939", "0.0.0.0:3939")
            .replace("[server]", &format!("[server]\nallow = {allow}"));
        assert_eq!(error(&source), ConfigError::InvalidIngress, "{allow}");
    }
    for allow in ["'all'", "['10.0.0.0/8']", "['127.0.0.0/7']", "['::1/127']"] {
        assert_eq!(
            error(&BASE.replace("[server]", &format!("[server]\nallow = {allow}"))),
            ConfigError::InvalidIngress,
            "{allow}"
        );
    }
}

#[test]
fn reopening_observes_disk_changes_while_existing_plugin_keeps_snapshot() {
    let file = File::new(BASE);
    let original = file.open().unwrap();
    fs::write(&file.path, BASE.replace("127.0.0.1:3939", "127.0.0.1:4000")).unwrap();
    let latest = file.open().unwrap();
    assert_eq!(original.config().server_bind().port(), 3939);
    assert_eq!(latest.config().server_bind().port(), 4000);
}

const TOOL_FIELDS: [&str; 2] = ["lua_vm_memory_limit", "task_output_buffer_window"];

#[test]
fn omitted_tool_configuration_preserves_unspecified_limits() {
    for source in [BASE.to_owned(), format!("{BASE}[tools]\n")] {
        let plugin = File::new(source).open().unwrap();
        assert_eq!(
            plugin.config().tools(),
            &config_core::FileToolConfig::default()
        );
    }
    for field in TOOL_FIELDS {
        let plugin = File::new(format!("{BASE}[tools]\n{field} = '321B'\n"))
            .open()
            .unwrap();
        let tools = plugin.config().tools();
        assert_eq!(
            tools.lua_vm_memory_limit,
            (field == TOOL_FIELDS[0]).then_some(321)
        );
        assert_eq!(
            tools.task_output_buffer_window,
            (field == TOOL_FIELDS[1]).then_some(321)
        );
    }
}

#[test]
fn custom_tool_configuration_parses_lua_memory_and_output_capacities() {
    let plugin = File::new(format!(
        "{BASE}[tools]\nlua_vm_memory_limit = '2GiB'\ntask_output_buffer_window = '128M'\n"
    ))
    .open()
    .unwrap();
    assert_eq!(
        plugin.config().tools(),
        &config_core::FileToolConfig {
            lua_vm_memory_limit: Some(2 * 1024usize.pow(3)),
            task_output_buffer_window: Some(128 * 1024usize.pow(2)),
        }
    );
}

#[test]
fn tool_limits_reject_invalid_capacity_values() {
    for field in TOOL_FIELDS {
        for value in [
            "'0'",
            "'-1G'",
            "'1.5G'",
            "'1X'",
            "''",
            "'1 G'",
            "'18446744073709551616T'",
            "'18446744073709551615T'",
            "true",
            "1",
        ] {
            assert_eq!(
                error(&format!("{BASE}[tools]\n{field} = {value}\n")),
                ConfigError::DecodeFailed,
                "{field} = {value}"
            );
        }
    }
}

#[test]
fn every_tool_capacity_accepts_all_binary_units() {
    for field in TOOL_FIELDS {
        for (unit, exponent) in [
            ("", 0),
            ("B", 0),
            ("K", 1),
            ("kB", 1),
            ("KiB", 1),
            ("m", 2),
            ("MB", 2),
            ("mib", 2),
            ("G", 3),
            ("gB", 3),
            ("GiB", 3),
            ("t", 4),
            ("TB", 4),
            ("tIb", 4),
        ] {
            let plugin = File::new(format!("{BASE}[tools]\n{field} = ' 1{unit} ' \n"))
                .open()
                .unwrap();
            let tools = plugin.config().tools();
            let actual = match field {
                "lua_vm_memory_limit" => tools.lua_vm_memory_limit,
                _ => tools.task_output_buffer_window,
            };
            assert_eq!(actual, Some(1024usize.pow(exponent)), "{field} {unit}");
        }
    }
}

#[test]
fn tool_configuration_serializes_readable_capacities_and_round_trips() {
    for (bytes, rendered) in [
        (17, "17B"),
        (1024, "1K"),
        (1024usize.pow(2), "1M"),
        (1024usize.pow(3), "1G"),
        (1024usize.pow(4), "1T"),
    ] {
        let tools = config_core::FileToolConfig {
            lua_vm_memory_limit: Some(bytes),
            task_output_buffer_window: Some(bytes),
        };
        let source = toml::to_string(&tools).unwrap();
        for field in TOOL_FIELDS {
            assert!(
                source.contains(&format!("{field} = \"{rendered}\"")),
                "{source}"
            );
        }
        assert_eq!(
            toml::from_str::<config_core::FileToolConfig>(&source).unwrap(),
            tools
        );
    }
    let empty = config_core::FileToolConfig::default();
    assert_eq!(
        toml::from_str::<config_core::FileToolConfig>(&toml::to_string(&empty).unwrap()).unwrap(),
        empty
    );
}

#[test]
fn old_tool_sections_and_fields_are_unknown() {
    for suffix in [
        "[tools.limits]\n",
        "[tools.limits]\nmemory = '2G'\ncache = '128M'\n",
        "[tools.memory]\n",
        "[tools.cache]\n",
    ] {
        assert_eq!(error(&format!("{BASE}{suffix}")), ConfigError::DecodeFailed);
    }
    for field in [
        "unknown",
        "limits",
        "memory",
        "cache",
        "task_memory_limit",
        "total_task_memory_limit",
        "total_memory_bytes",
        "memory_bytes",
        "cache_bytes",
        "script_bytes",
        "argument_bytes",
        "output_bytes",
        "result_bytes",
        "diagnostic_bytes",
        "instruction_budget",
        "input_capacity",
        "data_depth",
        "image_count",
        "image_bytes",
        "images_bytes",
    ] {
        assert_eq!(
            error(&format!("{BASE}[tools]\n{field} = '1B'\n")),
            ConfigError::DecodeFailed,
            "{field}"
        );
    }
}
