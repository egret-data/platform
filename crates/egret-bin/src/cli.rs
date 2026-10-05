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
    /// Print the aggregated OpenAPI 3.1 document as JSON to stdout and exit.
    /// Pure — no database or network — so it can run in CI / codegen.
    Openapi,
    /// Apply database migrations and exit.
    Migrate,
    /// One-off backfill: copy every existing draft's and version's Postgres
    /// content into the object store. Idempotent -- safe to run more than once.
    BackfillAppsStorage,
    /// Bootstrap the initial admin user and default workspace.
    Init {
        /// Create an additional platform_admin even if one already exists.
        #[arg(long)]
        force: bool,
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
}
