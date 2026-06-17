//! Command-line interface definition for the `egret` binary.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "egret", version, about = "Egret data portal")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Command {
    /// Run the HTTP server.
    Serve,
    /// Apply database migrations and exit.
    Migrate,
    /// Bootstrap the initial admin user and default workspace.
    Init {
        /// Create an additional platform_admin even if one already exists.
        #[arg(long)]
        force: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_subcommand() {
        assert_eq!(Cli::parse_from(["egret", "serve"]).command, Command::Serve);
        assert_eq!(
            Cli::parse_from(["egret", "migrate"]).command,
            Command::Migrate
        );
        assert_eq!(
            Cli::parse_from(["egret", "init"]).command,
            Command::Init { force: false }
        );
        assert_eq!(
            Cli::parse_from(["egret", "init", "--force"]).command,
            Command::Init { force: true }
        );
    }

    #[test]
    fn unknown_subcommand_is_rejected() {
        assert!(Cli::try_parse_from(["egret", "nope"]).is_err());
    }
}
