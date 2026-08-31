use std::ffi::OsStr;
use std::fs::{self, DirBuilder};
use std::io;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const RUNTIME_PREFIX: &str = "wezterm-sshmgr-";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeContext {
    pub runtime_dir: PathBuf,
    pub token: String,
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

pub(crate) fn random_request_id() -> String {
    encode_hex(&rand::random::<[u8; 16]>())
}

pub(crate) fn random_backup_id() -> String {
    encode_hex(&rand::random::<[u8; 12]>())
}

pub fn is_valid_token(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn is_request_filename(name: &str) -> bool {
    let Some(body) = name
        .strip_prefix("request-")
        .and_then(|name| name.strip_suffix(".json"))
    else {
        return false;
    };
    let Some((sequence, nonce)) = body.split_once('-') else {
        return false;
    };
    !sequence.is_empty()
        && !sequence.starts_with('0')
        && sequence.bytes().all(|byte| byte.is_ascii_digit())
        && nonce.len() == 32
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Resolve a path lexically, like Node's `path.resolve`, without requiring the
/// target to exist or following symlinks.
pub fn resolve_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    let path = path.as_ref();
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .context("cannot read the current directory")?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    let builder = &mut DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

#[cfg(unix)]
fn directory_is_private(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o077 == 0
}

#[cfg(not(unix))]
fn directory_is_private(_metadata: &fs::Metadata) -> bool {
    true
}

pub fn create_runtime() -> Result<RuntimeContext> {
    let temporary_root = std::env::temp_dir();
    for _ in 0..128 {
        let runtime_dir = temporary_root.join(format!("{RUNTIME_PREFIX}{}", random_backup_id()));
        match create_private_directory(&runtime_dir) {
            Ok(()) => {
                return Ok(RuntimeContext {
                    runtime_dir,
                    token: encode_hex(&rand::random::<[u8; 32]>()),
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).context("cannot create the sshmgr runtime directory");
            }
        }
    }
    bail!("cannot allocate a unique sshmgr runtime directory")
}

pub fn validate_runtime(snapshot_path: impl AsRef<Path>, token: &str) -> Result<PathBuf> {
    if !is_valid_token(token) {
        bail!("invalid sshmgr TUI session token");
    }
    let snapshot_path = resolve_path(snapshot_path)?;
    if snapshot_path.file_name() != Some(OsStr::new("snapshot.json")) {
        bail!("snapshot must be named snapshot.json");
    }
    let snapshot_metadata = fs::metadata(&snapshot_path).context("snapshot not found")?;
    if !snapshot_metadata.is_file() {
        bail!("snapshot not found");
    }
    let runtime_dir = snapshot_path
        .parent()
        .context("snapshot has no runtime directory")?
        .to_owned();
    if !runtime_dir
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.starts_with(RUNTIME_PREFIX))
    {
        bail!("invalid runtime directory");
    }
    let runtime_metadata = fs::metadata(&runtime_dir).context("invalid runtime directory")?;
    if !runtime_metadata.is_dir() {
        bail!("invalid runtime directory");
    }
    if !directory_is_private(&runtime_metadata) {
        bail!("runtime directory is not private");
    }
    Ok(runtime_dir)
}

pub fn cleanup_runtime(runtime_dir: impl AsRef<Path>) -> Result<()> {
    let runtime_dir = resolve_path(runtime_dir)?;
    let metadata = match fs::metadata(&runtime_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("cannot inspect runtime directory"),
    };
    if !metadata.is_dir()
        || !runtime_dir
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.starts_with(RUNTIME_PREFIX))
        || !directory_is_private(&metadata)
    {
        return Ok(());
    }

    for entry in fs::read_dir(&runtime_dir).context("cannot read runtime directory")? {
        let entry = entry.context("cannot read runtime entry")?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let owned = name == "snapshot.json"
            || name.starts_with("snapshot.json.tmp-")
            || name.starts_with("snapshot.json.backup-")
            || is_request_filename(&name);
        if owned {
            match fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("cannot remove runtime file"),
            }
        }
    }
    // A caller may deliberately keep unrelated files in the directory. Node's
    // `rmdir(...).catch(...)` leaves such a directory in place without error.
    let _ = fs::remove_dir(&runtime_dir);
    Ok(())
}

pub fn replace_file(source: impl AsRef<Path>, destination: impl AsRef<Path>) -> Result<()> {
    let source = resolve_path(source)?;
    let destination = resolve_path(destination)?;
    let first_error = match fs::rename(&source, &destination) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };

    let backup = destination.with_file_name(format!(
        "{}.backup-{}",
        destination
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("snapshot.json"),
        random_backup_id()
    ));
    if fs::rename(&destination, &backup).is_err() {
        return Err(first_error).context("cannot replace destination");
    }
    match fs::rename(&source, &destination) {
        Ok(()) => {
            fs::remove_file(&backup).context("cannot remove replaced-file backup")?;
            Ok(())
        }
        Err(replacement_error) => match fs::rename(&backup, &destination) {
            Ok(()) => Err(replacement_error).context("cannot replace destination"),
            Err(restore_error) => bail!(
                "cannot replace {}; original snapshot remains at {}: {restore_error}",
                destination.display(),
                backup.display()
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn validates_token_and_request_names() {
        assert!(is_valid_token(&"a".repeat(64)));
        assert!(!is_valid_token(&"A".repeat(64)));
        assert!(is_request_filename(&format!(
            "request-1-{}.json",
            "b".repeat(32)
        )));
        assert!(!is_request_filename(&format!(
            "request-0-{}.json",
            "b".repeat(32)
        )));
    }

    #[test]
    fn creates_and_validates_runtime() {
        let runtime = create_runtime().expect("runtime");
        assert!(is_valid_token(&runtime.token));
        assert!(
            runtime
                .runtime_dir
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap()
                .starts_with(RUNTIME_PREFIX)
        );
        let snapshot = runtime.runtime_dir.join("snapshot.json");
        fs::write(&snapshot, "{}").unwrap();
        assert_eq!(
            validate_runtime(&snapshot, &runtime.token).unwrap(),
            runtime.runtime_dir
        );
        cleanup_runtime(&runtime.runtime_dir).unwrap();
        assert!(!runtime.runtime_dir.exists());
    }

    #[test]
    fn cleanup_only_removes_owned_files() {
        let runtime = create_runtime().unwrap();
        fs::write(runtime.runtime_dir.join("snapshot.json"), "{}").unwrap();
        fs::write(
            runtime
                .runtime_dir
                .join(format!("request-1-{}.json", "a".repeat(32))),
            "{}",
        )
        .unwrap();
        fs::write(runtime.runtime_dir.join("keep.txt"), "keep").unwrap();
        cleanup_runtime(&runtime.runtime_dir).unwrap();
        assert_eq!(fs::read_dir(&runtime.runtime_dir).unwrap().count(), 1);
        assert!(runtime.runtime_dir.join("keep.txt").exists());
        fs::remove_dir_all(runtime.runtime_dir).unwrap();
    }

    #[test]
    fn replaces_an_existing_file() {
        let runtime = create_runtime().unwrap();
        let source = runtime.runtime_dir.join("snapshot.json.tmp-test");
        let destination = runtime.runtime_dir.join("snapshot.json");
        fs::write(&source, "new").unwrap();
        fs::write(&destination, "old").unwrap();
        replace_file(&source, &destination).unwrap();
        assert_eq!(fs::read_to_string(&destination).unwrap(), "new");
        assert!(!source.exists());
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }
}
