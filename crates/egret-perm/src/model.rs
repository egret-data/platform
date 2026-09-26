//! The `perm` model: grants and named rules over `registry.catalogs`.
//!
//! Types here mirror `migrations/0154_perm_model.sql` one-for-one. Where a
//! field is CHECK-constrained to a closed vocabulary in the database (an
//! enum's stored spelling, a mask/row_filter split, a tag/path split), the
//! Rust type makes the same split unrepresentable at compile time rather than
//! validating it at run time — see `docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md`
//! and the migration's own comments for why ("unrepresentable beats
//! unforgettable").

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The two actions SP4a knows. `read` compiles to `select` on every engine;
/// `write` compiles to `insert,delete` on Trino (no `update` access type
/// exists there) and `insert,update,delete` on StarRocks. DDL is out of
/// scope for SP4a — see the programme's global constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Read,
    Write,
}

impl Action {
    /// The stored spelling in `perm.grants.actions` (a `text[]`, not a
    /// Postgres enum — see the migration).
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Read => "read",
            Action::Write => "write",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "read" => Some(Action::Read),
            "write" => Some(Action::Write),
            _ => None,
        }
    }
}

/// `perm.effect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Allow,
    Deny,
}

impl Effect {
    pub fn as_str(self) -> &'static str {
        match self {
            Effect::Allow => "allow",
            Effect::Deny => "deny",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(Effect::Allow),
            "deny" => Some(Effect::Deny),
            _ => None,
        }
    }
}

/// `perm.principal_kind`. `WorkspaceMembers` is measured, not assumed (D7):
/// Trino has no group provider and egret never pushes one, so a `group`
/// principal matches nobody inside the engine today. `WorkspaceMembers`
/// exists here for a later task to expand into the member list enumerated at
/// compile time; nothing compiles it yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    User,
    Group,
    Role,
    WorkspaceMembers,
}

impl PrincipalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PrincipalKind::User => "user",
            PrincipalKind::Group => "group",
            PrincipalKind::Role => "role",
            PrincipalKind::WorkspaceMembers => "workspace_members",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(PrincipalKind::User),
            "group" => Some(PrincipalKind::Group),
            "role" => Some(PrincipalKind::Role),
            "workspace_members" => Some(PrincipalKind::WorkspaceMembers),
            _ => None,
        }
    }
}

/// One principal reference. `r#ref` is the bare identifier egret's D7
/// measurement settled on for users (a username, never an email) — the store
/// writes it through unchanged for every kind.
///
/// **The OpenAPI name is `PermPrincipal`, not `Principal`, and that is a
/// bug fix** (SP4a Task 10, finding 2). `egret-policy`'s admin surface
/// already exports a DIFFERENT `Principal` — the policy picker's
/// `{kind, name, label}` row (`egret-policy/src/admin/mod.rs`). Two distinct
/// Rust types deriving `ToSchema` under one schema name collapse into one
/// entry in the merged document, and the picker's won: every `$ref` from
/// `PrincipalSet` — and so every `NewGrant.principals` — resolved to
/// `{kind, name, label}`, a shape this type cannot deserialize. A client
/// generated from the committed `openapi.json` could not construct a grant at
/// all. The two names are now distinct, and each DTO refs its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = PermPrincipal)]
pub struct Principal {
    pub kind: PrincipalKind,
    pub r#ref: String,
}

/// A principal subtract: `include` minus `exclude`. This is what lets egret
/// drop Ranger's allow-exception/deny-exception blocks from the authoring
/// surface (see `crates/egret-perm/tests/store.rs`) — both `Grant.principals`
/// and `Rule.exempt` use it, and both round-trip through a `(kind, ref, mode)`
/// table (`perm.grant_principals`, `perm.rule_exemptions`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PrincipalSet {
    pub include: Vec<Principal>,
    pub exclude: Vec<Principal>,
}

/// A path into a catalog. `schema`/`table`/`column` narrow progressively —
/// `perm.grants` and `perm.rules` both CHECK that a level cannot be skipped
/// (a column implies a table implies a schema).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Scope {
    pub catalog_id: Uuid,
    pub schema: Option<String>,
    pub table: Option<String>,
    pub column: Option<String>,
}

/// Why a grant exists. `WorkspaceAssignment` rows are written by the
/// assignment path, not by a human, and are recreated from the assignment on
/// demand (D3) — `perm.grants` enforces at most one such row per
/// `(catalog_id, workspace_id)` via `grants_one_per_assignment`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GrantOrigin {
    Explicit,
    WorkspaceAssignment { workspace_id: Uuid },
}

/// One row of `perm.grants` plus its principal subtract (`perm.grant_principals`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Grant {
    pub id: Uuid,
    pub effect: Effect,
    pub actions: Vec<Action>,
    pub scope: Scope,
    pub principals: PrincipalSet,
    pub origin: GrantOrigin,
}

/// What a rule's scope is selected by. `Tag` references `tags.key` — never
/// `tags.name`, which is mutable and non-unique (D6) — and egret never emits
/// a tag *access* policy for it (D2); only masking/row-filter rules select by
/// tag. `Path` is a `Scope`, same shape as a grant's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Selector {
    Tag { key: String },
    Path(Scope),
}

/// `perm.rule_kind` plus its type-dependent payload, CHECK-constrained in the
/// database (`(kind = 'mask') = (mask_type IS NOT NULL)`, etc.) and made
/// unrepresentable here instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleKind {
    Mask {
        mask_type: String,
        value_expr: Option<String>,
    },
    RowFilter {
        filter_expr: String,
    },
}

/// One row of `perm.rules` plus its exemption set (`perm.rule_exemptions`).
///
/// **`exempt` is a full `PrincipalSet`, including `exclude`, but the
/// compiler cannot express `exclude` here.** The model, the store, and the
/// OpenAPI schema all accept it — `perm.rule_exemptions` has the same
/// `mode` column `perm.grant_principals` does, precisely so "exempt this
/// whole group except these people" round-trips the way a grant's subtract
/// does. But `egret-perm-compile`'s `rules::compile_rule` refuses any `Rule` whose
/// `exempt.exclude` is non-empty (`CompileError::UnexpressibleExemption`):
/// unlike an access-policy item, a Ranger masking/row-filter item
/// (`MaskItemInput`/`RowFilterItemInput`) has no allow/deny-exception block
/// to put a subtract in, so there is no correct Ranger shape for it. The
/// same is true of a `Role` principal in `exempt` (those item types have no
/// `roles` bucket at all). This makes `exempt.exclude`/`Role` an authorable,
/// storable shape that can never compile — a limitation an authoring UI
/// should surface when the rule is written, not one a steward discovers as
/// a 500 the first time the rule is compiled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Rule {
    pub id: Uuid,
    pub name: String,
    pub kind: RuleKind,
    pub selector: Selector,
    pub exempt: PrincipalSet,
}
