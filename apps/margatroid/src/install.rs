use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::Command,
    time::Duration,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const VERSION: &str = "0.22.5";
const BINARIES: [&str; 4] = ["margatroid", "tool_runner", "shell-client", "mcp-client"];

#[tokio::main]
async fn main() {
    if let Err(error) = install().await {
        eprintln!("margatroid installer: {error}");
        std::process::exit(1);
    }
}
fn fail(message: impl Into<String>) -> Box<dyn std::error::Error> {
    std::io::Error::other(message.into()).into()
}
fn release_platform(os: &str, arch: &str) -> Result<(&'static str, &'static str)> {
    let platform = match os {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "win32",
        _ => return Err(fail("Unsupported Landstrip platform; use --skip-landstrip")),
    };
    let arch = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => {
            return Err(fail(
                "Unsupported Landstrip architecture; use --skip-landstrip",
            ))
        }
    };
    Ok((platform, arch))
}
fn executable_name(name: &str) -> String {
    format!("{name}{}", env::consts::EXE_SUFFIX)
}
fn within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}
fn normalized(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };
    let mut output = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                output.pop();
            }
            Component::CurDir => (),
            value => output.push(value.as_os_str()),
        }
    }
    Ok(output)
}
fn reject_symlinks(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(fail(format!(
                    "Refusing symlinked installation path: {}",
                    ancestor.display()
                )))
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
fn install_bytes(target: &Path, content: &[u8], mode: u32, roots: &[&Path]) -> Result<()> {
    if !roots.iter().any(|root| within(target, root)) {
        return Err(fail("Installation target escapes destination"));
    }
    reject_symlinks(target)?;
    #[cfg(not(unix))]
    let _ = mode;
    let parent = target
        .parent()
        .ok_or_else(|| fail("Target has no parent"))?;
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!(
        ".margatroid-install-{}-{}",
        std::process::id(),
        target.file_name().unwrap().to_string_lossy()
    ));
    if staging.exists() || fs::symlink_metadata(&staging).is_ok() {
        return Err(fail(format!(
            "Staging file already exists: {}",
            staging.display()
        )));
    }
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        file.write_all(content)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        // Both absolute paths and their shared parent are checked before atomic replacement.
        reject_symlinks(target)?;
        fs::rename(&staging, target)?;
        Ok(())
    })();
    if staging.exists() {
        if staging.parent() != Some(parent) {
            return Err(fail("Unexpected staging path"));
        }
        fs::remove_file(&staging)?;
    }
    result
}
fn copy_resources(
    source: &Path,
    destination: &Path,
    update: bool,
    roots: &[&Path],
    counts: &mut (usize, usize),
) -> Result<()> {
    let mut entries = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let metadata = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if metadata.is_symlink() {
            return Err(fail(format!(
                "Refusing source symlink: {}",
                entry.path().display()
            )));
        }
        if metadata.is_dir() {
            copy_resources(&entry.path(), &target, update, roots, counts)?;
        } else if metadata.is_file() {
            reject_symlinks(&target)?;
            if target.exists() && !update {
                counts.1 += 1;
                continue;
            }
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                entry.metadata()?.permissions().mode() & 0o777
            };
            #[cfg(not(unix))]
            let mode = 0o644;
            install_bytes(&target, &fs::read(entry.path())?, mode, roots)?;
            counts.0 += 1;
        }
    }
    Ok(())
}
// Read the single executable from the release tar without extracting paths to disk.
fn release_executable(archive: &[u8]) -> Result<Vec<u8>> {
    let mut tar = Vec::new();
    GzDecoder::new(archive).read_to_end(&mut tar)?;
    let mut offset = 0usize;
    let mut executable = None;
    while offset.checked_add(512).is_some_and(|end| end <= tar.len()) {
        let header = &tar[offset..offset + 512];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let octal = |bytes: &[u8]| -> Result<usize> {
            let value = std::str::from_utf8(bytes)?.trim_matches(['\0', ' ']);
            Ok(if value.is_empty() {
                0
            } else {
                usize::from_str_radix(value, 8)?
            })
        };
        let expected = octal(&header[148..156])?;
        let actual: usize = header
            .iter()
            .enumerate()
            .map(|(index, byte)| {
                if (148..156).contains(&index) {
                    32
                } else {
                    usize::from(*byte)
                }
            })
            .sum();
        if expected != actual {
            return Err(fail("Invalid release tar header checksum"));
        }
        let name = std::str::from_utf8(&header[..100])?.trim_end_matches('\0');
        let size = octal(&header[124..136])?;
        let start = offset
            .checked_add(512)
            .ok_or_else(|| fail("Tar offset overflow"))?;
        let end = start
            .checked_add(size)
            .filter(|end| *end <= tar.len())
            .ok_or_else(|| fail("Truncated release tar"))?;
        if (header[156] == 0 || header[156] == b'0')
            && Path::new(name)
                .file_name()
                .is_some_and(|name| name == "landstrip" || name == "landstrip.exe")
        {
            if executable.is_some() {
                return Err(fail("Multiple release executables"));
            }
            executable = Some(tar[start..end].to_vec());
        }
        offset = start
            .checked_add(
                size.div_ceil(512)
                    .checked_mul(512)
                    .ok_or_else(|| fail("Tar size overflow"))?,
            )
            .ok_or_else(|| fail("Tar offset overflow"))?;
    }
    executable.ok_or_else(|| fail("Release has no Landstrip executable"))
}
async fn download_landstrip() -> Result<Vec<u8>> {
    let (platform, arch) = release_platform(env::consts::OS, env::consts::ARCH)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent("margatroid-installer")
        .build()?;
    let release: serde_json::Value = serde_json::from_slice(
        &client
            .get(format!(
                "https://api.github.com/repos/landstrip/landstrip/releases/tags/{VERSION}"
            ))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?,
    )?;
    let filename = format!("landstrip-{VERSION}-{platform}-{arch}.tar.gz");
    let asset = release["assets"]
        .as_array()
        .and_then(|assets| assets.iter().find(|asset| asset["name"] == filename))
        .ok_or_else(|| fail("Release asset missing"))?;
    let digest = asset["digest"]
        .as_str()
        .filter(|value| value.starts_with("sha256:"))
        .ok_or_else(|| fail("Release SHA256 missing"))?;
    let url = asset["browser_download_url"]
        .as_str()
        .filter(|value| {
            value.starts_with("https://github.com/landstrip/landstrip/releases/download/")
        })
        .ok_or_else(|| fail("Unexpected release URL"))?;
    let archive = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    if format!("sha256:{:x}", Sha256::digest(&archive)) != digest {
        return Err(fail("Landstrip release checksum mismatch"));
    }
    release_executable(&archive)
}
async fn install() -> Result<()> {
    let mut args = env::args().skip(1);
    let root = normalized(&PathBuf::from(
        args.next().ok_or_else(|| fail("Missing checkout path"))?,
    ))?;
    let flags = args.collect::<Vec<_>>();
    if flags
        .iter()
        .any(|flag| !matches!(flag.as_str(), "--skip-landstrip" | "--update-resources"))
    {
        return Err(fail("Unknown installer option"));
    }
    let skip_landstrip = flags.iter().any(|flag| flag == "--skip-landstrip");
    let update = flags.iter().any(|flag| flag == "--update-resources");
    let home_variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = normalized(&PathBuf::from(
        env::var_os(home_variable).ok_or_else(|| fail(format!("{home_variable} is not set")))?,
    ))?;
    let bin = normalized(&PathBuf::from(
        env::var_os("MARGATROID_BIN_DIR")
            .unwrap_or_else(|| home.join(".local/bin").into_os_string()),
    ))?;
    let data = home.join(".margatroid");
    let roots = [bin.as_path(), data.as_path()];
    for path in [&bin, &data] {
        reject_symlinks(path)?;
    }
    for name in BINARIES {
        let path = root.join("target/release").join(executable_name(name));
        if !path.is_file() {
            return Err(fail(format!("Missing build: {}", path.display())));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(&path)?.permissions().mode() & 0o111 == 0 {
                return Err(fail(format!("Build is not executable: {}", path.display())));
            }
        }
    }
    for path in [
        "ui/dist/index.html",
        "resources/agent-drivers/margatroid/assistant/dev/ui/index.html",
        "resources/agent-drivers/margatroid/assistant/dev/subagent-ui/index.html",
    ] {
        if !root.join(path).is_file() {
            return Err(fail(format!("Missing precompiled UI: {path}")));
        }
    }
    let landstrip = if skip_landstrip {
        None
    } else {
        println!("Downloading and verifying Landstrip {VERSION}...");
        Some(download_landstrip().await?)
    };
    if let Some(bytes) = landstrip {
        let target = bin.join(executable_name("landstrip"));
        install_bytes(&target, &bytes, 0o755, &roots)?;
        println!("Checking Landstrip...");
        let doctor_args = if cfg!(target_os = "linux") {
            vec!["doctor", "--backend", "container"]
        } else {
            vec!["doctor"]
        };
        for args in [vec!["--version"], doctor_args] {
            if !Command::new(&target)
                .args(args)
                .stdout(std::process::Stdio::null())
                .status()?
                .success()
            {
                return Err(fail("Installed Landstrip failed validation"));
            }
        }
    }
    println!("Installing executables...");
    for name in BINARIES {
        install_bytes(
            &bin.join(executable_name(name)),
            &fs::read(root.join("target/release").join(executable_name(name)))?,
            0o755,
            &roots,
        )?;
    }
    for kind in [
        "workspaces",
        "agent-drivers",
        "prompts",
        "sandboxes",
        "tools",
    ] {
        let path = data.join(kind);
        reject_symlinks(&path)?;
        fs::create_dir_all(path)?;
    }
    println!("Installing resources...");
    let mut counts = (0, 0);
    for kind in ["agent-drivers", "prompts", "sandboxes", "tools"] {
        copy_resources(
            &root.join("resources").join(kind),
            &data.join(kind),
            update,
            &roots,
            &mut counts,
        )?;
    }
    for (resource, binary) in [
        ("bash", "shell-client"),
        ("bash-exec", "shell-client"),
        ("mcp-register", "mcp-client"),
    ] {
        install_bytes(
            &data
                .join("tools/margatroid")
                .join(resource)
                .join("dev/bin")
                .join(executable_name(binary)),
            &fs::read(root.join("target/release").join(executable_name(binary)))?,
            0o755,
            &roots,
        )?;
    }
    for (name,text) in [("config.toml","[server]\nbind = \"127.0.0.1:3939\"\n"),("models.toml","# Replace model, API prefix and credentials before inference.\n[[models]]\nid = \"deepseek-v4.1-flash\"\nprovider = \"openai\"\napi_type = \"responses\"\nbase_url = \"https://api.openai.com/v1\"\nmodel = \"YOUR_MODEL\"\napi_key = \"YOUR_API_KEY\"\n")] {
        let target=data.join(name);reject_symlinks(&target)?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&target) {Ok(mut file)=>{file.write_all(text.as_bytes())?;file.sync_all()?;},Err(error)if error.kind()==std::io::ErrorKind::AlreadyExists=>(),Err(error)=>return Err(error.into()),}
    }
    println!("Installation complete.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    fn archive(size: usize, payload: &[u8], corrupt: bool) -> Vec<u8> {
        let mut header = [0u8; 512];
        header[..9].copy_from_slice(b"landstrip");
        header[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
        header[156] = b'0';
        header[148..156].fill(b' ');
        let checksum: usize = header.iter().map(|byte| usize::from(*byte)).sum();
        header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
        if corrupt {
            header[0] ^= 1;
        }
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&header).unwrap();
        encoder.write_all(payload).unwrap();
        encoder.finish().unwrap()
    }
    #[test]
    fn release_platform_supports_six_published_targets() {
        for (os, platform) in [
            ("linux", "linux"),
            ("macos", "darwin"),
            ("windows", "win32"),
        ] {
            for (arch, release_arch) in [("x86_64", "x64"), ("aarch64", "arm64")] {
                assert_eq!(
                    release_platform(os, arch).unwrap(),
                    (platform, release_arch)
                );
            }
        }
        assert!(release_platform("freebsd", "x86_64").is_err());
        assert!(release_platform("linux", "riscv64").is_err());
    }
    #[test]
    fn release_extraction_reads_only_binary_bytes() {
        assert_eq!(
            release_executable(&archive(4, b"test", false)).unwrap(),
            b"test"
        );
    }
    #[test]
    fn release_extraction_rejects_corrupt_headers_and_truncation() {
        assert!(release_executable(&archive(4, b"test", true)).is_err());
        assert!(release_executable(&archive(10, b"test", false)).is_err());
    }
}
