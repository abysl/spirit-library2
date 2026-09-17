use crate::node_cli::{MeshCommand, NodeCommand};
use clap::{Parser, Subcommand};
use spirit_sdk::{BlobHash, BlobStore};
use std::error::Error;
use std::io::{Read, Write};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "spirit",
    version,
    about = "a private device mesh and content-addressed blob store"
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        env = "SPIRIT_STORE",
        value_name = "DIR",
        help = "store directory [default: ~/.spirit2/store]"
    )]
    pub store: Option<PathBuf>,

    #[arg(
        long,
        global = true,
        env = "SPIRIT_NODE_DIR",
        value_name = "DIR",
        help = "node directory [default: ~/.spirit2/node]"
    )]
    pub node_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(
        subcommand,
        about = "put bytes into the store and read them back by hash"
    )]
    Blob(BlobCommand),
    #[command(subcommand, about = "initialize, run, pair, and contact devices")]
    Node(NodeCommand),
    #[command(subcommand, about = "create a private mesh and enroll devices")]
    Mesh(MeshCommand),
}

#[derive(Subcommand, Debug)]
pub enum BlobCommand {
    #[command(about = "store bytes, print the blob hash")]
    Put {
        #[arg(value_name = "FILE|-", help = "file to store, or - for stdin")]
        file: PathBuf,
    },
    #[command(about = "read bytes back, verified against their hash")]
    Get {
        #[arg(help = "64 hex characters, as printed by put")]
        hash: BlobHash,
        #[arg(
            long,
            value_name = "FILE",
            help = "write to this file instead of stdout"
        )]
        out: Option<PathBuf>,
    },
}

pub fn store_dir(flag: Option<PathBuf>) -> PathBuf {
    flag.unwrap_or_else(|| {
        let home = std::env::var_os("HOME").expect("HOME is not set");
        PathBuf::from(home).join(".spirit2/store")
    })
}

pub fn run(
    command: BlobCommand,
    store: &BlobStore,
    stdin: &mut impl Read,
    stdout: &mut impl Write,
) -> Result<(), Box<dyn Error>> {
    match command {
        BlobCommand::Put { file } => {
            let bytes = read_input(&file, stdin)?;
            writeln!(stdout, "{}", store.put(&bytes)?)?;
        }
        BlobCommand::Get { hash, out } => {
            let bytes = store.get(hash)?;
            match out {
                Some(path) => {
                    std::fs::write(&path, &bytes)?;
                    writeln!(stdout, "wrote {} bytes to {}", bytes.len(), path.display())?;
                }
                None => stdout.write_all(&bytes)?,
            }
        }
    }
    Ok(())
}

fn read_input(file: &PathBuf, stdin: &mut impl Read) -> std::io::Result<Vec<u8>> {
    if file.as_os_str() == "-" {
        let mut bytes = Vec::new();
        stdin.read_to_end(&mut bytes)?;
        Ok(bytes)
    } else {
        std::fs::read(file)
    }
}
