use crate::control::{self, Operation, Reply};
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use qrcode::{
    render::{svg, unicode::Dense1x2},
    QrCode,
};
use spirit_sdk::{LeftMesh, Node, NodeInfo};
use std::io::IsTerminal;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum NodeCommand {
    #[command(about = "create a persistent device identity")]
    Init {
        #[arg(long, help = "device nickname [default: hostname]")]
        name: Option<String>,
    },
    #[command(about = "print this device's public ID")]
    Id,
    #[command(about = "run this node until interrupted")]
    Serve {
        #[arg(long, help = "use loopback only, without relays or discovery")]
        local: bool,
    },
    #[command(about = "open a single-use pairing window and display its QR code")]
    Pair {
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600))]
        ttl_seconds: u64,
        #[arg(long, help = "print only the ticket")]
        no_qr: bool,
        #[arg(
            long,
            value_name = "FILE",
            help = "also save the QR code as an SVG image"
        )]
        qr_svg: Option<PathBuf>,
    },
    #[command(about = "ping a mesh member by nickname or public ID")]
    Ping { device: String },
}

#[derive(Debug, Subcommand)]
pub enum MeshCommand {
    #[command(about = "create a mesh with this device as its first member")]
    Create {
        #[arg(long)]
        name: String,
    },
    #[command(about = "enroll a device using its pairing ticket")]
    Add { ticket: String },
    #[command(about = "leave this device's mesh; members must enroll it again to readmit it")]
    Leave,
    #[command(about = "show this device's membership and service status")]
    Status,
    #[command(about = "list known mesh members")]
    Members {
        #[arg(long, help = "include full device IDs for disambiguation")]
        ids: bool,
    },
}

pub fn directory(flag: Option<PathBuf>) -> Result<PathBuf> {
    match flag {
        Some(path) => Ok(path),
        None => Ok(PathBuf::from(
            std::env::var_os("HOME").context("HOME is not set; use --node-dir")?,
        )
        .join(".spirit2/node")),
    }
}

pub async fn node(command: NodeCommand, root: PathBuf) -> Result<()> {
    match command {
        NodeCommand::Init { name } => {
            let name = match name {
                Some(name) => name,
                None => hostname::get()?
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("hostname is not UTF-8; use --name"))?,
            };
            let info = Node::init(&root, &name)?;
            println!("Initialized {}.", info.name);
        }
        NodeCommand::Id => println!("{}", Node::read_info(&root)?.id),
        NodeCommand::Serve { local } => control::serve(root, local).await?,
        NodeCommand::Pair {
            ttl_seconds,
            no_qr,
            qr_svg,
        } => {
            let Reply::Ticket(ticket) =
                control::request(&root, Operation::Pair { ttl_seconds }).await?
            else {
                bail!("unexpected node response");
            };
            let qr = QrCode::new(ticket.as_bytes())?;
            if let Some(path) = qr_svg {
                spirit_sdk::write_private(
                    &path,
                    qr.render::<svg::Color>()
                        .min_dimensions(512, 512)
                        .build()
                        .as_bytes(),
                )?;
            }
            if !no_qr {
                println!(
                    "Ready to pair {}. Ticket expires in {ttl_seconds} seconds.\n",
                    Node::read_info(&root)?.name
                );
                let image = qr.render::<Dense1x2>().quiet_zone(true).build();
                if std::io::stdout().is_terminal() {
                    println!("\x1b[30;47m{image}\x1b[0m\n");
                } else {
                    println!("{image}\n");
                }
            }
            println!("{ticket}");
        }
        NodeCommand::Ping { device } => {
            let Reply::Pong(pong) = control::request(&root, Operation::Ping { device }).await?
            else {
                bail!("unexpected node response");
            };
            println!("pong from {} in {} ms", pong.name, pong.elapsed_ms);
        }
    }
    Ok(())
}

async fn info(root: &std::path::Path) -> Result<(NodeInfo, bool)> {
    match control::request_if_running(root, Operation::Info).await? {
        Some(Reply::Info(info)) => Ok((info, true)),
        None => Ok((Node::read_info(root)?, false)),
        _ => bail!("unexpected node response"),
    }
}

fn departure_message(left: &LeftMesh) -> String {
    let name = &left.mesh_name;
    match (left.notified_members, left.remaining_members) {
        (_, 0) => format!("Left {name}. No other members remained."),
        (notified, remaining) if notified == remaining => {
            format!("Left {name} and notified its {remaining} remaining members.")
        }
        (notified, remaining) => format!(
            "Left {name}. Notified {notified} of {remaining} remaining members; the others learn this when they next reach this device while it is serving."
        ),
    }
}

pub async fn mesh(command: MeshCommand, root: PathBuf) -> Result<()> {
    match command {
        MeshCommand::Create { name } => {
            let info =
                match control::request_if_running(&root, Operation::Create { name: name.clone() })
                    .await?
                {
                    Some(Reply::Info(info)) => info,
                    None => Node::create_mesh(&root, &name)?,
                    _ => bail!("unexpected node response"),
                };
            println!(
                "Created {} with {} as its first member.",
                info.mesh_name.unwrap(),
                info.name
            );
        }
        MeshCommand::Add { ticket } => {
            let Reply::Added(member) = control::request(&root, Operation::Add { ticket }).await?
            else {
                bail!("unexpected node response");
            };
            println!(
                "Added {} to {}.",
                member.name,
                info(&root).await?.0.mesh_name.unwrap()
            );
        }
        MeshCommand::Leave => {
            let left = match control::request_if_running(&root, Operation::Leave).await? {
                Some(Reply::Left(left)) => left,
                None => Node::leave_mesh(&root)?,
                _ => bail!("unexpected node response"),
            };
            println!("{}", departure_message(&left));
        }
        MeshCommand::Status => {
            let (info, running) = info(&root).await?;
            println!("Device: {}", info.name);
            println!("Node: {}", if running { "running" } else { "stopped" });
            println!(
                "Mesh: {}",
                info.mesh_name.as_deref().unwrap_or("not enrolled")
            );
            if let Some(mesh_id) = info.mesh_id {
                println!("Mesh ID: {mesh_id}");
            }
            println!("Members: {}", info.members.len());
        }
        MeshCommand::Members { ids } => {
            let (mut info, _) = info(&root).await?;
            if info.mesh_id.is_none() {
                bail!("device is not enrolled in a mesh");
            }
            info.members
                .sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
            println!(
                "{}",
                if ids {
                    "NICKNAME\tDEVICE ID"
                } else {
                    "NICKNAME"
                }
            );
            for member in info.members {
                let suffix = if member.id == info.id {
                    " (this device)"
                } else {
                    ""
                };
                if ids {
                    println!("{}\t{}{}", member.name, member.id, suffix);
                } else {
                    println!("{}{suffix}", member.name);
                }
            }
        }
    }
    Ok(())
}
