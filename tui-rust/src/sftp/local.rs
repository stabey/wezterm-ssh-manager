use std::cmp::Ordering;
use std::future::Future;
use std::path::Path;

use tokio::fs;

use super::error::{Result, SftpError};
use super::types::{
    FileEntry, FileKind, FileProvider, MkdirOptions, OperationOptions, RemoveOptions,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct LocalFileProvider;

impl LocalFileProvider {
    pub async fn list(
        &self,
        directory: impl AsRef<Path>,
        options: &OperationOptions,
    ) -> Result<Vec<FileEntry>> {
        let directory = directory.as_ref();
        options.throw_if_cancelled()?;
        let mut reader = io_operation(
            options,
            "list local directory",
            directory,
            fs::read_dir(directory),
        )
        .await?;
        let mut entries = Vec::new();
        loop {
            let next = io_operation(
                options,
                "list local directory",
                directory,
                reader.next_entry(),
            )
            .await?;
            let Some(next) = next else { break };
            entries.push(self.stat(next.path(), options).await?);
        }
        entries.sort_by(compare_entries);
        Ok(entries)
    }

    pub async fn stat(
        &self,
        path: impl AsRef<Path>,
        options: &OperationOptions,
    ) -> Result<FileEntry> {
        let path = path.as_ref();
        let metadata =
            io_operation(options, "stat local path", path, fs::symlink_metadata(path)).await?;
        let file_type = metadata.file_type();
        let kind = if file_type.is_dir() {
            FileKind::Directory
        } else if file_type.is_file() {
            FileKind::File
        } else if file_type.is_symlink() {
            FileKind::Symlink
        } else {
            FileKind::Other
        };
        Ok(FileEntry {
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            path: path.to_string_lossy().into_owned(),
            kind,
            size: metadata.len(),
            modified_at: metadata.modified().ok(),
            mode: local_mode(&metadata),
        })
    }

    pub async fn mkdir(&self, path: impl AsRef<Path>, options: &MkdirOptions) -> Result<()> {
        let path = path.as_ref();
        let result = if options.recursive {
            io_mutation(
                &options.operation,
                "create local directory",
                path,
                fs::create_dir_all(path),
            )
            .await
        } else {
            io_mutation(
                &options.operation,
                "create local directory",
                path,
                fs::create_dir(path),
            )
            .await
        };
        result?;
        if let Some(mode) = options.mode {
            set_local_mode(path, mode, &options.operation).await?;
        }
        Ok(())
    }

    pub async fn rename(
        &self,
        from: impl AsRef<Path>,
        to: impl AsRef<Path>,
        options: &OperationOptions,
    ) -> Result<()> {
        let from = from.as_ref();
        io_mutation(options, "rename local path", from, fs::rename(from, to)).await
    }

    pub async fn remove(&self, path: impl AsRef<Path>, options: &RemoveOptions) -> Result<()> {
        let path = path.as_ref();
        let metadata = io_operation(
            &options.operation,
            "stat local path",
            path,
            fs::symlink_metadata(path),
        )
        .await?;
        if !metadata.is_dir() {
            return io_mutation(
                &options.operation,
                "remove local path",
                path,
                fs::remove_file(path),
            )
            .await;
        }
        if !options.recursive {
            return io_mutation(
                &options.operation,
                "remove local directory",
                path,
                fs::remove_dir(path),
            )
            .await;
        }

        // Post-order traversal avoids following directory symlinks and leaves a
        // cancellation point between every filesystem operation.
        let mut stack = vec![(path.to_path_buf(), false)];
        while let Some((current, visited)) = stack.pop() {
            options.operation.throw_if_cancelled()?;
            let metadata = io_operation(
                &options.operation,
                "stat local path",
                &current,
                fs::symlink_metadata(&current),
            )
            .await?;
            if !metadata.is_dir() {
                io_mutation(
                    &options.operation,
                    "remove local path",
                    &current,
                    fs::remove_file(&current),
                )
                .await?;
                continue;
            }
            if visited {
                io_mutation(
                    &options.operation,
                    "remove local directory",
                    &current,
                    fs::remove_dir(&current),
                )
                .await?;
                continue;
            }

            stack.push((current.clone(), true));
            let mut reader = io_operation(
                &options.operation,
                "list local directory",
                &current,
                fs::read_dir(&current),
            )
            .await?;
            while let Some(child) = io_operation(
                &options.operation,
                "list local directory",
                &current,
                reader.next_entry(),
            )
            .await?
            {
                stack.push((child.path(), false));
            }
        }
        Ok(())
    }
}

impl FileProvider for LocalFileProvider {
    fn list<'a>(
        &'a self,
        directory: &'a str,
        options: &'a OperationOptions,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<FileEntry>>> + Send + 'a>> {
        Box::pin(LocalFileProvider::list(self, directory, options))
    }

    fn mkdir<'a>(
        &'a self,
        path: &'a str,
        options: &'a MkdirOptions,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(LocalFileProvider::mkdir(self, path, options))
    }

    fn rename<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
        options: &'a OperationOptions,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(LocalFileProvider::rename(self, from, to, options))
    }

    fn remove<'a>(
        &'a self,
        path: &'a str,
        options: &'a RemoveOptions,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(LocalFileProvider::remove(self, path, options))
    }
}

