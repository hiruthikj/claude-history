use crate::error::{AppError, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "raine/claude-history";
const BIN_NAME: &str = "claude-history";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Map OS/arch to the release artifact suffix used in GitHub releases.
fn platform_suffix() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("macos", "x86_64") => Ok("darwin-amd64"),
        ("linux", "x86_64") => Ok("linux-amd64"),
        (os, arch) => Err(AppError::UpdateError(format!(
            "Unsupported platform: {os}/{arch}"
        ))),
    }
}

/// Check if the binary is managed by Homebrew.
fn is_homebrew_install(exe_path: &Path) -> bool {
    let path_str = exe_path.to_string_lossy();
    path_str.contains("/Cellar/")
}

/// Fetch the latest release tag from GitHub API using curl.
fn fetch_latest_version() -> Result<String> {
    let output = Command::new("curl")
        .args([
            "-sSf",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        ])
        .output()
        .map_err(|e| AppError::UpdateError(format!("Failed to run curl: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::UpdateError(format!(
            "Failed to fetch latest release: {}",
            stderr.trim()
        )));
    }

    let body: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| AppError::UpdateError(format!("Failed to parse GitHub API response: {e}")))?;

    let tag = body["tag_name"]
        .as_str()
        .ok_or_else(|| AppError::UpdateError("No tag_name in GitHub API response".to_string()))?;

    Ok(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

/// Download a URL to a file path using curl.
fn download(url: &str, dest: &Path) -> Result<()> {
    let status = Command::new("curl")
        .args([
            "-sSLf",
            "--connect-timeout",
            "10",
            "--max-time",
            "120",
            "-o",
        ])
        .arg(dest)
        .arg(url)
        .status()
        .map_err(|e| AppError::UpdateError(format!("Failed to run curl: {e}")))?;

    if !status.success() {
        return Err(AppError::UpdateError(format!("Download failed: {url}")));
    }
    Ok(())
}

/// Extract a tar.gz archive into a directory.
fn extract_tar(archive: &Path, dest: &Path) -> Result<()> {
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()
        .map_err(|e| AppError::UpdateError(format!("Failed to run tar: {e}")))?;

    if !status.success() {
        return Err(AppError::UpdateError(
            "Failed to extract archive".to_string(),
        ));
    }
    Ok(())
}

/// Compute SHA-256 hash of a file using system tools.
fn sha256_of(path: &Path) -> Result<String> {
    // Try sha256sum first (common on Linux)
    if let Ok(output) = Command::new("sha256sum").arg(path).output()
        && output.status.success()
    {
        let out = String::from_utf8_lossy(&output.stdout);
        if let Some(hash) = out.split_whitespace().next() {
            return Ok(hash.to_string());
        }
    }

    // Fall back to shasum -a 256 (macOS)
    let output = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .map_err(|e| {
            AppError::UpdateError(format!(
                "Neither sha256sum nor shasum found. Cannot verify checksum: {e}"
            ))
        })?;

    if !output.status.success() {
        return Err(AppError::UpdateError("Checksum command failed".to_string()));
    }

    let out = String::from_utf8_lossy(&output.stdout);
    out.split_whitespace()
        .next()
        .map(|s| s.to_string())
        .ok_or_else(|| AppError::UpdateError("Could not parse checksum output".to_string()))
}

/// Verify SHA-256 checksum of a file against the expected checksum line.
fn verify_checksum(file: &Path, expected_line: &str) -> Result<()> {
    let expected_hash = expected_line
        .split_whitespace()
        .next()
        .ok_or_else(|| AppError::UpdateError("Invalid checksum file format".to_string()))?;

    let actual_hash = sha256_of(file)?;
    if actual_hash != expected_hash {
        return Err(AppError::UpdateError(format!(
            "Checksum mismatch!\n  Expected: {expected_hash}\n  Got:      {actual_hash}"
        )));
    }
    Ok(())
}

#[derive(Debug)]
struct SupportInstallation {
    transaction_dir: tempfile::TempDir,
    exe_dir: PathBuf,
    dest_lib_dir: PathBuf,
    old_lib_dir: Option<PathBuf>,
    old_links: Vec<RuntimeLinkBackup>,
}

#[derive(Debug)]
struct RuntimeLinkBackup {
    link: PathBuf,
    backup: Option<PathBuf>,
}

impl SupportInstallation {
    fn commit(self) {
        if let Some(old_lib_dir) = self.old_lib_dir {
            let _ = remove_path(&old_lib_dir);
        }
        for old_link in self.old_links {
            if let Some(backup) = old_link.backup {
                let _ = remove_path(&backup);
            }
        }
    }

    fn rollback(self) -> Result<()> {
        let mut errors = Vec::new();

        for old_link in self.old_links.iter().rev() {
            if path_exists(&old_link.link)
                && let Err(error) = remove_path(&old_link.link)
            {
                errors.push(format!(
                    "failed to remove {}: {error}",
                    old_link.link.display()
                ));
                continue;
            }
            if let Some(backup) = &old_link.backup
                && let Err(error) = std::fs::rename(backup, &old_link.link)
            {
                errors.push(format!(
                    "failed to restore {}: {error}",
                    old_link.link.display()
                ));
            }
        }

        if path_exists(&self.dest_lib_dir)
            && let Err(error) = remove_path(&self.dest_lib_dir)
        {
            errors.push(format!(
                "failed to remove {}: {error}",
                self.dest_lib_dir.display()
            ));
        }
        if let Some(old_lib_dir) = &self.old_lib_dir
            && let Err(error) = std::fs::rename(old_lib_dir, &self.dest_lib_dir)
        {
            errors.push(format!(
                "failed to restore {}: {error}",
                self.dest_lib_dir.display()
            ));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(AppError::UpdateError(format!(
                "Support-file rollback failed: {}",
                errors.join("; ")
            )))
        }
    }
}

fn install_support_files(extract_dir: &Path, current_exe: &Path) -> Result<SupportInstallation> {
    let exe_dir = current_exe
        .parent()
        .ok_or_else(|| AppError::UpdateError("Could not determine binary directory".to_string()))?
        .to_path_buf();
    let transaction_dir = tempfile::Builder::new()
        .prefix(&format!(".{BIN_NAME}.update-"))
        .tempdir_in(&exe_dir)
        .map_err(|e| {
            AppError::UpdateError(format!("Failed to create update staging directory: {e}"))
        })?;
    let dest_lib_dir = exe_dir.join("lib");
    let mut installation = SupportInstallation {
        transaction_dir,
        exe_dir,
        dest_lib_dir,
        old_lib_dir: None,
        old_links: Vec::new(),
    };

    let lib_dir = extract_dir.join("lib");
    if !path_exists(&lib_dir) {
        if cfg!(feature = "release-dynamic-ort") {
            return Err(AppError::UpdateError(
                "Release archive does not contain a bundled ONNX Runtime library directory"
                    .to_string(),
            ));
        }
        return Ok(installation);
    }

    let staged_lib_dir = installation.transaction_dir.path().join("lib");
    std::fs::create_dir(&staged_lib_dir).map_err(|e| {
        AppError::UpdateError(format!("Failed to create library staging directory: {e}"))
    })?;
    copy_support_files(&lib_dir, &staged_lib_dir)?;
    if cfg!(feature = "release-dynamic-ort") {
        ensure_runtime_library(&staged_lib_dir)?;
    }

    if path_exists(&installation.dest_lib_dir) {
        let old_lib_dir = installation.transaction_dir.path().join("old-lib");
        std::fs::rename(&installation.dest_lib_dir, &old_lib_dir).map_err(|e| {
            AppError::UpdateError(format!("Failed to stage existing library directory: {e}"))
        })?;
        installation.old_lib_dir = Some(old_lib_dir);
    }
    if let Err(error) = std::fs::rename(&staged_lib_dir, &installation.dest_lib_dir) {
        let _ = installation.rollback();
        return Err(AppError::UpdateError(format!(
            "Failed to install library directory: {error}"
        )));
    }

    if let Err(error) = install_runtime_links(&mut installation) {
        let rollback_error = installation.rollback().err();
        return Err(match rollback_error {
            Some(rollback_error) => AppError::UpdateError(format!("{error}; {rollback_error}")),
            None => error,
        });
    }

    Ok(installation)
}

fn copy_support_files(source_dir: &Path, dest_dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(source_dir)
        .map_err(|e| AppError::UpdateError(format!("Failed to read library directory: {e}")))?
    {
        let entry = entry
            .map_err(|e| AppError::UpdateError(format!("Failed to read library entry: {e}")))?;
        let file_type = entry
            .file_type()
            .map_err(|e| AppError::UpdateError(format!("Failed to inspect library entry: {e}")))?;
        let destination = dest_dir.join(entry.file_name());
        if file_type.is_file() {
            std::fs::copy(entry.path(), &destination)
                .map_err(|e| AppError::UpdateError(format!("Failed to stage library: {e}")))?;
        } else if file_type.is_symlink() {
            copy_support_symlink(&entry.path(), &destination)?;
        }
    }
    Ok(())
}

fn copy_support_symlink(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let target = std::fs::read_link(source)
            .map_err(|e| AppError::UpdateError(format!("Failed to read library symlink: {e}")))?;
        if target.is_absolute() {
            return Err(AppError::UpdateError(format!(
                "Refusing absolute library symlink target: {}",
                target.display()
            )));
        }
        symlink(&target, destination)
            .map_err(|e| AppError::UpdateError(format!("Failed to stage library symlink: {e}")))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (source, destination);
    }
    Ok(())
}

