mod connection;
mod error;
mod local;
mod profile;
mod remote;
mod session;
mod transfer;
mod types;

pub use connection::connect_sftp;
pub use error::{ConnectionRole, SftpError};
pub use local::LocalFileProvider;
pub use profile::connection_from_profile;
pub use remote::RemoteFileProvider;
pub use session::SftpSession;
pub use types::{
    CompatibilityIssueSeverity, CredentialOverrides, FileEntry, FileKind, FileProvider,
    MkdirOptions, OperationOptions, ProfileConnectionOverrides, RemoveOptions, TransferDirection,
    TransferOptions, TransferProgress,
};
