use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| {
            path.join("resources/tools/local/bash/latest/shell.lua")
                .is_file()
        })
        .expect("workspace resources must be available to integration tests")
        .to_owned()
}

/// Build only as test preparation; products always launch the packaged binary.
pub fn shell_client() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let root = workspace_root();
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                }
            })
            .unwrap_or_else(|| root.join("target"));
        let status = std::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "--offline",
                "-q",
                "-p",
                "shell-client",
                "--bin",
                "shell-client",
            ])
            .current_dir(&root)
            .env("CARGO_TARGET_DIR", &target)
            .status()
            .expect("build the standalone shell-client test fixture");
        assert!(
            status.success(),
            "shell-client fixture build failed: {status}"
        );
        let binary = target.join("debug/shell-client");
        assert!(
            binary.is_file(),
            "missing shell-client: {}",
            binary.display()
        );
        binary
    })
}

/// Preserve the resource layout while installing a test-built executable.
pub fn install_shell(resource: &Path) {
    std::fs::create_dir_all(resource.join("bin")).unwrap();
    std::fs::copy(
        workspace_root().join("resources/tools/local/bash/latest/shell.lua"),
        resource.join("shell.lua"),
    )
    .unwrap();
    std::fs::copy(shell_client(), resource.join("bin/shell-client")).unwrap();
}

// This shared module also serves integration binaries that create only Lua fixtures.
#[allow(dead_code)]
pub fn copy_resource(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_resource(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
    if source.join("shell.lua").is_file() {
        install_shell(destination);
    }
}
