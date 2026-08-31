use std::path::Path;

use russh_sftp::client::error::Error as RusshSftpError;
use russh_sftp::protocol::StatusCode;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, SftpError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionRole {
    Target,
    Jump,
}

impl ConnectionRole {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Target => "target host",
            Self::Jump => "jump host",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialMethod {
    PrivateKey,
    Agent,
    Authentication,
}

impl CredentialMethod {
    pub const fn label(self) -> &'static str {
        match self {
            Self::PrivateKey => "private key",
            Self::Agent => "SSH agent",
            Self::Authentication => "authentication",
        }
    }
}

#[derive(Debug, Error)]
pub enum SftpError {
    #[error("SFTP operation was cancelled")]
    Aborted,

    #[error("{role_label} requires {method_label} credentials", role_label = .role.label(), method_label = .method.label())]
    CredentialRequired {
        role: ConnectionRole,
        method: CredentialMethod,
    },

    #[error("authentication failed for {role_label}", role_label = .role.label())]
    AuthenticationFailed { role: ConnectionRole },

    #[error("invalid SFTP connection: {0}")]
    InvalidConnection(String),

    #[error("SFTP connection failed: {0}")]
    ConnectionFailed(String),

    #[error("{operation}{path_suffix} failed: {detail}", path_suffix = path.as_ref().map(|value| format!(" {}", value.display())).unwrap_or_default())]
    OperationFailed {
        operation: &'static str,
        path: Option<std::path::PathBuf>,
        detail: String,
    },

    #[error("{0} is not a regular file")]
    NotAFile(String),

    #[error("destination already exists: {0}")]
    DestinationExists(String),

    #[error("replace failed for {path}; the original remains at {backup}: {detail}")]
    ReplaceFailed {
        path: String,
        backup: String,
        detail: String,
    },
}

impl SftpError {
    pub fn operation(
        operation: &'static str,
        path: impl AsRef<Path>,
        error: impl std::fmt::Display,
    ) -> Self {
        Self::OperationFailed {
            operation,
            path: Some(path.as_ref().to_path_buf()),
            detail: error.to_string(),
        }
    }

    pub fn remote_operation(
        operation: &'static str,
        path: impl Into<String>,
        error: impl std::fmt::Display,
    ) -> Self {
        Self::OperationFailed {
            operation,
            path: Some(std::path::PathBuf::from(path.into())),
            detail: error.to_string(),
        }
    }
}

pub fn is_remote_not_found(error: &RusshSftpError) -> bool {
    matches!(
        error,
        RusshSftpError::Status(status) if status.status_code == StatusCode::NoSuchFile
    )
}