fn runtime_library_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "libonnxruntime.dylib",
        "windows" => "onnxruntime.dll",
        _ => "libonnxruntime.so",
    }
}

fn ensure_runtime_library(lib_dir: &Path) -> Result<()> {
    let name = runtime_library_name();
    let path = lib_dir.join(name);
    if path.is_file() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let mut candidates = std::fs::read_dir(lib_dir)
            .map_err(|e| AppError::UpdateError(format!("Failed to read staged libraries: {e}")))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|candidate| is_versioned_runtime_name(candidate, name))
            .collect::<Vec<_>>();
        candidates.sort();
        if let Some(candidate) = candidates.into_iter().find(|candidate| candidate.is_file()) {
            let target = candidate.file_name().ok_or_else(|| {
                AppError::UpdateError("Failed to determine staged library name".to_string())
            })?;
            symlink(target, &path).map_err(|e| {
                AppError::UpdateError(format!("Failed to create bundled library symlink: {e}"))
            })?;
            if path.is_file() {
                return Ok(());
            }
        }
    }

    Err(AppError::UpdateError(format!(
        "Release archive does not contain a usable {name}"
    )))
}

fn is_versioned_runtime_name(path: &Path, unversioned_name: &str) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    match unversioned_name {
        "libonnxruntime.dylib" => {
            file_name.starts_with("libonnxruntime.")
                && file_name.ends_with(".dylib")
                && file_name != unversioned_name
        }
        "onnxruntime.dll" => false,
        _ => file_name.starts_with("libonnxruntime.so.") && file_name != unversioned_name,
    }
}

