use std::fs;
use std::path::Path;

#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::mpsc::{self, Sender};
#[cfg(test)]
use std::thread::{self, JoinHandle};
#[cfg(test)]
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::model::normalize_snapshot;
use crate::types::Snapshot;

pub fn read_snapshot(path: impl AsRef<Path>) -> Result<Snapshot> {
    let path = path.as_ref();
    let source = fs::read_to_string(path)
        .with_context(|| format!("cannot read snapshot {}", path.display()))?;
    let value: Value = serde_json::from_str(&source)
        .with_context(|| format!("cannot parse snapshot {}", path.display()))?;
    Ok(normalize_snapshot(&value))
}

#[cfg(test)]
pub struct SnapshotWatcher {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

#[cfg(test)]
impl SnapshotWatcher {
    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
impl Drop for SnapshotWatcher {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

#[cfg(test)]
fn watch_snapshot_with_interval<F, E>(
    path: impl Into<PathBuf>,
    mut on_snapshot: F,
    mut on_error: E,
    interval: Duration,
) -> SnapshotWatcher
where
    F: FnMut(Snapshot) + Send + 'static,
    E: FnMut(anyhow::Error) + Send + 'static,
{
    let path = path.into();
    let (stop_sender, stop_receiver) = mpsc::channel();
    let thread = thread::spawn(move || {
        let mut last_modified = SystemTime::UNIX_EPOCH;
        loop {
            let result = (|| -> Result<Option<Snapshot>> {
                let metadata = fs::metadata(&path)
                    .with_context(|| format!("cannot inspect snapshot {}", path.display()))?;
                let modified = metadata.modified().with_context(|| {
                    format!("cannot read snapshot modification time {}", path.display())
                })?;
                if modified > last_modified {
                    let snapshot = read_snapshot(&path)?;
                    last_modified = modified;
                    Ok(Some(snapshot))
                } else {
                    Ok(None)
                }
            })();
            match result {
                Ok(Some(snapshot)) => on_snapshot(snapshot),
                Ok(None) => {}
                Err(error) => on_error(error),
            }

            match stop_receiver.recv_timeout(interval) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    });
    SnapshotWatcher {
        stop: Some(stop_sender),
        thread: Some(thread),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::runtime::{cleanup_runtime, create_runtime};

    #[test]
    fn reads_and_normalizes_snapshot() {
        let runtime = create_runtime().unwrap();
        let path = runtime.runtime_dir.join("snapshot.json");
        fs::write(
            &path,
            r#"{"store_path":"profiles.lua","groups":["prod"],"profiles":[]}"#,
        )
        .unwrap();
        let snapshot = read_snapshot(&path).unwrap();
        assert_eq!(snapshot.store_path, "profiles.lua");
        assert_eq!(snapshot.default_where, "tab");
        assert_eq!(snapshot.groups, ["prod"]);
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }

    #[test]
    fn watcher_emits_initial_and_changed_snapshots() {
        let runtime = create_runtime().unwrap();
        let path = runtime.runtime_dir.join("snapshot.json");
        fs::write(&path, r#"{"profiles":[]}"#).unwrap();
        let (updates_tx, updates_rx) = mpsc::channel();
        let watcher = watch_snapshot_with_interval(
            path.clone(),
            move |snapshot| updates_tx.send(snapshot).unwrap(),
            |_| {},
            Duration::from_millis(10),
        );
        assert!(updates_rx.recv_timeout(Duration::from_secs(1)).is_ok());
        thread::sleep(Duration::from_millis(20));
        fs::write(&path, r#"{"groups":["lab"],"profiles":[]}"#).unwrap();
        let changed = updates_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(changed.groups, ["lab"]);
        watcher.stop();
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }
}
