use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use russh::client::{self, AuthResult, DisconnectReason, Handle};
use russh::keys::agent::{
    AgentIdentity,
    client::{AgentClient, AgentStream},
};
use russh::keys::{
    Algorithm, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate,
    decode_secret_key,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::error::{ConnectionRole, CredentialMethod, Result, SftpError};
use super::session::SftpSession;
use super::types::{
    AgentEndpoint, HostVerifier, OperationOptions, PrivateKeySource, SftpConnectionOptions,
};

#[derive(Clone)]
pub(super) struct ClientHandler {
    verifier: Option<HostVerifier>,
    disconnected: watch::Sender<Option<String>>,
    closing: Arc<AtomicBool>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        // This intentionally retains the existing personal-tool behavior:
        // absent an explicit verifier, the host key is accepted.
        Ok(self
            .verifier
            .as_ref()
            .is_none_or(|verify| verify(server_public_key)))
    }

    async fn disconnected(
        &mut self,
        reason: DisconnectReason<Self::Error>,
    ) -> std::result::Result<(), Self::Error> {
        if !self.closing.load(Ordering::Acquire) {
            let detail = format!("{reason:?}");
            let _ = self.disconnected.send(Some(detail));
        }
        match reason {
            DisconnectReason::ReceivedDisconnect(_) => Ok(()),
            DisconnectReason::Error(error) => Err(error),
        }
    }
}

type SshHandle = Handle<ClientHandler>;

/// Create a new SSH/SFTP connection. Existing WezTerm SSH panes are not reused.
pub async fn connect_sftp(
    options: &SftpConnectionOptions,
    operation: &OperationOptions,
) -> Result<SftpSession> {
    operation.throw_if_cancelled()?;
    validate_endpoint(options, ConnectionRole::Target)?;
    if options
        .jump
        .as_ref()
        .and_then(|jump| jump.jump.as_ref())
        .is_some()
    {
        return Err(SftpError::InvalidConnection(
            "only one SFTP jump host is supported".to_owned(),
        ));
    }

    let cancellation = operation.cancellation.as_ref();
    let (disconnected_sender, disconnected_receiver) = watch::channel(None);
    let closing = Arc::new(AtomicBool::new(false));
    let mut jump_handle = None;
    let mut target_handle;

    if let Some(jump) = options.jump.as_deref() {
        validate_endpoint(jump, ConnectionRole::Jump)?;
        let mut connected_jump = connect_tcp(
            jump,
            cancellation,
            disconnected_sender.clone(),
            closing.clone(),
        )
        .await?;
        if let Err(error) = authenticate(
            &mut connected_jump,
            jump,
            ConnectionRole::Jump,
            cancellation,
        )
        .await
        {
            disconnect_quietly(&connected_jump).await;
            return Err(error);
        }
        let channel = match controlled(
            connected_jump.channel_open_direct_tcpip(
                options.host.clone(),
                options.port,
                "127.0.0.1",
                0,
            ),
            cancellation,
            options.ready_timeout,
            "open jump host forwarding channel",
        )
        .await
        {
            Ok(channel) => channel,
            Err(error) => {
                disconnect_quietly(&connected_jump).await;
                return Err(error);
            }
        };
        let connected = match connect_stream(
            options,
            channel.into_stream(),
            cancellation,
            disconnected_sender.clone(),
            closing.clone(),
        )
        .await
        {
            Ok(connected) => connected,
            Err(error) => {
                disconnect_quietly(&connected_jump).await;
                return Err(error);
            }
        };
        target_handle = connected;
        jump_handle = Some(connected_jump);
    } else {
        target_handle = connect_tcp(
            options,
            cancellation,
            disconnected_sender.clone(),
            closing.clone(),
        )
        .await?;
    }

    if let Err(error) = authenticate(
        &mut target_handle,
        options,
        ConnectionRole::Target,
        cancellation,
    )
    .await
    {
        disconnect_quietly(&target_handle).await;
        if let Some(jump) = &jump_handle {
            disconnect_quietly(jump).await;
        }
        return Err(error);
    }

    let sftp_result = async {
        let channel = controlled(
            target_handle.channel_open_session(),
            cancellation,
            options.ready_timeout,
            "open SFTP session channel",
        )
        .await?;
        controlled(
            channel.request_subsystem(true, "sftp"),
            cancellation,
            options.ready_timeout,
            "request SFTP subsystem",
        )
        .await?;
        controlled(
            russh_sftp::client::SftpSession::new(channel.into_stream()),
            cancellation,
            options.ready_timeout,
            "initialize SFTP subsystem",
        )
        .await
    }
    .await;
    let sftp = match sftp_result {
        Ok(sftp) => sftp,
        Err(error) => {
            disconnect_quietly(&target_handle).await;
            if let Some(jump) = &jump_handle {
                disconnect_quietly(jump).await;
            }
            return Err(error);
        }
    };

    Ok(SftpSession::new(
        Arc::new(sftp),
        target_handle,
        jump_handle,
        disconnected_receiver,
        closing,
    ))
}

