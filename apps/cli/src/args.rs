//! Command-line definition (P10-001). Parsing only: no networking or daemon logic.

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "blueroute",
    version,
    about = "Build a network over Bluetooth with BlueRoute",
    after_help = "Exit codes: 0 ok, 1 operation failed, 2 usage, 3 daemon unavailable, \
                  4 unsupported on this machine, 5 not authorized."
)]
pub struct Cli {
    /// Print machine-readable JSON instead of text.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show whether BlueRoute is running and which network this computer is in.
    Status,
    /// What this computer can do.
    Capability {
        #[command(subcommand)]
        action: CapabilityAction,
    },
    /// Computers in your network.
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },
    /// Create, find, join and leave networks.
    Network {
        #[command(subcommand)]
        action: NetworkAction,
    },
    /// Decide which computers may join.
    Trust {
        #[command(subcommand)]
        action: TrustAction,
    },
    /// Troubleshooting information.
    Diagnostics(DiagnosticsArgs),
}

#[derive(Debug, Subcommand)]
pub enum CapabilityAction {
    Show,
}

#[derive(Debug, Subcommand)]
pub enum NodeAction {
    List,
    Show {
        /// Node id (32 hex characters).
        id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum NetworkAction {
    List,
    /// Create a network on this computer.
    Create {
        name: String,
    },
    /// Join a nearby network by id.
    Join {
        /// Network id (32 hex characters).
        id: String,
    },
    /// Leave the current network.
    Leave,
    /// Look for nearby networks (use --stop to stop looking).
    Discover {
        #[arg(long)]
        stop: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum TrustAction {
    /// Allow a computer to join. Nothing is ever approved automatically.
    Approve {
        /// Node id (32 hex characters).
        id: String,
    },
    /// Remove approval and forget a computer.
    Forget {
        /// Node id (32 hex characters).
        id: String,
    },
}

#[derive(Debug, Args)]
pub struct DiagnosticsArgs {
    /// Show every detail instead of a short summary.
    #[arg(long)]
    pub detailed: bool,
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn global_json_flag_works_before_and_after_the_subcommand() {
        for argv in [
            ["blueroute", "--json", "status"],
            ["blueroute", "status", "--json"],
        ] {
            assert!(Cli::try_parse_from(argv).unwrap().json);
        }
        assert!(!Cli::try_parse_from(["blueroute", "status"]).unwrap().json);
    }

    #[test]
    fn unknown_commands_and_missing_arguments_are_usage_errors() {
        for argv in [
            vec!["blueroute"],
            vec!["blueroute", "frobnicate"],
            vec!["blueroute", "network", "join"],
            vec!["blueroute", "trust", "approve"],
        ] {
            let error = Cli::try_parse_from(argv).unwrap_err();
            assert_eq!(error.exit_code(), 2);
        }
    }

    #[test]
    fn help_and_version_succeed() {
        for flag in ["--help", "--version"] {
            let error = Cli::try_parse_from(["blueroute", flag]).unwrap_err();
            assert_eq!(error.exit_code(), 0);
        }
    }
}