fn install_runtime_links(installation: &mut SupportInstallation) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        for name in ["libonnxruntime.so", "libonnxruntime.dylib"] {
            let link = installation.exe_dir.join(name);
            let backup = installation
                .transaction_dir
                .path()
                .join(format!("old-{name}"));
            let backup = if path_exists(&link) {
                std::fs::rename(&link, &backup).map_err(|e| {
                    AppError::UpdateError(format!("Failed to stage library symlink: {e}"))
                })?;
                Some(backup)
            } else {
                None
            };
            installation.old_links.push(RuntimeLinkBackup {
                link: link.clone(),
                backup,
            });

            if installation.dest_lib_dir.join(name).is_file() {
                let staged_link = installation
                    .transaction_dir
                    .path()
                    .join(format!("new-{name}"));
                symlink(Path::new("lib").join(name), &staged_link).map_err(|e| {
                    AppError::UpdateError(format!("Failed to create library symlink: {e}"))
                })?;
                std::fs::rename(staged_link, link).map_err(|e| {
                    AppError::UpdateError(format!("Failed to install library symlink: {e}"))
                })?;
            }
        }
    }
    Ok(())
}

fn path_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Replace the current binary with the new one, with rollback on failure.
fn replace_binary(new_binary: &Path, current_exe: &Path) -> Result<()> {
    let exe_dir = current_exe
        .parent()
        .ok_or_else(|| AppError::UpdateError("Could not determine binary directory".to_string()))?;

    // Copy to destination directory to avoid EXDEV (cross-device rename)
    let staged = exe_dir.join(format!(".{BIN_NAME}.new"));
    if path_exists(&staged) {
        remove_path(&staged).map_err(|e| {
            AppError::UpdateError(format!("Failed to clear stale staged binary: {e}"))
        })?;
    }
    std::fs::copy(new_binary, &staged)
        .map_err(|e| AppError::UpdateError(format!("Failed to copy new binary: {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| AppError::UpdateError(format!("Failed to set permissions: {e}")))?;
    }

    // Rename current -> .old, then staged -> current
    let backup = exe_dir.join(format!(".{BIN_NAME}.old"));
    if path_exists(&backup) {
        remove_path(&backup).map_err(|e| {
            AppError::UpdateError(format!("Failed to clear stale binary backup: {e}"))
        })?;
    }
    std::fs::rename(current_exe, &backup)
        .map_err(|e| AppError::UpdateError(format!("Failed to move current binary aside: {e}")))?;

    if let Err(e) = std::fs::rename(&staged, current_exe) {
        // Rollback: restore the original
        let _ = std::fs::rename(&backup, current_exe);
        return Err(AppError::UpdateError(format!(
            "Failed to install new binary (rolled back): {e}"
        )));
    }

    // Cleanup
    let _ = remove_path(&backup);
    Ok(())
}

fn do_update(
    pb: &indicatif::ProgressBar,
    artifact_name: &str,
    current_exe: &Path,
) -> Result<String> {
    let latest_version = fetch_latest_version()?;

    if latest_version == CURRENT_VERSION {
        return Ok(format!("Already up to date (v{CURRENT_VERSION})"));
    }

    pb.set_message(format!("Downloading v{latest_version}..."));

    let tmp = tempfile::tempdir()
        .map_err(|e| AppError::UpdateError(format!("Failed to create temp directory: {e}")))?;
    let tar_path = tmp.path().join(format!("{artifact_name}.tar.gz"));
    let sha_path = tmp.path().join(format!("{artifact_name}.sha256"));

    let base_url = format!("https://github.com/{REPO}/releases/download/v{latest_version}");

    download(&format!("{base_url}/{artifact_name}.tar.gz"), &tar_path)?;
    download(&format!("{base_url}/{artifact_name}.sha256"), &sha_path)?;

    pb.set_message("Verifying checksum...");
    let sha_content = std::fs::read_to_string(&sha_path)
        .map_err(|e| AppError::UpdateError(format!("Failed to read checksum file: {e}")))?;
    verify_checksum(&tar_path, &sha_content)?;

    pb.set_message("Installing...");
    let extract_dir = tmp.path().join("extract");
    std::fs::create_dir(&extract_dir)
        .map_err(|e| AppError::UpdateError(format!("Failed to create extract dir: {e}")))?;
    extract_tar(&tar_path, &extract_dir)?;

    let new_binary = extract_dir.join(BIN_NAME);
    if !new_binary.exists() {
        return Err(AppError::UpdateError(format!(
            "Extracted archive does not contain '{BIN_NAME}' binary"
        )));
    }

    let support_installation = install_support_files(&extract_dir, current_exe)?;
    if let Err(error) = replace_binary(&new_binary, current_exe) {
        let rollback_error = support_installation.rollback().err();
        return Err(match rollback_error {
            Some(rollback_error) => AppError::UpdateError(format!("{error}; {rollback_error}")),
            None => error,
        });
    }
    support_installation.commit();

    Ok(format!(
        "Updated {BIN_NAME} v{CURRENT_VERSION} -> v{latest_version}"
    ))
}

pub fn run() -> Result<()> {
    let current_exe = std::env::current_exe()
        .map_err(|e| AppError::UpdateError(format!("Could not determine executable path: {e}")))?;

    // Guard: Homebrew-managed installs (canonicalize to resolve symlinks)
    let canonical_exe = std::fs::canonicalize(&current_exe).unwrap_or(current_exe.clone());
    if is_homebrew_install(&canonical_exe) {
        return Err(AppError::UpdateError(
            "claude-history is managed by Homebrew. Run `brew upgrade claude-history` instead."
                .to_string(),
        ));
    }

    let platform = platform_suffix()?;
    let artifact_name = format!("{BIN_NAME}-{platform}");

    let pb = indicatif::ProgressBar::new_spinner();
    pb.set_style(
        indicatif::ProgressStyle::default_spinner()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
            .template("{spinner:.blue} {msg}")
            .unwrap(),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(120));
    pb.set_message("Checking for updates...");

    match do_update(&pb, &artifact_name, &canonical_exe) {
        Ok(msg) => {
            pb.finish_with_message(format!("✔ {msg}"));
            Ok(())
        }
        Err(e) => {
            pb.finish_with_message("✘ Update failed".to_string());
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_suffix_current() {
        let suffix = platform_suffix().unwrap();
        assert!(["darwin-arm64", "darwin-amd64", "linux-amd64"].contains(&suffix));
    }

    #[test]
    fn test_is_homebrew_cellar() {
        assert!(is_homebrew_install(Path::new(
            "/opt/homebrew/Cellar/claude-history/0.1.42/bin/claude-history"
        )));
    }

    #[test]
    fn test_is_homebrew_prefix() {
        assert!(is_homebrew_install(Path::new(
            "/usr/local/Cellar/claude-history/0.1.42/bin/claude-history"
        )));
    }

    #[test]
    fn test_is_not_homebrew_local_bin() {
        assert!(!is_homebrew_install(Path::new(
            "/usr/local/bin/claude-history"
        )));
    }

    #[test]
    fn test_is_not_homebrew_home() {
        assert!(!is_homebrew_install(Path::new(
            "/home/user/.local/bin/claude-history"
        )));
    }

    #[cfg(unix)]
    #[test]
    fn binary_replacement_does_not_follow_stale_staged_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let current_exe = temp.path().join(BIN_NAME);
        let new_binary = temp.path().join("new-binary");
        let sentinel = temp.path().join("sentinel");
        let staged = temp.path().join(format!(".{BIN_NAME}.new"));
        std::fs::write(&current_exe, b"old binary").unwrap();
        std::fs::write(&new_binary, b"new binary").unwrap();
        std::fs::write(&sentinel, b"sentinel").unwrap();
        symlink(&sentinel, &staged).unwrap();

        replace_binary(&new_binary, &current_exe).unwrap();

        assert_eq!(std::fs::read(&current_exe).unwrap(), b"new binary");
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"sentinel");
    }

    #[cfg(unix)]
    #[test]
    fn support_installation_preserves_symlinks_and_removes_stale_runtime() {
        use std::fs::symlink_metadata;
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let current_exe = temp.path().join(BIN_NAME);
        std::fs::write(&current_exe, b"old binary").unwrap();

        let old_lib = temp.path().join("lib");
        std::fs::create_dir(&old_lib).unwrap();
        let runtime_name = runtime_library_name();
        let old_versioned_name = if runtime_name == "libonnxruntime.dylib" {
            "libonnxruntime.1.23.0.dylib"
        } else {
            "libonnxruntime.so.1.23.0"
        };
        std::fs::write(old_lib.join(old_versioned_name), b"old runtime").unwrap();
        symlink(old_versioned_name, old_lib.join(runtime_name)).unwrap();
        symlink(
            Path::new("lib").join(runtime_name),
            temp.path().join(runtime_name),
        )
        .unwrap();

        let extract_dir = temp.path().join("extract");
        let new_lib = extract_dir.join("lib");
        std::fs::create_dir_all(&new_lib).unwrap();
        let new_versioned_name = if runtime_name == "libonnxruntime.dylib" {
            "libonnxruntime.1.24.2.dylib"
        } else {
            "libonnxruntime.so.1.24.2"
        };
        std::fs::write(new_lib.join(new_versioned_name), b"new runtime").unwrap();
        symlink(new_versioned_name, new_lib.join(runtime_name)).unwrap();

        let installation = install_support_files(&extract_dir, &current_exe).unwrap();
        assert!(!old_lib.join(old_versioned_name).exists());
        assert_eq!(
            std::fs::read_link(temp.path().join(runtime_name)).unwrap(),
            Path::new("lib").join(runtime_name)
        );
        assert!(
            symlink_metadata(temp.path().join(runtime_name))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_link(old_lib.join(runtime_name)).unwrap(),
            Path::new(new_versioned_name)
        );
        installation.commit();
    }

    #[cfg(unix)]
    #[test]
    fn support_installation_rolls_back_library_and_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let current_exe = temp.path().join(BIN_NAME);
        std::fs::write(&current_exe, b"old binary").unwrap();

        let old_lib = temp.path().join("lib");
        std::fs::create_dir(&old_lib).unwrap();
        let runtime_name = runtime_library_name();
        let old_versioned_name = if runtime_name == "libonnxruntime.dylib" {
            "libonnxruntime.1.23.0.dylib"
        } else {
            "libonnxruntime.so.1.23.0"
        };
        std::fs::write(old_lib.join(old_versioned_name), b"old runtime").unwrap();
        symlink(old_versioned_name, old_lib.join(runtime_name)).unwrap();
        symlink(
            Path::new("lib").join(runtime_name),
            temp.path().join(runtime_name),
        )
        .unwrap();

        let extract_dir = temp.path().join("extract");
        let new_lib = extract_dir.join("lib");
        std::fs::create_dir_all(&new_lib).unwrap();
        let new_versioned_name = if runtime_name == "libonnxruntime.dylib" {
            "libonnxruntime.1.24.2.dylib"
        } else {
            "libonnxruntime.so.1.24.2"
        };
        std::fs::write(new_lib.join(new_versioned_name), b"new runtime").unwrap();
        symlink(new_versioned_name, new_lib.join(runtime_name)).unwrap();

        let installation = install_support_files(&extract_dir, &current_exe).unwrap();
        installation.rollback().unwrap();

        assert_eq!(std::fs::read(current_exe).unwrap(), b"old binary");
        assert_eq!(
            std::fs::read(old_lib.join(old_versioned_name)).unwrap(),
            b"old runtime"
        );
        assert_eq!(
            std::fs::read_link(old_lib.join(runtime_name)).unwrap(),
            Path::new(old_versioned_name)
        );
        assert_eq!(
            std::fs::read_link(temp.path().join(runtime_name)).unwrap(),
            Path::new("lib").join(runtime_name)
        );
    }

    #[cfg(feature = "release-dynamic-ort")]
    #[test]
    fn release_support_installation_requires_a_usable_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let current_exe = temp.path().join(BIN_NAME);
        std::fs::write(&current_exe, b"old binary").unwrap();
        let extract_dir = temp.path().join("extract");
        std::fs::create_dir_all(extract_dir.join("lib")).unwrap();

        let error = install_support_files(&extract_dir, &current_exe).unwrap_err();
        assert!(error.to_string().contains("usable"));
        assert_eq!(std::fs::read(current_exe).unwrap(), b"old binary");
    }

    #[cfg(not(feature = "release-dynamic-ort"))]
    #[test]
    fn non_release_update_allows_archives_without_support_files() {
        let temp = tempfile::tempdir().unwrap();
        let current_exe = temp.path().join(BIN_NAME);
        std::fs::write(&current_exe, b"old binary").unwrap();
        let extract_dir = temp.path().join("extract");
        std::fs::create_dir(&extract_dir).unwrap();

        let installation = install_support_files(&extract_dir, &current_exe).unwrap();
        installation.commit();
        assert_eq!(std::fs::read(current_exe).unwrap(), b"old binary");
    }
}
