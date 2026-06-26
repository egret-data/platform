//! Command-line interface definition for the `egret` binary.

use clap::{Parser, Subcommand};
use uuid::Uuid;

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
    /// Manage per-workspace Iceberg catalog tokens.
    Catalog {
        #[command(subcommand)]
        action: CatalogAction,
    },
}

/// Catalog token operations.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum CatalogAction {
    /// Mint a new catalog token for a workspace.
    ///
    /// The raw token is printed once and never stored — copy it now.
    MintToken {
        /// Workspace slug (e.g. "engineering").
        #[arg(long)]
        workspace: String,
        /// Human-readable label, e.g. "trino-prod".
        #[arg(long, default_value = "default")]
        label: String,
    },
    /// Revoke a catalog token by its id (sets revoked_at = now()).
    RevokeToken {
        /// UUID of the token to revoke (shown when it was minted).
        #[arg(long)]
        id: Uuid,
    },
    /// List all active (non-revoked) tokens for a workspace.
    ListTokens {
        /// Workspace slug.
        #[arg(long)]
        workspace: String,
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

    #[test]
    fn parses_catalog_mint_token() {
        let cli = Cli::parse_from(["egret", "catalog", "mint-token", "--workspace", "eng"]);
        assert_eq!(
            cli.command,
            Command::Catalog {
                action: CatalogAction::MintToken {
                    workspace: "eng".into(),
                    label: "default".into(),
                }
            }
        );
    }

    #[test]
    fn parses_catalog_mint_token_with_label() {
        let cli = Cli::parse_from([
            "egret",
            "catalog",
            "mint-token",
            "--workspace",
            "eng",
            "--label",
            "trino-prod",
        ]);
        assert_eq!(
            cli.command,
            Command::Catalog {
                action: CatalogAction::MintToken {
                    workspace: "eng".into(),
                    label: "trino-prod".into(),
                }
            }
        );
    }

    #[test]
    fn parses_catalog_revoke_token() {
        let id = Uuid::new_v4();
        let cli = Cli::parse_from(["egret", "catalog", "revoke-token", "--id", &id.to_string()]);
        assert_eq!(
            cli.command,
            Command::Catalog {
                action: CatalogAction::RevokeToken { id }
            }
        );
    }

    #[test]
    fn parses_catalog_list_tokens() {
        let cli = Cli::parse_from(["egret", "catalog", "list-tokens", "--workspace", "eng"]);
        assert_eq!(
            cli.command,
            Command::Catalog {
                action: CatalogAction::ListTokens {
                    workspace: "eng".into(),
                }
            }
        );
    }
}
