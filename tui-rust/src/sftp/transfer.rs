use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use filetime::FileTime;
use russh_sftp::client::SftpSession as RusshSftpSession;
use russh_sftp::client::fs::Metadata;
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::error::{Result, SftpError};
use super::remote::{RemoteFileProvider, remote_basename, remote_dirname, remote_join};
use super::types::{
    FileKind, MkdirOptions, OperationOptions, TransferDirection, TransferOptions, TransferProgress,
};

const COPY_BUFFER_SIZE: usize = 64 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

struct TransferContext<'a> {
    options: &'a TransferOptions,
    direction: TransferDirection,
    source: &'a str,
    destination: &'a str,
    total_bytes: Option<u64>,
    started_at: Instant,
}

/// Stream a regular local file to an SFTP server.
///
/// Atomic transfers are written to a sibling first. Once the write has been
/// acknowledged and optional timestamps have been applied, the sibling is
/// renamed into place. This keeps a cancelled transfer away from the requested
/// destination path.
pub async fn upload_file(
    sftp: &Arc<RusshSftpSession>,
    local_path: &str,
    remote_path: &str,
    options: &TransferOptions,
) -> Result<()> {
    options.operation.throw_if_cancelled()?;
    let local_path = Path::new(local_path);
    let remote = RemoteFileProvider::new(sftp.clone());
    let source = settled_io(
        &options.operation,
        "stat upload source",
        local_path,
        fs::symlink_metadata(local_path),
    )
    .await?;
    if !source.file_type().is_file() {
        return Err(SftpError::NotAFile(
            local_path.to_string_lossy().into_owned(),
        ));
    }

    let destination_exists = cancellable_remote(
        &options.operation,
        "check remote destination",
        remote_path,
        remote.exists(remote_path),
    )
    .await?;
    if destination_exists
        && (!options.overwrite
            || remote.stat(remote_path, &options.operation).await?.kind != FileKind::File)
    {
        return Err(SftpError::DestinationExists(remote_path.to_owned()));
    }

    if options.create_parents {
        let parent = remote_dirname(remote_path);
        if parent != "." && parent != "/" {
            remote
                .mkdir(
                    &parent,
                    &MkdirOptions {
                        operation: options.operation.clone(),
                        recursive: true,
                        mode: None,
                    },
                )
                .await?;
        }
    }

    let temporary = if options.atomic {
        remote_temporary(remote_path)
    } else {
        remote_path.to_owned()
    };
    let started_at = Instant::now();
    let local_source = local_path.to_string_lossy().into_owned();
    let context = TransferContext {
        options,
        direction: TransferDirection::Upload,
        source: &local_source,
        destination: remote_path,
        total_bytes: Some(source.len()),
        started_at,
    };
    report(&context, 0);

    let result = async {
        let mut reader = settled_io(
            &options.operation,
            "open upload source",
            local_path,
            fs::File::open(local_path),
        )
        .await?;
        let mut writer = cancellable_remote(
            &options.operation,
            "create remote upload file",
            &temporary,
            sftp.create(temporary.clone()),
        )
        .await?;
        let transferred = copy_with_progress(&mut reader, &mut writer, &context).await?;

        // russh-sftp queues write requests. shutdown() drains every response and
        // closes the remote handle, so no successful transfer is reported before
        // the server has acknowledged all writes.
        cancellable_io(
            &options.operation,
            "finish remote upload file",
            Path::new(&temporary),
            writer.shutdown(),
        )
        .await?;
        report(&context, transferred);

        if options.preserve_times {
            let timestamps = Metadata {
                atime: source.accessed().ok().map(unix_seconds_u32),
                mtime: source.modified().ok().map(unix_seconds_u32),
                ..Metadata::default()
            };
            cancellable_remote(
                &options.operation,
                "set remote file timestamps",
                &temporary,
                sftp.set_metadata(temporary.clone(), timestamps),
            )
            .await?;
        }

        if options.atomic {
            if options.overwrite {
                remote
                    .replace(&temporary, remote_path, &options.operation)
                    .await?;
            } else {
                remote
                    .rename(&temporary, remote_path, &options.operation)
                    .await?;
            }
        }
        Ok(transferred)
    }
    .await;

    match result {
        Ok(transferred) => {
            report(&context, transferred);
            Ok(())
        }
        Err(error) => {
            // Cleanup deliberately ignores the cancellation token: a cancelled
            // operation should not strand its private temporary file.
            if options.atomic {
                let _ = sftp.remove_file(temporary).await;
            }
            Err(error)
        }
    }
}

