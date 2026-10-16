//! `PolicyPort` — the cross-seam interface into the Policy/Ranger store.
//!
//! Only the calls that reach the policy store from *outside* the seam belong
//! here (today: engine-mgmt's engine-create auto-bind). In-seam callers (the
//! Ranger router, the admin policy editor) use the concrete store directly.
//! At a real service split, an HTTP client implements this same trait over the
//! same DTOs — no call-site churn.
use crate::dto::PolicyService;
use serde_json::Value;

/// Transport-neutral policy-seam error (no sqlx leak across the boundary).
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("policy resource not found: {0}")]
    NotFound(String),
    #[error("policy resource already exists: {0}")]
    AlreadyExists(String),
    #[error("policy store error: {0}")]
    Backend(String),
}

#[async_trait::async_trait]
pub trait PolicyPort: Send + Sync {
    /// Ensure the built-in `trino` service-def is seeded. Idempotent.
    async fn ensure_trino_def(&self) -> Result<(), PolicyError>;

    /// Ensure the built-in `tag` service-def is seeded. Idempotent. Required
    /// before a tag service instance can be created for tag-based policies.
    async fn ensure_tag_def(&self) -> Result<(), PolicyError>;

    /// List all policy profiles (services).
    async fn list_services(&self) -> Result<Vec<PolicyService>, PolicyError>;

    /// Create a new policy profile. Errors `AlreadyExists` on a name clash.
    async fn create_service(
        &self,
        name: &str,
        type_: &str,
        config: &Value,
    ) -> Result<PolicyService, PolicyError>;

    /// Delete a policy profile (service) and, by cascade, its policies. Errors
    /// `NotFound` when no such id exists. Used to undo a profile an engine create
    /// minted before failing.
    async fn delete_service(&self, id: i64) -> Result<(), PolicyError>;

    /// The name of the tag service `service_id`'s profile can actually use, or
    /// `None` when it has none it can.
    ///
    /// "Usable" means BOTH halves hold: the profile's `config_jsonb` carries a
    /// `tagService`, AND that name resolves to a real `authz_services` row.
    /// Either half missing is the same outcome for the compiler — it is exactly
    /// the condition `egret_perm_compile`'s `CompileError::MissingTagService`
    /// reports ("unconfigured, or naming a profile that does not exist") — so
    /// this collapses them into one answer rather than making every caller
    /// re-derive the distinction.
    ///
    /// **Why the engine seam needs it.** Every tag-selector rule (mask, row
    /// filter) compiles into the profile's TAG service. A profile with no
    /// usable one silently compiles zero tag policies, so masked columns are
    /// served raw. `PUT /engines/{id}` checks this before it binds.
    async fn usable_tag_service(&self, service_id: i64) -> Result<Option<String>, PolicyError>;

    /// Look up a policy profile by name; `None` when it doesn't exist (unlike a
    /// hard-error lookup). Used by engine provisioning to seed delegator policies
    /// onto a bound Ranger service without failing when the engine is unbound.
    async fn get_service_opt(&self, name: &str) -> Result<Option<PolicyService>, PolicyError>;

    /// Whether a policy of this name already exists under the service (idempotent
    /// seeding: skip re-creating a policy that's already there).
    async fn policy_exists(&self, service_id: i64, name: &str) -> Result<bool, PolicyError>;

    /// Create a policy under a service (stores the raw Ranger policy body). Used
    /// by engine provisioning to seed the impersonate/query-timeout policies.
    async fn create_policy(
        &self,
        service_id: i64,
        name: &str,
        policy: &Value,
    ) -> Result<(), PolicyError>;

    /// [`create_policy`](Self::create_policy) for egret's OWN infrastructure
    /// policies: same semantics (`AlreadyExists` on a clash), but the row is
    /// stamped `managed_by = 'infra'`. Errors `Backend` for a name that
    /// `infra_names::is_egret_infrastructure_policy` does not recognise on the profile's type.
    /// Used by engine provisioning for the delegator's two fixed policies.
    async fn create_infra_policy(
        &self,
        service_id: i64,
        name: &str,
        policy: &Value,
    ) -> Result<(), PolicyError>;

    /// Insert-or-replace one of egret's OWN infrastructure policies, stamped
    /// `managed_by = 'infra'` (re-stamping a row an operator pre-created under
    /// that name). Unlike [`create_infra_policy`](Self::create_infra_policy) it
    /// is NOT refused on a profile the SP4a compiler has claimed
    /// (`origin = 'generated'`): that guard is for hand-written policies, and
    /// egret's plumbing must land on a compiled profile too. Errors `Backend`
    /// for a name `infra_names::is_egret_infrastructure_policy` does not
    /// recognise on the profile's type. Used by engine provisioning for the
    /// delegator grant and the per-user baseline.
    async fn upsert_infra_policy(
        &self,
        service_id: i64,
        name: &str,
        policy: &Value,
    ) -> Result<(), PolicyError>;

    /// Seed idempotent admin all-access policies for `users` on the given
    /// service type ("trino"/"starrocks"). Returns the count written.
    async fn seed_admin_all_access(
        &self,
        service_id: i64,
        service_name: &str,
        service_type: &str,
        users: &[String],
    ) -> Result<usize, PolicyError>;
}