fn validate_endpoint(options: &SftpConnectionOptions, role: ConnectionRole) -> Result<()> {
    if options.host.trim().is_empty() {
        return Err(SftpError::InvalidConnection(format!(
            "{} host is required",
            role.label()
        )));
    }
    if options.port == 0 || options.port > u32::from(u16::MAX) {
        return Err(SftpError::InvalidConnection(format!(
            "{} port {} is outside 1..={}",
            role.label(),
            options.port,
            u16::MAX
        )));
    }
    Ok(())
}

async fn connect_tcp(
    options: &SftpConnectionOptions,
    cancellation: Option<&CancellationToken>,
    disconnected: watch::Sender<Option<String>>,
    closing: Arc<AtomicBool>,
) -> Result<SshHandle> {
    let handler = handler(options, disconnected, closing);
    let config = client_config(options);
    let port = u16::try_from(options.port)
        .map_err(|_| SftpError::InvalidConnection(format!("invalid SFTP port {}", options.port)))?;
    let address = (options.host.as_str(), port);
    let handle = controlled(
        client::connect(config, address, handler),
        cancellation,
        options.ready_timeout,
        "connect SSH transport",
    )
    .await?;
    Ok(handle)
}

async fn connect_stream<S>(
    options: &SftpConnectionOptions,
    stream: S,
    cancellation: Option<&CancellationToken>,
    disconnected: watch::Sender<Option<String>>,
    closing: Arc<AtomicBool>,
) -> Result<SshHandle>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let handler = handler(options, disconnected, closing);
    let handle = controlled(
        client::connect_stream(client_config(options), stream, handler),
        cancellation,
        options.ready_timeout,
        "connect SSH transport through jump host",
    )
    .await?;
    Ok(handle)
}

fn handler(
    options: &SftpConnectionOptions,
    disconnected: watch::Sender<Option<String>>,
    closing: Arc<AtomicBool>,
) -> ClientHandler {
    ClientHandler {
        verifier: options.host_verifier.clone(),
        disconnected,
        closing,
    }
}

fn client_config(options: &SftpConnectionOptions) -> Arc<client::Config> {
    let defaults = client::Config::default();
    Arc::new(client::Config {
        keepalive_interval: options.keepalive_interval,
        keepalive_max: options
            .keepalive_count_max
            .unwrap_or(defaults.keepalive_max),
        nodelay: true,
        ..defaults
    })
}