/// Stream a regular remote file to the local filesystem.
pub async fn download_file(
    sftp: &Arc<RusshSftpSession>,
    remote_path: &str,
    local_path: &str,
    options: &TransferOptions,
) -> Result<()> {
    options.operation.throw_if_cancelled()?;
    let remote = RemoteFileProvider::new(sftp.clone());
    let source = remote.stat(remote_path, &options.operation).await?;
    if source.kind != FileKind::File {
        return Err(SftpError::NotAFile(remote_path.to_owned()));
    }

    let local_path = Path::new(local_path);
    if let Some(destination) = local_metadata(local_path, &options.operation).await?
        && (!options.overwrite || !destination.file_type().is_file())
    {
        return Err(SftpError::DestinationExists(
            local_path.to_string_lossy().into_owned(),
        ));
    }

    if options.create_parents
        && let Some(parent) = local_path.parent()
        && !parent.as_os_str().is_empty()
    {
        settled_io(
            &options.operation,
            "create download destination directory",
            parent,
            fs::create_dir_all(parent),
        )
        .await?;
    }

    let temporary = if options.atomic {
        local_temporary(local_path)
    } else {
        local_path.to_path_buf()
    };
    let total = Some(source.size);
    let started_at = Instant::now();
    let local_destination = local_path.to_string_lossy().into_owned();
    let context = TransferContext {
        options,
        direction: TransferDirection::Download,
        source: remote_path,
        destination: &local_destination,
        total_bytes: total,
        started_at,
    };
    report(&context, 0);

    let result = async {
        let mut reader = cancellable_remote(
            &options.operation,
            "open remote download file",
            remote_path,
            sftp.open(remote_path.to_owned()),
        )
        .await?;
        let mut writer = settled_io(
            &options.operation,
            "create local download file",
            &temporary,
            fs::File::create(&temporary),
        )
        .await?;
        let transferred = copy_with_progress(&mut reader, &mut writer, &context).await?;
        cancellable_io(
            &options.operation,
            "flush local download file",
            &temporary,
            writer.flush(),
        )
        .await?;
        report(&context, transferred);

        drop(writer);
        if options.preserve_times
            && let Some(modified_at) = source.modified_at
        {
            set_local_times(&temporary, modified_at, &options.operation).await?;
        }
        if options.atomic {
            replace_local(
                &temporary,
                local_path,
                options.overwrite,
                &options.operation,
            )
            .await?;
        }
        Ok(transferred)
    }
    .await;

    match result {
        Ok(transferred) => {
            report(&context, transferred);
            Ok(())
        }
        Err(error) => {
            // The path is guaranteed to be either our private sibling or the
            // partially-created destination from a non-atomic transfer.
            if options.atomic {
                let _ = fs::remove_file(&temporary).await;
            }
            Err(error)
        }
    }
}

async fn copy_with_progress<R, W>(
    reader: &mut R,
    writer: &mut W,
    context: &TransferContext<'_>,
) -> Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    let mut transferred = 0_u64;
    let mut last_progress = Instant::now();
    loop {
        context.options.operation.throw_if_cancelled()?;
        let read = cancellable_io(
            &context.options.operation,
            "read transfer source",
            Path::new(context.source),
            reader.read(&mut buffer),
        )
        .await?;
        if read == 0 {
            break;
        }
        cancellable_io(
            &context.options.operation,
            "write transfer destination",
            Path::new(context.destination),
            writer.write_all(&buffer[..read]),
        )
        .await?;
        transferred = transferred.saturating_add(read as u64);
        if last_progress.elapsed() >= PROGRESS_INTERVAL
            || context
                .total_bytes
                .is_some_and(|total| transferred >= total)
        {
            report(context, transferred);
            last_progress = Instant::now();
        }
    }
    Ok(transferred)
}

fn report(context: &TransferContext<'_>, transferred_bytes: u64) {
    let Some(callback) = &context.options.on_progress else {
        return;
    };
    let elapsed = context.started_at.elapsed();
    let percent = context.total_bytes.map(|total| {
        if total == 0 {
            100.0
        } else {
            ((transferred_bytes as f64 / total as f64) * 100.0).min(100.0)
        }
    });
    let bytes_per_second = if elapsed.is_zero() {
        0.0
    } else {
        transferred_bytes as f64 / elapsed.as_secs_f64()
    };
    let progress = TransferProgress {
        direction: context.direction,
        source: context.source.to_owned(),
        transferred_bytes,
        total_bytes: context.total_bytes,
        percent,
        bytes_per_second,
    };
    // Rendering/progress consumers must not be able to corrupt a transfer.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(progress)));
}

async fn replace_local(
    from: &Path,
    to: &Path,
    overwrite: bool,
    options: &OperationOptions,
) -> Result<()> {
    options.throw_if_cancelled()?;
    match committed_io(
        options,
        "replace local destination",
        to,
        fs::rename(from, to),
    )
    .await
    {
        Ok(()) => return Ok(()),
        Err(first_error) if !overwrite => return Err(first_error),
        Err(first_error) => {
            options.throw_if_cancelled()?;
            let destination_exists =
                plain_io("check local destination", to, fs::try_exists(to)).await?;
            if !destination_exists {
                return Err(first_error);
            }
        }
    }

    // Windows does not reliably replace an existing file with rename(). Move
    // it aside and roll back if installing the new file fails.
    let backup = local_backup(to);
    plain_io("back up local destination", to, fs::rename(to, &backup)).await?;
    match plain_io("install local destination", to, fs::rename(from, to)).await {
        Ok(()) => {
            let _ = fs::remove_file(backup).await;
            Ok(())
        }
        Err(replacement_error) => match fs::rename(&backup, to).await {
            Ok(()) => Err(replacement_error),
            Err(restore_error) => Err(SftpError::ReplaceFailed {
                path: to.to_string_lossy().into_owned(),
                backup: backup.to_string_lossy().into_owned(),
                detail: format!("replacement: {replacement_error}; restore: {restore_error}"),
            }),
        },
    }
}

