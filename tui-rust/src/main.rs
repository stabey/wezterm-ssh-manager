mod app;
mod cli;
mod model;
mod protocol;
mod runtime;
mod sftp;
mod snapshot;
mod types;
mod ui;

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match cli::run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::from(2)
        }
    }
}
