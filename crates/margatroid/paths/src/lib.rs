use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const APP_CONFIG_NAME: &str = "margatroid.toml";
const DAEMON_LOCK_NAME: &str = "margatroidd.lock";

pub fn margatroid_root() -> Option<PathBuf> {
    dirs::home_dir().map(|path| path.join(".margatroid"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonPaths {
    data_dir: PathBuf,
    config: PathBuf,
    lock: PathBuf,
}

impl DaemonPaths {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        Self {
            config: data_dir.join(APP_CONFIG_NAME),
            lock: data_dir.join(DAEMON_LOCK_NAME),
            data_dir,
        }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn config(&self) -> &Path {
        &self.config
    }

    pub fn lock(&self) -> &Path {
        &self.lock
    }

    pub fn prepare(&self) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("cannot create data directory {}", self.data_dir.display()))?;
        set_private_directory_permissions(&self.data_dir)
    }
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("cannot secure data directory {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod daemon_tests {
    use super::*;

    #[test]
    fn daemon_paths_share_one_root() {
        let paths = DaemonPaths::new("/tmp/margatroid-test");
        assert_eq!(
            paths.config(),
            Path::new("/tmp/margatroid-test/margatroid.toml")
        );
        assert_eq!(
            paths.lock(),
            Path::new("/tmp/margatroid-test/margatroidd.lock")
        );
    }
}
