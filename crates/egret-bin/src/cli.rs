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
    /// Workspace administration (membership).
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },
}

/// Workspace membership operations.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum WorkspaceAction {
    /// Grant an IdP group a role in a workspace (idempotent).
    AddGroup {
        /// Workspace slug (e.g. "engineering").
        #[arg(long)]
        workspace: String,
        /// IdP group name as it appears in the `groups` claim (e.g. "data-team").
        #[arg(long)]
        group: String,
        /// Workspace role: ws_admin | ws_editor | ws_viewer.
        #[arg(long, default_value = "ws_editor")]
        role: String,
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
        /// Name of the warehouse (within the workspace) this token is scoped to.
        /// A workspace now owns N user-managed warehouses; a token authenticates
        /// against exactly one. Defaults to "default".
        #[arg(long, default_value = "default")]
        warehouse: String,
        /// Human-readable label, e.g. "trino-prod".
        #[arg(long, default_value = "default")]
        label: String,
        /// Comma-separated permissions to grant: `read`, `write`, or both.
        /// Defaults to `read,write` (full access). Example: `--perms read`.
        #[arg(long, default_value = "read,write")]
        perms: String,
        /// Comma-separated namespace names this token is scoped to.
        /// Omit (or pass empty) to grant access to all namespaces.
        /// Example: `--namespaces marts,staging`.
        #[arg(long, default_value = "")]
        namespaces: String,
    },
    /// Revoke a catalog token by its id (sets revoked_at = now()).
    RevokeToken {
        /// UUID of the token to revoke (shown when it was minted).
        #[arg(long)]
        id: Uuid,
    },
    /// List all active (non-revoked) tokens for a workspace warehouse.
    ListTokens {
        /// Workspace slug.
        #[arg(long)]
        workspace: String,
        /// Name of the warehouse (within the workspace) whose tokens to list.
        /// Defaults to "default".
        #[arg(long, default_value = "default")]
        warehouse: String,
    },
}

/// Parse the `--namespaces` flag value into a `Vec<String>`.
///
/// Splits on `,`, trims whitespace from each token, and drops empty entries.
/// An empty or absent value (default `""`) returns `vec![]` meaning all namespaces.
pub fn parse_namespaces(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
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
                    warehouse: "default".into(),
                    label: "default".into(),
                    perms: "read,write".into(),
                    namespaces: "".into(),
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
                    warehouse: "default".into(),
                    label: "trino-prod".into(),
                    perms: "read,write".into(),
                    namespaces: "".into(),
                }
            }
        );
    }

    #[test]
    fn mint_token_parses_perms() {
        // Explicit --perms read
        let cli = Cli::parse_from([
            "egret",
            "catalog",
            "mint-token",
            "--workspace",
            "eng",
            "--perms",
            "read",
        ]);
        assert_eq!(
            cli.command,
            Command::Catalog {
                action: CatalogAction::MintToken {
                    workspace: "eng".into(),
                    warehouse: "default".into(),
                    label: "default".into(),
                    perms: "read".into(),
                    namespaces: "".into(),
                }
            }
        );

        // Explicit --perms write
        let cli = Cli::parse_from([
            "egret",
            "catalog",
            "mint-token",
            "--workspace",
            "eng",
            "--perms",
            "write",
        ]);
        assert_eq!(
            cli.command,
            Command::Catalog {
                action: CatalogAction::MintToken {
                    workspace: "eng".into(),
                    warehouse: "default".into(),
                    label: "default".into(),
                    perms: "write".into(),
                    namespaces: "".into(),
                }
            }
        );

        // Default (omitted) is read,write
        let cli = Cli::parse_from(["egret", "catalog", "mint-token", "--workspace", "eng"]);
        if let Command::Catalog {
            action: CatalogAction::MintToken { perms, .. },
        } = cli.command
        {
            assert_eq!(perms, "read,write");
        } else {
            panic!("unexpected command variant");
        }
    }

    #[test]
    fn parse_namespaces_trims_and_splits() {
        use super::parse_namespaces;

        // Normal comma-separated with spaces
        assert_eq!(
            parse_namespaces("marts, staging"),
            vec!["marts".to_string(), "staging".to_string()]
        );

        // Absent / empty → all namespaces
        assert_eq!(parse_namespaces(""), Vec::<String>::new());

        // Double comma drops empty segment
        assert_eq!(
            parse_namespaces("a,,b"),
            vec!["a".to_string(), "b".to_string()]
        );

        // Single namespace
        assert_eq!(parse_namespaces("marts"), vec!["marts".to_string()]);
    }

    #[test]
    fn mint_token_parses_namespaces_flag() {
        let cli = Cli::parse_from([
            "egret",
            "catalog",
            "mint-token",
            "--workspace",
            "eng",
            "--namespaces",
            "marts,staging",
        ]);
        if let Command::Catalog {
            action: CatalogAction::MintToken { namespaces, .. },
        } = cli.command
        {
            // Exercise the flag end-to-end: clap captures the raw string,
            // then the parser splits it the way the handler will.
            assert_eq!(parse_namespaces(&namespaces), vec!["marts", "staging"]);
        } else {
            panic!("unexpected command variant");
        }
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
    fn parses_workspace_add_group() {
        // Default role (omitted) is ws_editor.
        let cli = Cli::parse_from([
            "egret",
            "workspace",
            "add-group",
            "--workspace",
            "engineering",
            "--group",
            "data-team",
        ]);
        assert_eq!(
            cli.command,
            Command::Workspace {
                action: WorkspaceAction::AddGroup {
                    workspace: "engineering".into(),
                    group: "data-team".into(),
                    role: "ws_editor".into(),
                }
            }
        );

        // Explicit --role is captured.
        let cli = Cli::parse_from([
            "egret",
            "workspace",
            "add-group",
            "--workspace",
            "engineering",
            "--group",
            "data-team",
            "--role",
            "ws_admin",
        ]);
        assert_eq!(
            cli.command,
            Command::Workspace {
                action: WorkspaceAction::AddGroup {
                    workspace: "engineering".into(),
                    group: "data-team".into(),
                    role: "ws_admin".into(),
                }
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
                    warehouse: "default".into(),
                }
            }
        );
    }
}
