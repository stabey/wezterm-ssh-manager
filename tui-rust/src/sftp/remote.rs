use std::cmp::Ordering;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use russh_sftp::client::SftpSession as RusshSftpSession;
use russh_sftp::client::error::Error as RusshSftpError;
use russh_sftp::client::fs::Metadata;

use super::error::{Result, SftpError, is_remote_not_found};
use super::types::{
    FileEntry, FileKind, FileProvider, MkdirOptions, OperationOptions, RemoveOptions,
};

#[derive(Clone)]
pub struct RemoteFileProvider {
    sftp: Arc<RusshSftpSession>,
}

impl std::fmt::Debug for RemoteFileProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("RemoteFileProvider").finish()
    }
}

impl RemoteFileProvider {
    pub fn new(sftp: Arc<RusshSftpSession>) -> Self {
        Self { sftp }
    }

    pub async fn list(
        &self,
        directory: &str,
        options: &OperationOptions,
    ) -> Result<Vec<FileEntry>> {
        let entries = remote_operation(
            options,
            "list remote directory",
            directory,
            self.sftp.read_dir(directory.to_owned()),
        )
        .await?;
        let mut entries = entries
            .map(|entry| {
                let name = entry.file_name();
                entry_from_metadata(entry.path(), name, entry.metadata())
            })
            .collect::<Vec<_>>();
        entries.sort_by(compare_entries);
        Ok(entries)
    }

    pub async fn stat(&self, path: &str, options: &OperationOptions) -> Result<FileEntry> {
        let metadata = remote_operation(
            options,
            "stat remote path",
            path,
            self.sftp.symlink_metadata(path.to_owned()),
        )
        .await?;
        Ok(entry_from_metadata(
            path.to_owned(),
            remote_basename(path),
            metadata,
        ))
    }

    pub async fn realpath(&self, path: &str, options: &OperationOptions) -> Result<String> {
        remote_operation(
            options,
            "resolve remote path",
            path,
            self.sftp.canonicalize(path.to_owned()),
        )
        .await
    }

    pub async fn mkdir(&self, path: &str, options: &MkdirOptions) -> Result<()> {
        if !options.recursive {
            self.mkdir_one(path, options).await?;
            return Ok(());
        }

        let normalized = normalize_remote_path(path);
        let absolute = normalized.starts_with('/');
        let mut current = if absolute {
            "/".to_owned()
        } else {
            String::new()
        };
        for part in normalized.split('/').filter(|part| !part.is_empty()) {
            options.operation.throw_if_cancelled()?;
            current = if current == "/" {
                format!("/{part}")
            } else if current.is_empty() {
                part.to_owned()
            } else {
                format!("{current}/{part}")
            };
            match self.mkdir_one(&current, options).await {
                Ok(()) => {}
                Err(create_error) => match self.stat(&current, &options.operation).await {
                    Ok(entry) if entry.kind == FileKind::Directory => {}
                    _ => return Err(create_error),
                },
            }
        }
        Ok(())
    }

    pub async fn rename(&self, from: &str, to: &str, options: &OperationOptions) -> Result<()> {
        remote_mutation(
            options,
            "rename remote path",
            from,
            self.sftp.rename(from.to_owned(), to.to_owned()),
        )
        .await
    }

    /// Replace a path using one atomic rename when the server permits overwrite.
    /// SFTP v3 has no portable overwrite flag, so servers that reject that
    /// rename fall back to a rollback-safe sibling backup sequence.
    pub async fn replace(&self, from: &str, to: &str, options: &OperationOptions) -> Result<()> {
        options.throw_if_cancelled()?;
        match self.sftp.rename(from.to_owned(), to.to_owned()).await {
            Ok(()) => Ok(()),
            Err(first_error) => {
                options.throw_if_cancelled()?;
                let backup = remote_join(
                    &remote_dirname(to),
                    &format!(".{}.sshmgr-{}.backup", remote_basename(to), unique_id()),
                );
                let mut backed_up = false;
                match self.sftp.rename(to.to_owned(), backup.clone()).await {
                    Ok(()) => backed_up = true,
                    Err(error) if is_remote_not_found(&error) => {}
                    Err(error) => {
                        return Err(SftpError::remote_operation(
                            "back up remote destination",
                            to,
                            error,
                        ));
                    }
                }

                match self.sftp.rename(from.to_owned(), to.to_owned()).await {
                    Ok(()) => {
                        if backed_up {
                            let _ = self.sftp.remove_file(backup).await;
                        }
                        Ok(())
                    }
                    Err(replacement_error) => {
                        if backed_up
                            && let Err(restore_error) =
                                self.sftp.rename(backup.clone(), to.to_owned()).await
                        {
                            return Err(SftpError::ReplaceFailed {
                                path: to.to_owned(),
                                backup,
                                detail: format!(
                                    "replacement: {replacement_error}; restore: {restore_error}"
                                ),
                            });
                        }
                        Err(SftpError::remote_operation(
                            "replace remote path",
                            to,
                            format!("{replacement_error}; initial rename: {first_error}"),
                        ))
                    }
                }
            }
        }
    }

