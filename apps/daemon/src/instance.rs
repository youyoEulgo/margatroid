use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};

#[derive(Debug)]
pub(crate) struct DaemonInstanceGuard {
    file: File,
}

impl DaemonInstanceGuard {
    pub(crate) fn acquire(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(path)
            .with_context(|| format!("cannot open daemon lock {}", path.display()))?;
        secure_lock_file(&file, path)?;
        if let Err(error) = file.try_lock() {
            match error {
                TryLockError::WouldBlock => bail!(
                    "another margatroidd instance owns data directory {}",
                    path.parent().unwrap_or(path).display()
                ),
                TryLockError::Error(error) => {
                    return Err(error)
                        .with_context(|| format!("cannot lock daemon file {}", path.display()));
                }
            }
        }

        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(file, "{}", std::process::id())?;
        file.sync_data()?;
        Ok(Self { file })
    }
}

impl Drop for DaemonInstanceGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(unix)]
fn secure_lock_file(file: &File, path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot secure daemon lock {}", path.display()))
}

#[cfg(not(unix))]
fn secure_lock_file(_file: &File, _path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_second_instance_for_same_data_directory() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("margatroidd.lock");
        let first = DaemonInstanceGuard::acquire(&path).unwrap();
        let error = DaemonInstanceGuard::acquire(&path).unwrap_err();

        assert!(error.to_string().contains("another margatroidd instance"));
        drop(first);
        DaemonInstanceGuard::acquire(&path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn lock_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("margatroidd.lock");
        let _guard = DaemonInstanceGuard::acquire(&path).unwrap();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