async fn set_local_times(
    path: &Path,
    modified_at: SystemTime,
    options: &OperationOptions,
) -> Result<()> {
    options.throw_if_cancelled()?;
    let owned_path = path.to_path_buf();
    let operation_path = owned_path.clone();
    let task = tokio::task::spawn_blocking(move || {
        let modified = FileTime::from_system_time(modified_at);
        let accessed = FileTime::now();
        filetime::set_file_times(owned_path, accessed, modified)
    });
    let joined = task.await;
    joined
        .map_err(|error| SftpError::operation("set local file timestamps", &operation_path, error))?
        .map_err(|error| {
            SftpError::operation("set local file timestamps", &operation_path, error)
        })?;
    options.throw_if_cancelled()
}

async fn local_metadata(
    path: &Path,
    options: &OperationOptions,
) -> Result<Option<std::fs::Metadata>> {
    options.throw_if_cancelled()?;
    let result = fs::symlink_metadata(path).await;
    options.throw_if_cancelled()?;
    match result {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(SftpError::operation("check local destination", path, error)),
    }
}

/// Await filesystem setup operations to completion before observing a newly
/// requested cancellation. Tokio implements many of them on a blocking pool;
/// dropping such a future does not cancel the underlying OS operation.
async fn settled_io<T>(
    options: &OperationOptions,
    operation: &'static str,
    path: &Path,
    future: impl Future<Output = io::Result<T>>,
) -> Result<T> {
    options.throw_if_cancelled()?;
    let value = future
        .await
        .map_err(|error| SftpError::operation(operation, path, error))?;
    options.throw_if_cancelled()?;
    Ok(value)
}

/// Once a local mutation starts, await its outcome and report the committed
/// result. This avoids returning `Aborted` while an uncancellable blocking
/// rename continues in the background.
async fn committed_io<T>(
    options: &OperationOptions,
    operation: &'static str,
    path: &Path,
    future: impl Future<Output = io::Result<T>>,
) -> Result<T> {
    options.throw_if_cancelled()?;
    plain_io(operation, path, future).await
}

async fn plain_io<T>(
    operation: &'static str,
    path: &Path,
    future: impl Future<Output = io::Result<T>>,
) -> Result<T> {
    future
        .await
        .map_err(|error| SftpError::operation(operation, path, error))
}

async fn cancellable_io<T>(
    options: &OperationOptions,
    operation: &'static str,
    path: &Path,
    future: impl Future<Output = io::Result<T>>,
) -> Result<T> {
    options.throw_if_cancelled()?;
    if let Some(cancellation) = &options.cancellation {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(SftpError::Aborted),
            result = future => result.map_err(|error| SftpError::operation(operation, path, error)),
        }
    } else {
        future
            .await
            .map_err(|error| SftpError::operation(operation, path, error))
    }
}

async fn cancellable_remote<T, E>(
    options: &OperationOptions,
    operation: &'static str,
    path: &str,
    future: impl Future<Output = std::result::Result<T, E>>,
) -> Result<T>
where
    E: std::fmt::Display,
{
    options.throw_if_cancelled()?;
    let value = future
        .await
        .map_err(|error| SftpError::remote_operation(operation, path, error))?;
    options.throw_if_cancelled()?;
    Ok(value)
}

fn remote_temporary(path: &str) -> String {
    remote_join(
        &remote_dirname(path),
        &format!(".{}.sshmgr-{}.part", remote_basename(path), unique_id()),
    )
}

fn local_temporary(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_owned());
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{name}.sshmgr-{}.part", unique_id()))
}

fn local_backup(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_owned());
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{name}.sshmgr-{}.backup", unique_id()))
}

fn unix_seconds_u32(time: SystemTime) -> u32 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
        .min(u64::from(u32::MAX)) as u32
}

fn unique_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{nanos:x}-{sequence:x}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_temporary_is_a_sibling() {
        let temporary = remote_temporary("/srv/data/report.txt");
        assert!(temporary.starts_with("/srv/data/.report.txt.sshmgr-"));
        assert!(temporary.ends_with(".part"));
    }

    #[test]
    fn progress_math_handles_an_empty_file() {
        let captured = Arc::new(std::sync::Mutex::new(None));
        let receiver = captured.clone();
        let options = TransferOptions {
            on_progress: Some(Arc::new(move |progress| {
                *receiver.lock().expect("progress mutex") = Some(progress);
            })),
            ..TransferOptions::default()
        };
        report(
            &TransferContext {
                options: &options,
                direction: TransferDirection::Upload,
                source: "source",
                destination: "destination",
                total_bytes: Some(0),
                started_at: Instant::now(),
            },
            0,
        );
        let progress = captured
            .lock()
            .expect("progress mutex")
            .clone()
            .expect("progress callback");
        assert_eq!(progress.percent, Some(100.0));
    }
}