async fn authenticate(
    handle: &mut SshHandle,
    options: &SftpConnectionOptions,
    role: ConnectionRole,
    cancellation: Option<&CancellationToken>,
) -> Result<()> {
    let username = options
        .username
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(default_username);
    let authentication = &options.authentication;
    if authentication.password.is_none()
        && authentication.private_keys.is_empty()
        && authentication.agent.is_none()
    {
        return Err(SftpError::CredentialRequired {
            role,
            method: CredentialMethod::Authentication,
        });
    }

    if let Some(password) = &authentication.password {
        let result = controlled(
            handle.authenticate_password(username.clone(), password.clone()),
            cancellation,
            options.ready_timeout,
            "authenticate with password",
        )
        .await?;
        if result.success() {
            return Ok(());
        }
    }

    for source in &authentication.private_keys {
        let private_key = load_private_key(source, role, cancellation).await?;
        if authenticate_private_key(
            handle,
            &username,
            Arc::new(private_key),
            cancellation,
            options.ready_timeout,
        )
        .await?
        {
            return Ok(());
        }
    }

    if let Some(endpoint) = &authentication.agent
        && authenticate_agent(
            handle,
            &username,
            endpoint,
            role,
            cancellation,
            options.ready_timeout,
        )
        .await?
    {
        return Ok(());
    }

    Err(SftpError::AuthenticationFailed { role })
}

async fn load_private_key(
    source: &PrivateKeySource,
    role: ConnectionRole,
    cancellation: Option<&CancellationToken>,
) -> Result<PrivateKey> {
    let data = if let Some(data) = &source.data {
        data.clone()
    } else if let Some(path) = &source.path {
        let path = expand_home(path);
        controlled(
            tokio::fs::read(&path),
            cancellation,
            None,
            "read private key",
        )
        .await?
    } else {
        return Err(SftpError::CredentialRequired {
            role,
            method: CredentialMethod::PrivateKey,
        });
    };
    let text = std::str::from_utf8(&data).map_err(|error| {
        SftpError::ConnectionFailed(format!("private key is not UTF-8: {error}"))
    })?;
    decode_secret_key(text, source.passphrase.as_deref())
        .map_err(|error| SftpError::ConnectionFailed(format!("cannot decode private key: {error}")))
}

