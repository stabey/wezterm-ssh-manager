use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use russh::keys::PublicKeyOrCertificate;
use tokio_util::sync::CancellationToken;

use super::error::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Directory,
    File,
    Symlink,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
    pub modified_at: Option<SystemTime>,
    pub mode: Option<u32>,
}

#[derive(Clone, Default)]
pub struct OperationOptions {
    pub cancellation: Option<CancellationToken>,
}

impl fmt::Debug for OperationOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperationOptions")
            .field(
                "cancelled",
                &self
                    .cancellation
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled),
            )
            .finish()
    }
}

impl OperationOptions {
    pub fn with_cancellation(cancellation: CancellationToken) -> Self {
        Self {
            cancellation: Some(cancellation),
        }
    }

    pub fn throw_if_cancelled(&self) -> Result<()> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            Err(super::error::SftpError::Aborted)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MkdirOptions {
    pub operation: OperationOptions,
    pub recursive: bool,
    pub mode: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct RemoveOptions {
    pub operation: OperationOptions,
    pub recursive: bool,
}

pub trait FileProvider: Send + Sync {
    fn list<'a>(
        &'a self,
        directory: &'a str,
        options: &'a OperationOptions,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<FileEntry>>> + Send + 'a>>;

    fn mkdir<'a>(
        &'a self,
        path: &'a str,
        options: &'a MkdirOptions,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

    fn rename<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
        options: &'a OperationOptions,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

    fn remove<'a>(
        &'a self,
        path: &'a str,
        options: &'a RemoveOptions,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Clone, Debug)]
pub struct TransferProgress {
    pub direction: TransferDirection,
    pub source: String,
    pub transferred_bytes: u64,
    pub total_bytes: Option<u64>,
    pub percent: Option<f64>,
    pub bytes_per_second: f64,
}

pub type ProgressCallback = Arc<dyn Fn(TransferProgress) + Send + Sync + 'static>;

#[derive(Clone)]
pub struct TransferOptions {
    pub operation: OperationOptions,
    pub overwrite: bool,
    pub create_parents: bool,
    pub atomic: bool,
    pub preserve_times: bool,
    pub on_progress: Option<ProgressCallback>,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            operation: OperationOptions::default(),
            overwrite: false,
            create_parents: true,
            atomic: true,
            preserve_times: false,
            on_progress: None,
        }
    }
}

impl fmt::Debug for TransferOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransferOptions")
            .field("operation", &self.operation)
            .field("overwrite", &self.overwrite)
            .field("create_parents", &self.create_parents)
            .field("atomic", &self.atomic)
            .field("preserve_times", &self.preserve_times)
            .field("has_progress_callback", &self.on_progress.is_some())
            .finish()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrivateKeySource {
    pub path: Option<PathBuf>,
    pub data: Option<Vec<u8>>,
    pub passphrase: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentEndpoint {
    Default,
    Path(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SftpAuthentication {
    pub password: Option<String>,
    pub private_keys: Vec<PrivateKeySource>,
    pub agent: Option<AgentEndpoint>,
}

pub type HostVerifier = Arc<dyn Fn(&PublicKeyOrCertificate) -> bool + Send + Sync + 'static>;

#[derive(Clone)]
pub struct SftpConnectionOptions {
    pub host: String,
    pub port: u32,
    pub username: Option<String>,
    pub authentication: SftpAuthentication,
    pub ready_timeout: Option<Duration>,
    pub keepalive_interval: Option<Duration>,
    pub keepalive_count_max: Option<usize>,
    pub jump: Option<Box<SftpConnectionOptions>>,
    pub host_verifier: Option<HostVerifier>,
}

impl fmt::Debug for SftpConnectionOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SftpConnectionOptions")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("authentication", &self.authentication)
            .field("ready_timeout", &self.ready_timeout)
            .field("keepalive_interval", &self.keepalive_interval)
            .field("keepalive_count_max", &self.keepalive_count_max)
            .field("jump", &self.jump)
            .field("has_host_verifier", &self.host_verifier.is_some())
            .finish()
    }
}

impl Default for SftpConnectionOptions {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 22,
            username: None,
            authentication: SftpAuthentication::default(),
            ready_timeout: None,
            keepalive_interval: None,
            keepalive_count_max: None,
            jump: None,
            host_verifier: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CredentialOverrides {
    pub password: Option<String>,
    pub private_keys: Option<Vec<PrivateKeySource>>,
    pub agent: Option<AgentEndpoint>,
}

#[derive(Clone, Debug, Default)]
pub struct ProfileConnectionOverrides {
    pub credentials: CredentialOverrides,
    pub jump: CredentialOverrides,
    pub environment: Option<HashMap<String, String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompatibilityIssueSeverity {
    NeedsInput,
    Warning,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompatibilityIssue {
    pub field: String,
    pub severity: CompatibilityIssueSeverity,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct ProfileConnectionResult {
    pub connection: SftpConnectionOptions,
    pub issues: Vec<CompatibilityIssue>,
    pub supported: bool,
}
