//! Load BlueRoute durable state with this build's code and print what it holds (P15-004).
//!
//! Used by the package upgrade test: after a real `dpkg` upgrade it loads the state written by
//! the older package exactly as the daemon would at startup, including in-place membership
//! schema migration. It never creates an identity: a missing node identity is an error.
//!
//! Usage: `state_check STATE_DIRECTORY` (the directory that holds `node-id` and
//! `memberships-v1`). Prints `node=ID`, then one `network=ID state=STATE role=ROLE` line per
//! remembered network, sorted; exits non-zero on any load failure.
use std::path::PathBuf;
use std::process::ExitCode;

use blueroute_core::{CoreError, ErrorKind, MembershipRole, NodeId};
use blueroute_linux::{NetworkMembershipStore, NodeIdentityGenerator, NodeIdentityStore};

/// Refuses to invent an identity, so only an existing one can be loaded.
struct NoNewIdentity;

impl NodeIdentityGenerator for NoNewIdentity {
    fn generate(&self) -> Result<NodeId, CoreError> {
        Err(CoreError::new(
            ErrorKind::PersistenceError,
            "no node identity exists in this state directory",
        ))
    }
}

fn role_name(role: Option<MembershipRole>) -> &'static str {
    match role {
        Some(MembershipRole::Host) => "host",
        Some(MembershipRole::Client) => "client",
        None => "none",
    }
}

fn run(directory: PathBuf) -> Result<Vec<String>, CoreError> {
    let node = NodeIdentityStore::with_generator(directory.join("node-id"), NoNewIdentity)
        .load_or_create()?;
    let registry = NetworkMembershipStore::new(directory.join("memberships-v1")).load()?;
    let mut networks: Vec<String> = registry
        .networks()
        .map(|membership| {
            format!(
                "network={} state={:?} role={}",
                membership.network_id,
                membership.state,
                role_name(membership.role)
            )
        })
        .collect();
    networks.sort();
    let mut lines = vec![format!("node={node}")];
    lines.extend(networks);
    Ok(lines)
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(directory), None) = (args.next(), args.next()) else {
        eprintln!("usage: state_check STATE_DIRECTORY");
        return ExitCode::from(2);
    };
    match run(PathBuf::from(directory)) {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("state check failed: {error}");
            ExitCode::FAILURE
        }
    }
}