async fn authenticate_private_key(
    handle: &mut SshHandle,
    username: &str,
    key: Arc<PrivateKey>,
    cancellation: Option<&CancellationToken>,
    timeout: Option<Duration>,
) -> Result<bool> {
    let candidates = signature_candidates(handle, key.algorithm(), cancellation, timeout).await?;
    for hash in candidates {
        let result = controlled(
            handle.authenticate_publickey(
                username.to_owned(),
                PrivateKeyWithHashAlg::new(key.clone(), hash),
            ),
            cancellation,
            timeout,
            "authenticate with private key",
        )
        .await?;
        if result.success() {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn signature_candidates(
    handle: &SshHandle,
    algorithm: Algorithm,
    cancellation: Option<&CancellationToken>,
    timeout: Option<Duration>,
) -> Result<Vec<Option<HashAlg>>> {
    if !matches!(algorithm, Algorithm::Rsa { .. }) {
        return Ok(vec![None]);
    }
    let negotiated = controlled(
        handle.best_supported_rsa_hash(),
        cancellation,
        timeout,
        "negotiate RSA signature algorithm",
    )
    .await?;
    Ok(match negotiated {
        Some(hash) => vec![hash],
        None => vec![Some(HashAlg::Sha512), Some(HashAlg::Sha256), None],
    })
}

#[cfg(unix)]
async fn authenticate_agent(
    handle: &mut SshHandle,
    username: &str,
    endpoint: &AgentEndpoint,
    role: ConnectionRole,
    cancellation: Option<&CancellationToken>,
    timeout: Option<Duration>,
) -> Result<bool> {
    let client = match endpoint {
        AgentEndpoint::Default => match controlled(
            AgentClient::connect_env(),
            cancellation,
            timeout,
            "connect SSH agent",
        )
        .await
        {
            Ok(client) => client,
            Err(SftpError::Aborted) => return Err(SftpError::Aborted),
            Err(_) => {
                return Err(SftpError::CredentialRequired {
                    role,
                    method: CredentialMethod::Agent,
                });
            }
        },
        AgentEndpoint::Path(path) => {
            controlled(
                AgentClient::connect_uds(path),
                cancellation,
                timeout,
                "connect SSH agent",
            )
            .await?
        }
    };
    authenticate_with_agent(handle, username, client, cancellation, timeout).await
}

#[cfg(windows)]
async fn authenticate_agent(
    handle: &mut SshHandle,
    username: &str,
    endpoint: &AgentEndpoint,
    role: ConnectionRole,
    cancellation: Option<&CancellationToken>,
    timeout: Option<Duration>,
) -> Result<bool> {
    let path = match endpoint {
        AgentEndpoint::Default => r"\\.\pipe\openssh-ssh-agent",
        AgentEndpoint::Path(path) => path.as_str(),
    };
    let client_result = controlled(
        AgentClient::connect_named_pipe(path),
        cancellation,
        timeout,
        "connect Windows OpenSSH agent",
    )
    .await;
    let client = match client_result {
        Ok(client) => client,
        Err(SftpError::Aborted) => return Err(SftpError::Aborted),
        Err(_) if matches!(endpoint, AgentEndpoint::Default) => {
            return Err(SftpError::CredentialRequired {
                role,
                method: CredentialMethod::Agent,
            });
        }
        Err(error) => return Err(error),
    };
    authenticate_with_agent(handle, username, client, cancellation, timeout).await
}

#[cfg(not(any(unix, windows)))]
async fn authenticate_agent(
    _handle: &mut SshHandle,
    _username: &str,
    _endpoint: &AgentEndpoint,
    _role: ConnectionRole,
    _cancellation: Option<&CancellationToken>,
    _timeout: Option<Duration>,
) -> Result<bool> {
    Err(SftpError::ConnectionFailed(
        "SSH agent authentication is not supported on this platform".to_owned(),
    ))
}

async fn authenticate_with_agent<S>(
    handle: &mut SshHandle,
    username: &str,
    mut agent: AgentClient<S>,
    cancellation: Option<&CancellationToken>,
    timeout: Option<Duration>,
) -> Result<bool>
where
    S: AgentStream + Send + Unpin,
{
    let identities = controlled(
        agent.request_identities(),
        cancellation,
        timeout,
        "read SSH agent identities",
    )
    .await?;
    for identity in identities {
        let public_key = identity.public_key().into_owned();
        let candidates =
            signature_candidates(handle, public_key.algorithm(), cancellation, timeout).await?;
        for hash in candidates {
            let result = match &identity {
                AgentIdentity::PublicKey { key, .. } => {
                    controlled(
                        handle.authenticate_publickey_with(
                            username.to_owned(),
                            key.clone(),
                            hash,
                            &mut agent,
                        ),
                        cancellation,
                        timeout,
                        "authenticate with SSH agent",
                    )
                    .await?
                }
                AgentIdentity::Certificate { certificate, .. } => {
                    controlled(
                        handle.authenticate_certificate_with(
                            username.to_owned(),
                            certificate.clone(),
                            hash,
                            &mut agent,
                        ),
                        cancellation,
                        timeout,
                        "authenticate with SSH agent certificate",
                    )
                    .await?
                }
            };
            if matches!(result, AuthResult::Success) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn controlled<T, E>(
    future: impl Future<Output = std::result::Result<T, E>>,
    cancellation: Option<&CancellationToken>,
    timeout: Option<Duration>,
    operation: &'static str,
) -> Result<T>
where
    E: std::fmt::Display,
{
    let run = async {
        if let Some(timeout) = timeout {
            tokio::time::timeout(timeout, future)
                .await
                .map_err(|_| {
                    SftpError::ConnectionFailed(format!(
                        "{operation} timed out after {} ms",
                        timeout.as_millis()
                    ))
                })?
                .map_err(|error| SftpError::ConnectionFailed(format!("{operation}: {error}")))
        } else {
            future
                .await
                .map_err(|error| SftpError::ConnectionFailed(format!("{operation}: {error}")))
        }
    };
    if let Some(cancellation) = cancellation {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(SftpError::Aborted),
            result = run => result,
        }
    } else {
        run.await
    }
}

fn expand_home(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if text == "~" {
        dirs::home_dir().unwrap_or_else(|| path.to_path_buf())
    } else if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        dirs::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

fn default_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default()
}

async fn disconnect_quietly(handle: &SshHandle) {
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "SFTP closed", "en")
        .await;
}