    pub async fn remove(&self, path: &str, options: &RemoveOptions) -> Result<()> {
        let entry = self.stat(path, &options.operation).await?;
        if entry.kind != FileKind::Directory {
            return remote_mutation(
                &options.operation,
                "remove remote path",
                path,
                self.sftp.remove_file(path.to_owned()),
            )
            .await;
        }
        if !options.recursive {
            return remote_mutation(
                &options.operation,
                "remove remote directory",
                path,
                self.sftp.remove_dir(path.to_owned()),
            )
            .await;
        }

        let mut stack = vec![(path.to_owned(), false)];
        while let Some((current, visited)) = stack.pop() {
            options.operation.throw_if_cancelled()?;
            let entry = self.stat(&current, &options.operation).await?;
            if entry.kind != FileKind::Directory {
                remote_mutation(
                    &options.operation,
                    "remove remote path",
                    &current,
                    self.sftp.remove_file(current.clone()),
                )
                .await?;
                continue;
            }
            if visited {
                remote_mutation(
                    &options.operation,
                    "remove remote directory",
                    &current,
                    self.sftp.remove_dir(current.clone()),
                )
                .await?;
                continue;
            }
            stack.push((current.clone(), true));
            for child in self.list(&current, &options.operation).await? {
                stack.push((child.path, false));
            }
        }
        Ok(())
    }

    pub(crate) async fn exists(&self, path: &str) -> std::result::Result<bool, RusshSftpError> {
        self.sftp.try_exists(path.to_owned()).await
    }

    async fn mkdir_one(&self, path: &str, options: &MkdirOptions) -> Result<()> {
        remote_mutation(
            &options.operation,
            "create remote directory",
            path,
            self.sftp.create_dir(path.to_owned()),
        )
        .await?;
        if let Some(mode) = options.mode {
            let metadata = Metadata {
                permissions: Some(mode),
                ..Metadata::default()
            };
            remote_mutation(
                &options.operation,
                "set remote directory mode",
                path,
                self.sftp.set_metadata(path.to_owned(), metadata),
            )
            .await?;
        }
        Ok(())
    }
}

impl FileProvider for RemoteFileProvider {
    fn list<'a>(
        &'a self,
        directory: &'a str,
        options: &'a OperationOptions,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<FileEntry>>> + Send + 'a>> {
        Box::pin(RemoteFileProvider::list(self, directory, options))
    }

    fn mkdir<'a>(
        &'a self,
        path: &'a str,
        options: &'a MkdirOptions,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(RemoteFileProvider::mkdir(self, path, options))
    }

    fn rename<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
        options: &'a OperationOptions,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(RemoteFileProvider::rename(self, from, to, options))
    }

    fn remove<'a>(
        &'a self,
        path: &'a str,
        options: &'a RemoveOptions,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(RemoteFileProvider::remove(self, path, options))
    }
}

async fn remote_operation<T>(
    options: &OperationOptions,
    operation: &'static str,
    path: &str,
    future: impl Future<Output = std::result::Result<T, RusshSftpError>>,
) -> Result<T> {
    options.throw_if_cancelled()?;
    let value = future
        .await
        .map_err(|error| SftpError::remote_operation(operation, path, error))?;
    options.throw_if_cancelled()?;
    Ok(value)
}

async fn remote_mutation<T>(
    options: &OperationOptions,
    operation: &'static str,
    path: &str,
    future: impl Future<Output = std::result::Result<T, RusshSftpError>>,
) -> Result<T> {
    options.throw_if_cancelled()?;
    future
        .await
        .map_err(|error| SftpError::remote_operation(operation, path, error))
}

fn entry_from_metadata(path: String, name: String, metadata: Metadata) -> FileEntry {
    let kind = if metadata.is_dir() {
        FileKind::Directory
    } else if metadata.is_regular() {
        FileKind::File
    } else if metadata.is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::Other
    };
    FileEntry {
        name,
        path,
        kind,
        size: metadata.len(),
        modified_at: metadata
            .mtime
            .map(|seconds| UNIX_EPOCH + Duration::from_secs(u64::from(seconds))),
        mode: metadata.permissions,
    }
}

fn compare_entries(left: &FileEntry, right: &FileEntry) -> Ordering {
    match (
        left.kind == FileKind::Directory,
        right.kind == FileKind::Directory,
    ) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => left
            .name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name)),
    }
}

pub(crate) fn remote_basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    trimmed
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("/")
        .to_owned()
}

pub(crate) fn remote_dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => "/".to_owned(),
        Some(index) => trimmed[..index].to_owned(),
        None => ".".to_owned(),
    }
}

pub(crate) fn remote_join(parent: &str, child: &str) -> String {
    match parent {
        "" | "." => child.to_owned(),
        "/" => format!("/{child}"),
        _ => format!("{}/{child}", parent.trim_end_matches('/')),
    }
}

fn normalize_remote_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|last| *last != "..") => {
                parts.pop();
            }
            ".." if !absolute => parts.push(part),
            ".." => {}
            _ => parts.push(part),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_owned()
    } else {
        joined
    }
}

fn unique_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::SystemTime;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{nanos:x}-{sequence:x}", std::process::id())
}
