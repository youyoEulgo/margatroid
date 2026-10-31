use agent_driver_core::{AgentDriverError, AgentDriverReadLimits, read_driver};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct Driver {
    directory: PathBuf,
}

impl Driver {
    fn new() -> Self {
        let directory = loop {
            let next = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "agent-driver-core-test-{}-{next}",
                std::process::id()
            ));
            match fs::create_dir(&directory) {
                Ok(()) => break directory,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create test directory: {error}"),
            }
        };
        let driver = Self { directory };
        driver.manifest("schema_version = 1\n[inference]\nmodel = 'test/model'\n");
        driver.write("base.lua", "return '你好'\n");
        driver.write("settings.toml", "");
        driver
    }

    fn write(&self, name: &str, value: impl AsRef<[u8]>) {
        fs::write(self.directory.join(name), value).unwrap();
    }

    fn manifest(&self, source: &str) {
        self.write("driver.toml", source);
    }

    fn path(&self) -> PathBuf {
        self.directory.join("driver.toml")
    }

    fn read(&self) -> Result<agent_driver_core::AgentDriverInput, AgentDriverError> {
        read_driver(&self.path(), &AgentDriverReadLimits::default())
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        assert_eq!(
            self.directory.parent(),
            Some(std::env::temp_dir().as_path())
        );
        assert!(
            self.directory
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("agent-driver-core-test-")
        );
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
fn reads_complete_materials_without_resolving_dependencies() {
    let driver = Driver::new();
    driver.manifest(
        "schema_version = 1\ndependencies = ['prompt:local/missing', 'tool:local/工具:v1.2', 'sandbox:local/workspace:latest']\n[inference]\nmodel = 'provider/model'\ntemperature = 0.5\ntop_p = 0.9\nmax_output_tokens = 2048\nstop = ['END', '停止']\n",
    );
    driver.write("settings.toml", "[agent]\nname = '示例'\n");
    let input = driver.read().unwrap();
    assert_eq!(input.manifest.schema_version, 1);
    assert_eq!(
        input.manifest.dependencies,
        [
            "prompt:local/missing",
            "tool:local/工具:v1.2",
            "sandbox:local/workspace:latest"
        ]
    );
    assert_eq!(input.manifest.inference.model, "provider/model");
    assert_eq!(input.manifest.inference.temperature, Some(0.5));
    assert_eq!(input.manifest.inference.top_p, Some(0.9));
    assert_eq!(input.manifest.inference.max_output_tokens, Some(2048));
    assert_eq!(input.manifest.inference.stop, ["END", "停止"]);
    assert_eq!(input.program, "return '你好'\n");
    assert_eq!(input.origin, driver.directory.join("base.lua"));
    assert_eq!(input.default_settings, "[agent]\nname = '示例'\n");
}

#[test]
fn empty_settings_and_optional_model_parameters_are_valid() {
    let driver = Driver::new();
    let input = driver.read().unwrap();
    assert!(input.default_settings.is_empty());
    assert!(input.manifest.dependencies.is_empty());
    assert_eq!(input.manifest.inference.temperature, None);
    assert_eq!(input.manifest.inference.top_p, None);
    assert_eq!(input.manifest.inference.max_output_tokens, None);
    assert!(input.manifest.inference.stop.is_empty());
}

#[test]
fn every_file_is_required() {
    for name in ["driver.toml", "base.lua", "settings.toml"] {
        let driver = Driver::new();
        let target = driver.directory.join(name);
        assert_eq!(target.parent(), Some(driver.directory.as_path()));
        fs::remove_file(target).unwrap();
        assert_eq!(driver.read(), Err(AgentDriverError::FileMissing), "{name}");
    }
}

#[test]
fn each_limit_accepts_exact_size_and_rejects_one_byte_less() {
    let driver = Driver::new();
    driver.write("settings.toml", "title = '界'\n");
    let exact = AgentDriverReadLimits {
        manifest_bytes: fs::metadata(driver.path()).unwrap().len() as usize,
        program_bytes: fs::metadata(driver.directory.join("base.lua"))
            .unwrap()
            .len() as usize,
        settings_bytes: fs::metadata(driver.directory.join("settings.toml"))
            .unwrap()
            .len() as usize,
    };
    assert!(read_driver(&driver.path(), &exact).is_ok());
    for index in 0..3 {
        let mut limits = exact;
        match index {
            0 => limits.manifest_bytes -= 1,
            1 => limits.program_bytes -= 1,
            _ => limits.settings_bytes -= 1,
        }
        assert_eq!(
            read_driver(&driver.path(), &limits),
            Err(AgentDriverError::TooLarge)
        );
    }
}

#[test]
fn zero_and_maximum_limits_do_not_overflow() {
    let driver = Driver::new();
    let limits = AgentDriverReadLimits {
        manifest_bytes: usize::MAX,
        program_bytes: usize::MAX,
        settings_bytes: 0,
    };
    assert!(read_driver(&driver.path(), &limits).is_ok());
    driver.write("settings.toml", "\n");
    assert_eq!(
        read_driver(&driver.path(), &limits),
        Err(AgentDriverError::TooLarge)
    );
    driver.write("settings.toml", "");
    driver.write("base.lua", "");
    assert!(
        read_driver(
            &driver.path(),
            &AgentDriverReadLimits {
                program_bytes: 0,
                ..limits
            }
        )
        .is_ok()
    );
}

#[test]
fn rejects_invalid_utf8_in_every_file() {
    for name in ["driver.toml", "base.lua", "settings.toml"] {
        let driver = Driver::new();
        driver.write(name, [0xff, 0xfe]);
        assert_eq!(driver.read(), Err(AgentDriverError::InvalidUtf8), "{name}");
    }
}

#[test]
fn rejects_invalid_manifest_structure_and_version() {
    let driver = Driver::new();
    for source in [
        "",
        "schema_version = 1\n",
        "schema_version = 2\n[inference]\nmodel = 'test'\n",
        "schema_version = -1\n[inference]\nmodel = 'test'\n",
        "[inference]\nmodel = 'test'\n",
        "schema_version = 1\ndependencies = [1]\n[inference]\nmodel = 'test'\n",
        "schema_version = 1\n[inference]\nmodel = 7\n",
        "schema_version = 1\n[inference]\n",
        "schema_version = 1\n[inference]\nmodel = 'test'\nmodel = 'duplicate'\n",
    ] {
        driver.manifest(source);
        assert_eq!(
            driver.read(),
            Err(AgentDriverError::InvalidManifest),
            "{source}"
        );
    }
}

#[test]
fn validates_numeric_model_parameters_including_nonfinite_values() {
    let driver = Driver::new();
    for parameter in [
        "temperature = -0.1",
        "temperature = 2.1",
        "temperature = nan",
        "temperature = inf",
        "temperature = -inf",
        "top_p = -0.1",
        "top_p = 1.1",
        "top_p = nan",
        "top_p = inf",
        "top_p = -inf",
        "max_output_tokens = 0",
        "max_output_tokens = -1",
        "max_output_tokens = 4294967296",
        "stop = [1]",
    ] {
        driver.manifest(&format!(
            "schema_version = 1\n[inference]\nmodel = 'test'\n{parameter}\n"
        ));
        assert_eq!(
            driver.read(),
            Err(AgentDriverError::InvalidManifest),
            "{parameter}"
        );
    }
    for parameter in [
        "temperature = 0.0",
        "temperature = 2.0",
        "top_p = 0.0",
        "top_p = 1.0",
        "max_output_tokens = 1",
        "max_output_tokens = 4294967295",
    ] {
        driver.manifest(&format!(
            "schema_version = 1\n[inference]\nmodel = 'test'\n{parameter}\n"
        ));
        assert!(driver.read().is_ok(), "{parameter}");
    }
}

#[test]
fn enforces_model_identifier_and_stop_sequence_budgets() {
    let driver = Driver::new();
    for model in [
        String::new(),
        "   ".into(),
        "x".repeat(1025),
        "line\\nbreak".into(),
    ] {
        driver.manifest(&format!(
            "schema_version = 1\n[inference]\nmodel = \"{model}\"\n"
        ));
        assert_eq!(driver.read(), Err(AgentDriverError::InvalidManifest));
    }
    driver.manifest(&format!(
        "schema_version = 1\n[inference]\nmodel = '{}'\nstop = ['{}']\n",
        "x".repeat(1024),
        "界".repeat(1365)
    ));
    assert!(driver.read().is_ok());
    driver.manifest(&format!(
        "schema_version = 1\n[inference]\nmodel = 'test'\nstop = ['{}']\n",
        "x".repeat(4097)
    ));
    assert_eq!(driver.read(), Err(AgentDriverError::InvalidManifest));
    for count in [128, 129] {
        let stops = vec!["'end'"; count].join(",");
        driver.manifest(&format!(
            "schema_version = 1\n[inference]\nmodel = 'test'\nstop = [{stops}]\n"
        ));
        assert_eq!(driver.read().is_ok(), count == 128);
    }
}

#[test]
fn validates_dependency_syntax_without_normalizing_or_resolving() {
    let driver = Driver::new();
    for dependency in [
        "",
        "tool",
        "Tool:local/name",
        ":local/name",
        "tool:/name",
        "tool:local/",
        "tool:local/name:",
        "tool:local/name:tag:extra",
        "tool:local/name/extra",
        "tool:../name",
        "tool:local/.",
        "tool:local/name:..",
        "tool:lo\\cal/name",
        "tool:local/na\\me",
        "tool:local/name:ta\\g",
        "https://host/file",
        "tool:local/name\n",
    ] {
        let encoded = toml::Value::String(dependency.into()).to_string();
        driver.manifest(&format!(
            "schema_version = 1\ndependencies = [{encoded}]\n[inference]\nmodel = 'test'\n"
        ));
        assert_eq!(
            driver.read(),
            Err(AgentDriverError::InvalidManifest),
            "{dependency:?}"
        );
    }
    for dependency in [
        "tool:local/name",
        "tool:作用域/名称:标签",
        "tool_2:local/name:v1.2",
    ] {
        driver.manifest(&format!(
            "schema_version = 1\ndependencies = ['{dependency}']\n[inference]\nmodel = 'test'\n"
        ));
        assert_eq!(driver.read().unwrap().manifest.dependencies, [dependency]);
    }
}

#[test]
fn validates_settings_without_changing_original_text() {
    let driver = Driver::new();
    for source in ["broken = [", "key = 1\nkey = 2", "[bad"] {
        driver.write("settings.toml", source);
        assert_eq!(driver.read(), Err(AgentDriverError::InvalidDefaultSettings));
    }
    let source = "# retained user-facing template\n[agent]\nname = 'test'\n\n";
    driver.write("settings.toml", source);
    assert_eq!(driver.read().unwrap().default_settings, source);
}

#[test]
fn rejects_empty_paths_and_directories_in_place_of_files() {
    let driver = Driver::new();
    assert_eq!(
        read_driver(Path::new(""), &AgentDriverReadLimits::default()),
        Err(AgentDriverError::InvalidPath)
    );
    assert_eq!(
        read_driver(&driver.directory, &AgentDriverReadLimits::default()),
        Err(AgentDriverError::InvalidPath)
    );
    let target = driver.directory.join("settings.toml");
    assert_eq!(target.parent(), Some(driver.directory.as_path()));
    fs::remove_file(&target).unwrap();
    fs::create_dir(target).unwrap();
    assert_eq!(driver.read(), Err(AgentDriverError::InvalidPath));
}

#[test]
fn rereads_materials_without_cache_and_does_not_modify_them() {
    let driver = Driver::new();
    let first = driver.read().unwrap();
    driver.write("base.lua", "return 'updated'\n");
    driver.write("settings.toml", "enabled = true\n");
    let second = driver.read().unwrap();
    assert_eq!(first.program, "return '你好'\n");
    assert_eq!(second.program, "return 'updated'\n");
    assert_eq!(second.default_settings, "enabled = true\n");
    assert_eq!(
        fs::read_to_string(driver.directory.join("base.lua")).unwrap(),
        second.program
    );
    assert_eq!(
        fs::read_to_string(driver.directory.join("settings.toml")).unwrap(),
        second.default_settings
    );
    assert_eq!(fs::read_dir(&driver.directory).unwrap().count(), 3);
}

#[test]
fn public_errors_implement_display_and_standard_error() {
    fn check(error: &dyn std::error::Error) {
        assert!(!error.to_string().is_empty());
    }
    for error in [
        AgentDriverError::InvalidPath,
        AgentDriverError::FileMissing,
        AgentDriverError::TooLarge,
        AgentDriverError::InvalidUtf8,
        AgentDriverError::InvalidManifest,
        AgentDriverError::InvalidDefaultSettings,
        AgentDriverError::ReadFailed("test failure".into()),
    ] {
        check(&error);
    }
}
