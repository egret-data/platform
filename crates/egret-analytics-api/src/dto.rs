//! Pure-serde notebook-docs DTOs shared across the Analytics and Kernel seams.
//!
//! Moved verbatim (same fields + serde attrs) from `egret-notebook` so the wire
//! shapes are unchanged; `egret-notebook` re-exports these to keep its handlers
//! compiling. The trimming/persistence logic (`cap_outputs`) stays in
//! `egret-notebook` — only the TYPES live here.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Summary view of a notebook (no cells). Used by ownership/IDOR gates.
#[derive(Debug, Clone, Serialize)]
pub struct NotebookSummary {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub is_public: bool,
    pub owner_user_id: Uuid,
    pub is_owner: bool,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Whether the requesting viewer has bookmarked this notebook.
    pub bookmarked: bool,
    /// Folder this notebook lives in; `None` = Uncategorized.
    pub folder_id: Option<Uuid>,
    /// Soft-delete marker; `None` = live, `Some(..)` = trashed.
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// An engine (DB connection) option shown in notebook settings.
#[derive(Debug, Clone, Serialize)]
pub struct EngineOption {
    pub id: Uuid,
    pub name: String,
    /// Engine type label: "trino" or "starrocks".
    pub engine: String,
    /// Whether this engine is currently enabled for notebooks in this workspace.
    pub enabled: bool,
}

/// Full kernel manifest returned by `GET /ws/{workspace}/notebook-runtime` and
/// consumed by `egret-kernel` to build the kernel process environment.
#[derive(Debug, Clone, Serialize)]
pub struct NotebookRuntime {
    /// The caller's stable username (email local-part, collision-suffixed).
    pub username: String,
    /// Workspace engines (DB connections) enabled for notebooks.
    pub engines: Vec<EngineOption>,
    /// Short-lived per-user engine JWT the kernel presents to query engines.
    /// `None` when the caller has no stored refresh token (e.g. password/local
    /// users or OIDC logins without an offline token) — minting is best-effort
    /// since no P1 engine consumes this yet.
    pub engine_jwt: Option<String>,
    /// Expiry of `engine_jwt` (RFC3339); kernels re-mint before this time.
    /// `None` whenever `engine_jwt` is `None`.
    pub engine_jwt_expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The `notebook_cells.last_output` contract: a trimmed nbformat-style output
/// list, shared by the E-P2 recorder (writer) and the frontend renderer (reader).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CellOutput {
    Stream {
        name: String,
        text: String,
    },
    ExecuteResult {
        data: serde_json::Value,
    },
    DisplayData {
        data: serde_json::Value,
    },
    Error {
        ename: String,
        evalue: String,
        traceback: Vec<String>,
    },
    Truncated {
        dropped: usize,
    },
}
