use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde_json::{Number, Value};

use crate::runtime::{RUNTIME_PREFIX, is_valid_token, random_request_id, resolve_path};
use crate::types::{ManagerRequest, RequestEnvelope};

pub type TerminalWriter = Box<dyn FnMut(&str) -> io::Result<()> + Send>;

pub struct RequestProtocol {
    runtime_dir: PathBuf,
    token: String,
    sequence: u64,
    write_terminal: TerminalWriter,
}

impl RequestProtocol {
    pub fn new(runtime_dir: impl AsRef<Path>, token: impl Into<String>) -> Result<Self> {
        Self::with_writer(runtime_dir, token, |data| {
            let stdout = io::stdout();
            let mut output = stdout.lock();
            output.write_all(data.as_bytes())?;
            output.flush()
        })
    }

    pub fn with_writer<F>(
        runtime_dir: impl AsRef<Path>,
        token: impl Into<String>,
        write_terminal: F,
    ) -> Result<Self>
    where
        F: FnMut(&str) -> io::Result<()> + Send + 'static,
    {
        let runtime_dir = resolve_path(runtime_dir)?;
        if !runtime_dir
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.starts_with(RUNTIME_PREFIX))
        {
            bail!("invalid runtime directory");
        }
        let token = token.into();
        if !is_valid_token(&token) {
            bail!("invalid session token");
        }
        Ok(Self {
            runtime_dir,
            token,
            sequence: 0,
            write_terminal: Box::new(write_terminal),
        })
    }

    pub fn emit(&mut self, request: &ManagerRequest) -> Result<RequestEnvelope> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .context("sshmgr request sequence overflow")?;
        let sequence = self.sequence;
        let (request_name, mut file) = self.allocate_request(sequence)?;
        let request_path = self.runtime_dir.join(&request_name);

        let write_result = (|| -> Result<()> {
            let mut body = serde_json::to_value(request)
                .context("cannot encode sshmgr request")?
                .as_object()
                .cloned()
                .ok_or_else(|| anyhow!("sshmgr request must encode as an object"))?;
            body.insert("_session".to_owned(), Value::String(self.token.clone()));
            body.insert("_seq".to_owned(), Value::Number(Number::from(sequence)));
            serde_json::to_writer(&mut file, &body).context("cannot write sshmgr request")?;
            file.sync_all().context("cannot sync sshmgr request")?;
            Ok(())
        })();
        drop(file);
        if let Err(error) = write_result {
            let _ = fs::remove_file(&request_path);
            return Err(error);
        }

        let envelope = RequestEnvelope {
            v: 2,
            token: self.token.clone(),
            seq: sequence,
            request: request_name,
        };
        let encoded = BASE64_STANDARD.encode(
            serde_json::to_vec(&envelope).context("cannot encode sshmgr request envelope")?,
        );
        let wakeup = format!(
            "\u{1b}]1337;SetUserVar=sshmgr={encoded}\u{7}\u{1b}]1337;SetUserVar=sshmgr=\u{7}"
        );
        if let Err(error) = (self.write_terminal)(&wakeup) {
            let _ = fs::remove_file(&request_path);
            return Err(error).context("cannot notify WezTerm about sshmgr request");
        }
        Ok(envelope)
    }

    fn allocate_request(&self, sequence: u64) -> Result<(String, File)> {
        for _ in 0..8 {
            let request = format!("request-{sequence}-{}.json", random_request_id());
            let path = self.runtime_dir.join(&request);
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(path) {
                Ok(file) => return Ok((request, file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error).context("cannot create sshmgr request"),
            }
        }
        bail!("cannot allocate a unique sshmgr request")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::runtime::{cleanup_runtime, create_runtime};

    fn capture_protocol(
        runtime_dir: &Path,
        token: &str,
    ) -> (RequestProtocol, Arc<Mutex<Vec<String>>>) {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&writes);
        let protocol = RequestProtocol::with_writer(runtime_dir, token, move |data| {
            captured.lock().unwrap().push(data.to_owned());
            Ok(())
        })
        .unwrap();
        (protocol, writes)
    }

    #[test]
    fn writes_authenticated_request_before_osc_wakeup() {
        let runtime = create_runtime().unwrap();
        let token = "a".repeat(64);
        let (mut protocol, writes) = capture_protocol(&runtime.runtime_dir, &token);
        let envelope = protocol
            .emit(&ManagerRequest::Connect {
                id: "prod/db".into(),
                where_: "tab".into(),
            })
            .unwrap();

        assert_eq!(envelope.v, 2);
        assert_eq!(envelope.token, token);
        assert_eq!(envelope.seq, 1);
        let request: Value =
            serde_json::from_slice(&fs::read(runtime.runtime_dir.join(&envelope.request)).unwrap())
                .unwrap();
        assert_eq!(
            request,
            serde_json::json!({
                "op": "connect",
                "id": "prod/db",
                "where": "tab",
                "_session": token,
                "_seq": 1
            })
        );

        let output = writes.lock().unwrap();
        assert!(output[0].starts_with("\u{1b}]1337;SetUserVar=sshmgr="));
        assert!(output[0].ends_with("\u{7}\u{1b}]1337;SetUserVar=sshmgr=\u{7}"));
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }

    #[test]
    fn increments_sequence_numbers() {
        let runtime = create_runtime().unwrap();
        let (mut protocol, _) = capture_protocol(&runtime.runtime_dir, &"b".repeat(64));
        assert_eq!(protocol.emit(&ManagerRequest::Reload).unwrap().seq, 1);
        assert_eq!(protocol.emit(&ManagerRequest::Hide).unwrap().seq, 2);
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }

    #[test]
    fn osc_contains_only_the_envelope_not_request_secrets() {
        let runtime = create_runtime().unwrap();
        let token = "e".repeat(64);
        let (mut protocol, writes) = capture_protocol(&runtime.runtime_dir, &token);
        let raw = serde_json::json!({"options": {"password": "s3cret"}})
            .as_object()
            .unwrap()
            .clone();
        let envelope = protocol
            .emit(&ManagerRequest::Upsert { id: None, raw })
            .unwrap();

        let output = writes.lock().unwrap()[0].clone();
        assert!(!output.contains("s3cret"));
        let prefix = "\u{1b}]1337;SetUserVar=sshmgr=";
        let encoded = output
            .strip_prefix(prefix)
            .unwrap()
            .split('\u{7}')
            .next()
            .unwrap();
        let decoded: RequestEnvelope =
            serde_json::from_slice(&BASE64_STANDARD.decode(encoded).unwrap()).unwrap();
        assert_eq!(decoded, envelope);
        let body = fs::read_to_string(runtime.runtime_dir.join(envelope.request)).unwrap();
        assert!(body.contains("s3cret"));
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }

    #[test]
    fn failed_terminal_write_removes_request() {
        let runtime = create_runtime().unwrap();
        let mut protocol =
            RequestProtocol::with_writer(&runtime.runtime_dir, "c".repeat(64), |_| {
                Err(io::Error::other("terminal closed"))
            })
            .unwrap();
        assert!(protocol.emit(&ManagerRequest::Reload).is_err());
        assert_eq!(fs::read_dir(&runtime.runtime_dir).unwrap().count(), 0);
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn request_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let runtime = create_runtime().unwrap();
        let (mut protocol, _) = capture_protocol(&runtime.runtime_dir, &"d".repeat(64));
        let envelope = protocol.emit(&ManagerRequest::Reload).unwrap();
        let mode = fs::metadata(runtime.runtime_dir.join(envelope.request))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }
}
