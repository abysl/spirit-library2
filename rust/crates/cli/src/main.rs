mod cli;
mod control;
mod node_cli;

use clap::Parser;
use cli::{Cli, Command};
use spirit_sdk::BlobStore;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("spirit: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Blob(command) => {
            let store = BlobStore::open(cli::store_dir(cli.store))?;
            cli::run(
                command,
                &store,
                &mut std::io::stdin().lock(),
                &mut std::io::stdout().lock(),
            )
            .map_err(|error| anyhow::anyhow!("{error}"))
        }
        Command::Node(command) => node_cli::node(command, node_cli::directory(cli.node_dir)?).await,
        Command::Mesh(command) => node_cli::mesh(command, node_cli::directory(cli.node_dir)?).await,
    }
}
