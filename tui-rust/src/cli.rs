use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::protocol::RequestProtocol;
use crate::runtime::{
    cleanup_runtime, create_runtime, replace_file, resolve_path, validate_runtime,
};
use crate::snapshot::read_snapshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliCommand {
    Run {
        snapshot_path: PathBuf,
    },
    CreateRuntime,
    CleanupRuntime {
        runtime_dir: PathBuf,
    },
    ReplaceFile {
        source: PathBuf,
        destination: PathBuf,
    },
    Help,
}

fn resolved_or_original(path: &str) -> PathBuf {
    resolve_path(path).unwrap_or_else(|_| PathBuf::from(path))
}

pub fn parse_args<I, S>(argv: I) -> CliCommand
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let argv = argv
        .into_iter()
        .map(|value| value.into().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if argv.iter().any(|value| value == "--create-runtime") {
        return CliCommand::CreateRuntime;
    }
    if let Some(index) = argv.iter().position(|value| value == "--cleanup-runtime")
        && let Some(runtime_dir) = argv.get(index + 1)
    {
        return CliCommand::CleanupRuntime {
            runtime_dir: resolved_or_original(runtime_dir),
        };
    }
    if let Some(index) = argv.iter().position(|value| value == "--replace-file")
        && let (Some(source), Some(destination)) = (argv.get(index + 1), argv.get(index + 2))
    {
        return CliCommand::ReplaceFile {
            source: resolved_or_original(source),
            destination: resolved_or_original(destination),
        };
    }
    if let Some(index) = argv.iter().position(|value| value == "--snapshot")
        && let Some(snapshot_path) = argv.get(index + 1)
    {
        return CliCommand::Run {
            snapshot_path: resolved_or_original(snapshot_path),
        };
    }
    CliCommand::Help
}

pub fn run_helper(command: &CliCommand, output: &mut dyn Write) -> Result<bool> {
    match command {
        CliCommand::CreateRuntime => {
            serde_json::to_writer(output, &create_runtime()?)
                .context("cannot encode runtime context")?;
            Ok(true)
        }
        CliCommand::CleanupRuntime { runtime_dir } => {
            cleanup_runtime(runtime_dir)?;
            Ok(true)
        }
        CliCommand::ReplaceFile {
            source,
            destination,
        } => {
            replace_file(source, destination)?;
            Ok(true)
        }
        CliCommand::Run { .. } | CliCommand::Help => Ok(false),
    }
}

pub async fn run() -> Result<()> {
    let command = parse_args(env::args_os().skip(1));
    match command {
        CliCommand::Help => bail!(HELP),
        CliCommand::CreateRuntime
        | CliCommand::CleanupRuntime { .. }
        | CliCommand::ReplaceFile { .. } => {
            let stdout = io::stdout();
            let mut output = stdout.lock();
            run_helper(&command, &mut output)?;
            output.flush().context("cannot flush helper output")?;
            Ok(())
        }
        CliCommand::Run { snapshot_path } => run_tui(snapshot_path).await,
    }
}

async fn run_tui(snapshot_path: PathBuf) -> Result<()> {
    let token = env::var("WEZTERM_SSHMGR_SESSION_TOKEN").unwrap_or_default();
    // Rust 2024 makes environment mutation unsafe because another thread may
    // concurrently inspect the process environment. This is the first action
    // in the single-threaded CLI startup path, before the UI spawns workers.
    #[allow(unused_unsafe)]
    unsafe {
        env::remove_var("WEZTERM_SSHMGR_SESSION_TOKEN");
    }
    let runtime_dir = validate_runtime(&snapshot_path, &token)?;
    let initial_snapshot = read_snapshot(&snapshot_path)?;
    let protocol = RequestProtocol::new(&runtime_dir, token)?;

    let app_result = crate::app::run(snapshot_path, initial_snapshot, protocol).await;
    let cleanup_result = cleanup_runtime(&runtime_dir);
    match (app_result, cleanup_result) {
        (Err(app_error), Err(cleanup_error)) => Err(app_error).context(format!(
            "also failed to clean runtime directory: {cleanup_error:#}"
        )),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

pub const HELP: &str = "wezterm-ssh-manager Rust TUI\n\nUsage:\n  sshmgr-tui --snapshot <runtime/snapshot.json>\n  sshmgr-tui --create-runtime\n  sshmgr-tui --cleanup-runtime <runtime-dir>\n  sshmgr-tui --replace-file <source> <destination>\n";

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::runtime::{RuntimeContext, cleanup_runtime, is_valid_token};

    #[test]
    fn parses_normal_and_helper_modes() {
        assert_eq!(parse_args(["--create-runtime"]), CliCommand::CreateRuntime);
        assert!(matches!(
            parse_args(["--snapshot", "./snapshot.json"]),
            CliCommand::Run { .. }
        ));
        assert!(matches!(
            parse_args(["--cleanup-runtime", "/tmp/wezterm-sshmgr-one"]),
            CliCommand::CleanupRuntime { .. }
        ));
        assert!(matches!(
            parse_args(["--replace-file", "a", "b"]),
            CliCommand::ReplaceFile { .. }
        ));
        assert_eq!(parse_args(Vec::<String>::new()), CliCommand::Help);
    }

    #[test]
    fn helper_creates_runtime_as_json() {
        let mut output = Vec::new();
        assert!(run_helper(&CliCommand::CreateRuntime, &mut output).unwrap());
        let context: RuntimeContext = serde_json::from_slice(&output).unwrap();
        assert!(is_valid_token(&context.token));
        cleanup_runtime(context.runtime_dir).unwrap();
    }

    #[test]
    fn helper_replaces_files() {
        let runtime = create_runtime().unwrap();
        let source = runtime.runtime_dir.join("snapshot.json.tmp-cli");
        let destination = runtime.runtime_dir.join("snapshot.json");
        fs::write(&source, "new").unwrap();
        fs::write(&destination, "old").unwrap();
        let mut output = Vec::new();
        assert!(
            run_helper(
                &CliCommand::ReplaceFile {
                    source,
                    destination: destination.clone(),
                },
                &mut output,
            )
            .unwrap()
        );
        assert_eq!(fs::read_to_string(destination).unwrap(), "new");
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }

    #[test]
    fn help_documents_compatible_flags() {
        for flag in [
            "--snapshot",
            "--create-runtime",
            "--cleanup-runtime",
            "--replace-file",
        ] {
            assert!(HELP.contains(flag));
        }
    }
}
