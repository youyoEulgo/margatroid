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
        driver.write("base.lua", "return '你好'\n");
        driver.write("setting.toml", "");
        driver
    }

    fn write(&self, name: &str, value: impl AsRef<[u8]>) {
        fs::write(self.directory.join(name), value).unwrap();
    }

    fn initial(&self, limit: usize) -> Result<String, AgentDriverError> {
        agent_driver_core::read_initial_settings(&self.directory.join("setting.toml"), limit)
    }
    fn path(&self) -> PathBuf {
        self.directory.join("base.lua")
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
    driver.write("setting.toml", "[agent]\nname = '示例'\n");
    let input = driver.read().unwrap();
    assert_eq!(input.program, "return '你好'\n");
    assert_eq!(input.origin, driver.directory.join("base.lua"));
    assert_eq!(
        driver
            .initial(AgentDriverReadLimits::default().settings_bytes)
            .unwrap(),
        "[agent]\nname = '示例'\n"
    );
}

#[test]
fn empty_settings_and_optional_model_parameters_are_valid() {
    let driver = Driver::new();
    assert!(driver.read().is_ok());
    assert!(
        driver
            .initial(AgentDriverReadLimits::default().settings_bytes)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn every_file_is_required() {
    {
        let name = "base.lua";
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
    driver.write("setting.toml", "title = '界'\n");
    let exact = AgentDriverReadLimits {
        program_bytes: fs::metadata(driver.directory.join("base.lua"))
            .unwrap()
            .len() as usize,
        settings_bytes: fs::metadata(driver.directory.join("setting.toml"))
            .unwrap()
            .len() as usize,
    };
    assert!(read_driver(&driver.path(), &exact).is_ok());
    for index in 0..2 {
        let mut limits = exact;
        match index {
            0 => limits.program_bytes -= 1,
            _ => limits.settings_bytes -= 1,
        }
        let result = if index == 0 {
            read_driver(&driver.path(), &limits).map(|_| ())
        } else {
            driver.initial(limits.settings_bytes).map(|_| ())
        };
        assert_eq!(result, Err(AgentDriverError::TooLarge));
    }
}

#[test]
fn zero_and_maximum_limits_do_not_overflow() {
    let driver = Driver::new();
    let limits = AgentDriverReadLimits {
        program_bytes: usize::MAX,
        settings_bytes: 0,
    };
    assert!(read_driver(&driver.path(), &limits).is_ok());
    driver.write("setting.toml", "\n");
    assert_eq!(
        driver.initial(limits.settings_bytes),
        Err(AgentDriverError::TooLarge)
    );
    driver.write("setting.toml", "");
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
    {
        let name = "base.lua";
        let driver = Driver::new();
        driver.write(name, [0xff, 0xfe]);
        assert_eq!(driver.read(), Err(AgentDriverError::InvalidUtf8), "{name}");
    }
}

#[test]
fn validates_settings_without_changing_original_text() {
    let driver = Driver::new();
    for source in ["broken = [", "key = 1\nkey = 2", "[bad"] {
        driver.write("setting.toml", source);
        assert_eq!(
            driver.initial(AgentDriverReadLimits::default().settings_bytes),
            Err(AgentDriverError::InvalidDefaultSettings)
        );
        assert!(driver.read().is_ok());
    }
    let source = "# retained user-facing template\n[agent]\nname = 'test'\n\n";
    driver.write("setting.toml", source);
    assert_eq!(
        driver
            .initial(AgentDriverReadLimits::default().settings_bytes)
            .unwrap(),
        source
    );
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
    let target = driver.directory.join("setting.toml");
    assert_eq!(target.parent(), Some(driver.directory.as_path()));
    fs::remove_file(&target).unwrap();
    fs::create_dir(target).unwrap();
    assert_eq!(
        driver.initial(AgentDriverReadLimits::default().settings_bytes),
        Err(AgentDriverError::InvalidPath)
    );
}

#[test]
fn rereads_materials_without_cache_and_does_not_modify_them() {
    let driver = Driver::new();
    let first = driver.read().unwrap();
    driver.write("base.lua", "return 'updated'\n");
    driver.write("setting.toml", "enabled = true\n");
    let second = driver.read().unwrap();
    assert_eq!(first.program, "return '你好'\n");
    assert_eq!(second.program, "return 'updated'\n");
    assert_eq!(
        driver
            .initial(AgentDriverReadLimits::default().settings_bytes)
            .unwrap(),
        "enabled = true\n"
    );
    assert_eq!(
        fs::read_to_string(driver.directory.join("base.lua")).unwrap(),
        second.program
    );
    assert_eq!(
        fs::read_to_string(driver.directory.join("setting.toml")).unwrap(),
        driver
            .initial(AgentDriverReadLimits::default().settings_bytes)
            .unwrap()
    );
    assert_eq!(fs::read_dir(&driver.directory).unwrap().count(), 2);
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

#[test]
fn plural_settings_filename_is_not_a_template_fallback() {
    let driver = Driver::new();
    let current = driver.directory.join("setting.toml");
    let obsolete = driver.directory.join("settings.toml");
    assert_eq!(current.parent(), Some(driver.directory.as_path()));
    assert_eq!(obsolete.parent(), Some(driver.directory.as_path()));
    assert_eq!(current.file_name().unwrap(), "setting.toml");
    assert_eq!(obsolete.file_name().unwrap(), "settings.toml");
    assert!(!obsolete.exists());
    fs::rename(current, obsolete).unwrap();
    assert_eq!(
        driver.initial(AgentDriverReadLimits::default().settings_bytes),
        Err(AgentDriverError::FileMissing)
    );
    assert!(driver.read().is_ok());
}
