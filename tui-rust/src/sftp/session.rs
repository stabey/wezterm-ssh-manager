use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use russh::client::Handle;
use tokio::sync::watch;

use super::connection::ClientHandler;
use super::error::Result;
use super::remote::RemoteFileProvider;
use super::transfer::{download_file, upload_file};
use super::types::{OperationOptions, TransferOptions};

pub struct SftpSession {
    sftp: Arc<russh_sftp::client::SftpSession>,
    target: Option<Handle<ClientHandler>>,
    jump: Option<Handle<ClientHandler>>,
    disconnected: watch::Receiver<Option<String>>,
    closing: Arc<AtomicBool>,
    pub remote: RemoteFileProvider,
}

impl std::fmt::Debug for SftpSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SftpSession")
            .field("closed", &self.target.is_none())
            .finish()
    }
}

impl SftpSession {
    pub(super) fn new(
        sftp: Arc<russh_sftp::client::SftpSession>,
        target: Handle<ClientHandler>,
        jump: Option<Handle<ClientHandler>>,
        disconnected: watch::Receiver<Option<String>>,
        closing: Arc<AtomicBool>,
    ) -> Self {
        Self {
            remote: RemoteFileProvider::new(sftp.clone()),
            sftp,
            target: Some(target),
            jump,
            disconnected,
            closing,
        }
    }

    pub fn disconnect_receiver(&self) -> watch::Receiver<Option<String>> {
        self.disconnected.clone()
    }

    pub async fn remote_home(&self, options: &OperationOptions) -> Result<String> {
        self.remote.realpath(".", options).await
    }

    pub async fn upload(
        &self,
        local_path: &str,
        remote_path: &str,
        options: &TransferOptions,
    ) -> Result<()> {
        upload_file(&self.sftp, local_path, remote_path, options).await
    }

    pub async fn download(
        &self,
        remote_path: &str,
        local_path: &str,
        options: &TransferOptions,
    ) -> Result<()> {
        download_file(&self.sftp, remote_path, local_path, options).await
    }