async fn io_operation<T>(
    options: &OperationOptions,
    operation: &'static str,
    path: &Path,
    future: impl Future<Output = std::io::Result<T>>,
) -> Result<T> {
    options.throw_if_cancelled()?;
    let value = future
        .await
        .map_err(|error| SftpError::operation(operation, path, error))?;
    options.throw_if_cancelled()?;
    Ok(value)
}

async fn io_mutation<T>(
    options: &OperationOptions,
    operation: &'static str,
    path: &Path,
    future: impl Future<Output = std::io::Result<T>>,
) -> Result<T> {
    options.throw_if_cancelled()?;
    future
        .await
        .map_err(|error| SftpError::operation(operation, path, error))
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

#[cfg(unix)]
fn local_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.mode())
}

#[cfg(not(unix))]
fn local_mode(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
async fn set_local_mode(path: &Path, mode: u32, options: &OperationOptions) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let permissions = std::fs::Permissions::from_mode(mode);
    io_mutation(
        options,
        "set local directory mode",
        path,
        fs::set_permissions(path, permissions),
    )
    .await
}

#[cfg(not(unix))]
async fn set_local_mode(_path: &Path, _mode: u32, options: &OperationOptions) -> Result<()> {
    options.throw_if_cancelled()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn lists_renames_and_recursively_removes_local_paths() {
        let root = tempdir().expect("temporary directory");
        let provider = LocalFileProvider;
        let nested = root.path().join("directory").join("nested");
        provider
            .mkdir(
                &nested,
                &MkdirOptions {
                    recursive: true,
                    ..MkdirOptions::default()
                },
            )
            .await
            .expect("create nested directory");
        fs::write(root.path().join("z.txt"), b"z")
            .await
            .expect("write z");
        fs::write(root.path().join("a.txt"), b"a")
            .await
            .expect("write a");

        let entries = provider
            .list(root.path(), &OperationOptions::default())
            .await
            .expect("list root");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["directory", "a.txt", "z.txt"]
        );
        assert_eq!(entries[0].kind, FileKind::Directory);

        let renamed = root.path().join("renamed.txt");
        provider
            .rename(
                root.path().join("a.txt"),
                &renamed,
                &OperationOptions::default(),
            )
            .await
            .expect("rename file");
        assert_eq!(
            provider
                .stat(&renamed, &OperationOptions::default())
                .await
                .expect("stat renamed file")
                .size,
            1
        );

        provider
            .remove(
                root.path().join("directory"),
                &RemoveOptions {
                    recursive: true,
                    ..RemoveOptions::default()
                },
            )
            .await
            .expect("remove tree");
        assert!(!root.path().join("directory").exists());
    }

    #[tokio::test]
    async fn honors_an_already_cancelled_operation() {
        let root = tempdir().expect("temporary directory");
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let operation = OperationOptions::with_cancellation(cancellation);

        let list_error = LocalFileProvider
            .list(root.path(), &operation)
            .await
            .expect_err("cancelled list");
        assert!(matches!(list_error, SftpError::Aborted));

        let directory = root.path().join("must-not-exist");
        let mkdir_error = LocalFileProvider
            .mkdir(
                &directory,
                &MkdirOptions {
                    operation,
                    recursive: false,
                    mode: None,
                },
            )
            .await
            .expect_err("cancelled mkdir");
        assert!(matches!(mkdir_error, SftpError::Aborted));
        assert!(!directory.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn recursive_remove_does_not_follow_directory_symlinks() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("temporary directory");
        let outside = tempdir().expect("outside directory");
        fs::write(outside.path().join("keep.txt"), b"keep")
            .await
            .expect("write outside file");
        let tree = root.path().join("tree");
        fs::create_dir(&tree).await.expect("create tree");
        symlink(outside.path(), tree.join("outside-link")).expect("create symlink");

        LocalFileProvider
            .remove(
                &tree,
                &RemoveOptions {
                    recursive: true,
                    ..RemoveOptions::default()
                },
            )
            .await
            .expect("remove symlink-containing tree");

        assert!(outside.path().join("keep.txt").exists());
        assert!(!tree.exists());
    }
}