    pub async fn close(&mut self) -> Result<()> {
        if self.target.is_none() {
            return Ok(());
        }
        // The transport callback is also used for unexpected disconnects.
        // Suppress it before an intentional close so a delayed event from an
        // old session cannot tear down a newly established session in the UI.
        self.closing.store(true, Ordering::Release);
        let close_result = self
            .sftp
            .close()
            .await
            .map_err(|error| super::error::SftpError::ConnectionFailed(error.to_string()));
        if let Some(target) = self.target.take() {
            let _ = target
                .disconnect(russh::Disconnect::ByApplication, "SFTP closed", "en")
                .await;
        }
        if let Some(jump) = self.jump.take() {
            let _ = jump
                .disconnect(russh::Disconnect::ByApplication, "SFTP closed", "en")
                .await;
        }
        close_result
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::sftp::{MkdirOptions, RemoveOptions, TransferOptions, connect_sftp};

    use super::super::types::{PrivateKeySource, SftpAuthentication, SftpConnectionOptions};

    /// Real-server smoke test for the complete native SFTP path.
    ///
    /// Run explicitly with:
    /// `SSHMGR_SFTP_E2E_HOST=127.0.0.1 SSHMGR_SFTP_E2E_PORT=22222 \
    ///  SSHMGR_SFTP_E2E_USER=me SSHMGR_SFTP_E2E_KEY=/path/to/key \
    ///  cargo test sftp_server_round_trip -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "requires an explicitly configured writable SSH/SFTP test server"]
    async fn sftp_server_round_trip() -> Result<()> {
        let required = |name: &str| {
            std::env::var(name)
                .unwrap_or_else(|_| panic!("{name} must be set for the ignored SFTP E2E test"))
        };
        let host = required("SSHMGR_SFTP_E2E_HOST");
        let port = required("SSHMGR_SFTP_E2E_PORT")
            .parse::<u32>()
            .expect("SSHMGR_SFTP_E2E_PORT must be a number");
        let username = required("SSHMGR_SFTP_E2E_USER");
        let key = PathBuf::from(required("SSHMGR_SFTP_E2E_KEY"));
        let operation = OperationOptions::default();
        let mut session = connect_sftp(
            &SftpConnectionOptions {
                host,
                port,
                username: Some(username),
                authentication: SftpAuthentication {
                    private_keys: vec![PrivateKeySource {
                        path: Some(key),
                        ..PrivateKeySource::default()
                    }],
                    ..SftpAuthentication::default()
                },
                ready_timeout: Some(Duration::from_secs(10)),
                ..SftpConnectionOptions::default()
            },
            &operation,
        )
        .await?;

        let home = session.remote_home(&operation).await?;
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let remote_root = if home == "/" {
            format!("/.sshmgr-rust-e2e-{}-{suffix:x}", std::process::id())
        } else {
            format!(
                "{}/.sshmgr-rust-e2e-{}-{suffix:x}",
                home.trim_end_matches('/'),
                std::process::id()
            )
        };
        let remote_uploaded = format!("{remote_root}/uploaded.txt");
        let remote_renamed = format!("{remote_root}/renamed.txt");
        let local = tempfile::tempdir().expect("local E2E directory");
        let local_source = local.path().join("source.txt");
        let local_download = local.path().join("downloaded.txt");
        let initial_payload = b"native russh-sftp round trip\n";
        let replaced_payload = b"native russh-sftp atomic replacement\n";
        std::fs::write(&local_source, initial_payload).expect("write local E2E source");

        let round_trip = async {
            // Exercise a real list request before mutations as well as every
            // provider/transfer primitive used by the dual-pane UI.
            let _ = session.remote.list(&home, &operation).await?;
            session
                .remote
                .mkdir(
                    &remote_root,
                    &MkdirOptions {
                        operation: operation.clone(),
                        recursive: true,
                        mode: None,
                    },
                )
                .await?;
            session
                .upload(
                    &local_source.to_string_lossy(),
                    &remote_uploaded,
                    &TransferOptions {
                        preserve_times: true,
                        ..TransferOptions::default()
                    },
                )
                .await?;
            let uploaded = session.remote.stat(&remote_uploaded, &operation).await?;
            if uploaded.size != initial_payload.len() as u64 {
                return Err(super::super::error::SftpError::ConnectionFailed(format!(
                    "uploaded size {} did not match {}",
                    uploaded.size,
                    initial_payload.len()
                )));
            }
            let no_overwrite = session
                .upload(
                    &local_source.to_string_lossy(),
                    &remote_uploaded,
                    &TransferOptions::default(),
                )
                .await;
            if !matches!(
                no_overwrite,
                Err(super::super::error::SftpError::DestinationExists(_))
            ) {
                return Err(super::super::error::SftpError::ConnectionFailed(
                    "upload without overwrite did not reject an existing destination".to_owned(),
                ));
            }
            std::fs::write(&local_source, replaced_payload).expect("write replacement E2E source");
            session
                .upload(
                    &local_source.to_string_lossy(),
                    &remote_uploaded,
                    &TransferOptions {
                        overwrite: true,
                        preserve_times: true,
                        ..TransferOptions::default()
                    },
                )
                .await?;
            session
                .remote
                .rename(&remote_uploaded, &remote_renamed, &operation)
                .await?;
            std::fs::write(&local_download, b"must not be overwritten")
                .expect("write existing E2E destination");
            let no_overwrite = session
                .download(
                    &remote_renamed,
                    &local_download.to_string_lossy(),
                    &TransferOptions::default(),
                )
                .await;
            if !matches!(
                no_overwrite,
                Err(super::super::error::SftpError::DestinationExists(_))
            ) {
                return Err(super::super::error::SftpError::ConnectionFailed(
                    "download without overwrite did not reject an existing destination".to_owned(),
                ));
            }
            session
                .download(
                    &remote_renamed,
                    &local_download.to_string_lossy(),
                    &TransferOptions {
                        overwrite: true,
                        preserve_times: true,
                        ..TransferOptions::default()
                    },
                )
                .await?;
            Ok::<_, super::super::error::SftpError>(
                std::fs::read(&local_download).expect("read E2E download"),
            )
        }
        .await;

        let _ = session
            .remote
            .remove(
                &remote_root,
                &RemoveOptions {
                    operation: OperationOptions::default(),
                    recursive: true,
                },
            )
            .await;
        let _ = session.close().await;
        assert_eq!(round_trip?, replaced_payload);
        Ok(())
    }
}
