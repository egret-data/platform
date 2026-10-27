//! Production bootstrap of lakekeeper's default project at egret startup.
//!
//! It builds its bootstrap principal through **public, non-test** lakekeeper
//! APIs only (no `RequestMetadata::test_user`, which is `test-utils`-gated), so
//! the prod build never references `test_user`.
//!
//! ## What lakekeeper's `bootstrap` actually requires (CE-2 T3 discovery)
//!
//! `server::Service::bootstrap` (the static method on
//! `ApiServer<C, A, S>`) does three things relevant here:
//!
//! 1. **Authz**: `authorizer.can_bootstrap(&request_metadata)`. Under
//!    egret's [`crate::EgretAuthorizer`] this always passes.
//! 2. **Seeds the default project**: when `DEFAULT_PROJECT_ID` is set (the OSS
//!    default → nil UUID), `bootstrap` creates the "Default Project" if absent.
//!    This is the outcome egret needs.
//! 3. **Registers the calling user as the first/global-admin user** — but
//!    *only* `if request_metadata.is_authenticated()`. It then calls
//!    `parse_create_user_request`, which derives the new user's id from
//!    `request_metadata.user_id()` (the request carries no explicit id at
//!    bootstrap). If the actor is authenticated but `user_id()` is `None`, it
//!    fails with `MissingUserId` (HTTP 400).
//!
//! The in-process `RequestMetadata::new_lakekeeper_internal` actor
//! (`InternalActor::LakekeeperInternal`) *is* authenticated yet its `user_id()`
//! is `None` — so it hits exactly the `MissingUserId` path. That is the CE-1
//! finding. We therefore need an actor that is `Actor::Principal(UserId)`,
//! carrying a real, **stable** `UserId`.
//!
//! ## The public path to a `UserId`-bearing `RequestMetadata` (no test-utils)
//!
//! All of these are public, non-`#[cfg(test)]`, non-`test-utils` lakekeeper /
//! `limes` APIs:
//!
//! * `RequestMetadata::new_lakekeeper_internal(Uuid)` — public constructor.
//! * `RequestMetadata::set_authentication(actor: Actor, authentication)` —
//!   public setter that installs the actor; afterwards `user_id()` returns the
//!   principal's id.
//! * `Actor::Principal(UserId)` — public enum variant
//!   (`lakekeeper::service::Actor`).
//! * `UserId::try_from("<idp>~<subject>")` — public `TryFrom<&str>`
//!   (`lakekeeper::service::UserId`).
//! * `limes::Authentication::builder()…build()` — public `TypedBuilder`
//!   (re-exported as `lakekeeper::limes`).
//!
//! `test_user` / `new_test` (the only constructors that bake in a `UserId`
//! directly) are `#[cfg(any(test, feature = "test-utils"))]` — so we deliberately
//! reconstruct the same shape from the public setter instead.
//!
//! ## Stable `UserId` derivation
//!
//! lakekeeper's `bootstrap` registers the calling user as the catalog's first
//! user (a global admin) and keys it by `UserId`. For that identity to be the
//! same on every boot, the `UserId` must be deterministic. We derive it as
//! `oidc~<uuidv5>` where the UUID is a name-based (v5) hash of the configured
//! `catalog.system_user` under a fixed egret namespace. Properties:
//!
//! * **Deterministic**: same `system_user` ⇒ same `UserId`, every process, every
//!   boot. Re-running `bootstrap` finds the same admin already registered.
//! * **Valid**: a v5 UUID string is 36 chars of `[0-9a-f-]` — always passes
//!   lakekeeper's `UserId` validation (non-empty, < 128 chars, no control
//!   chars) regardless of what odd characters the operator put in
//!   `system_user`.
//! * **Namespaced**: the `oidc~` IdP prefix matches lakekeeper's expected
//!   `<idp_id>~<subject>` form and keeps the synthetic principal in a sane
//!   identity space.

use crate::mount::EngineAuthorizer;
use anyhow::Context as _;
use lakekeeper::api::management::v1::server::{BootstrapRequest, Service as ServerService};
use lakekeeper::api::management::v1::user::UserType;
use lakekeeper::api::management::v1::ApiServer;
use lakekeeper::api::ApiContext;
use lakekeeper::api::RequestMetadata;
use lakekeeper::limes::Authentication;
use lakekeeper::service::{Actor, UserId};
use lakekeeper_storage_postgres::{PostgresBackend, SecretsState};
use uuid::Uuid;

use crate::mount::EngineState;

/// `ApiServer` specialized to egret's concrete backends — the type whose
/// `Service` trait impls carry `bootstrap`.
type EngineApiServer = ApiServer<PostgresBackend, EngineAuthorizer, SecretsState>;

/// IdP id used for the synthetic egret-system principal. lakekeeper `UserId`s
/// are `<idp_id>~<subject_in_idp>`; `oidc` is lakekeeper's canonical OIDC IdP id.
const SYSTEM_USER_IDP: &str = "oidc";

/// Fixed namespace UUID for deriving the system principal's subject from
/// `catalog.system_user` (UUIDv5).
///
/// Generated ONCE for egret (RFC-4122 random v4). It MUST NEVER change
/// after first deployment: changing it re-derives every system `UserId`, so
/// `bootstrap` would no longer find the admin user it previously registered —
/// breaking bootstrap idempotency.
const EGRET_SYSTEM_USER_NAMESPACE: Uuid =
    Uuid::from_u128(0x6b3f_2a17_8c54_4d9e_b1a0_7e5c_2f48_9d61);

/// Derive the stable lakekeeper [`UserId`] for the configured system principal.
///
/// `system_user` is hashed (UUIDv5, fixed namespace) into a deterministic
/// subject, yielding `oidc~<uuid>`. Same input ⇒ same `UserId` on every boot.
///
/// # Errors
/// Only fails if lakekeeper rejects the (always-valid) derived id — effectively
/// infallible, but surfaced as a `Result` to avoid an `unwrap` on external API.
pub(crate) fn system_user_id(system_user: &str) -> anyhow::Result<UserId> {
    let subject = Uuid::new_v5(&EGRET_SYSTEM_USER_NAMESPACE, system_user.as_bytes());
    let id = format!("{SYSTEM_USER_IDP}~{subject}");
    UserId::try_from(id.as_str())
        .map_err(|e| anyhow::anyhow!("constructing system-user UserId `{id}`: {e:?}"))
}

/// Build a prod (non-test-utils) [`RequestMetadata`] carrying the stable
/// system-admin principal, using only public lakekeeper/limes APIs.
pub(crate) fn system_request_metadata(system_user: &str) -> anyhow::Result<RequestMetadata> {
    let user_id = system_user_id(system_user)?;

    // Minimal authentication record describing the same principal. `bootstrap`
    // self-provisions the calling user, so only the subject is load-bearing; the
    // name/email here are descriptive fallbacks the bootstrap request overrides.
    let authentication = Authentication::builder()
        .token_header(None)
        .claims(serde_json::json!({}))
        .subject(user_id.clone().into())
        .name(Some("Egret System".to_string()))
        .email(None)
        .principal_type(None)
        .build();

    // `new_lakekeeper_internal` gives a public base; `set_authentication` then
    // installs `Actor::Principal(user_id)`, so `user_id()` returns Some(..) —
    // exactly what `bootstrap`'s user registration requires.
    let mut request_metadata = RequestMetadata::new_lakekeeper_internal(Uuid::now_v7());
    request_metadata.set_authentication(Actor::Principal(user_id), authentication);
    Ok(request_metadata)
}

/// Bootstrap lakekeeper's default project using a stable admin principal.
///
/// Idempotent: re-running against an already-bootstrapped catalog is a no-op
/// success (lakekeeper returns `CatalogAlreadyBootstrapped` / `ConcurrentBootstrap`,
/// which we tolerate). After the first successful call the catalog's default
/// project (nil-UUID, "Default Project") exists and the configured
/// `system_user` is registered as its global-admin user.
///
/// `state` must be built against an already-migrated `lakekeeper` schema
/// (run [`crate::run_lakekeeper_migrations`] first).
///
/// # Errors
/// Returns an error if the system-user id cannot be derived, or if `bootstrap`
/// fails for any reason other than already-bootstrapped.
pub async fn ensure_bootstrapped(
    state: ApiContext<EngineState>,
    system_user: &str,
) -> anyhow::Result<()> {
    let request_metadata =
        system_request_metadata(system_user).context("building system bootstrap principal")?;

    let bootstrap_req = BootstrapRequest::builder()
        .accept_terms_of_use()
        .user_name("Egret System".to_string())
        .user_type(UserType::Application)
        .build();

    match <EngineApiServer as ServerService<_, _, _>>::bootstrap(
        state,
        request_metadata,
        bootstrap_req,
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(e) if is_already_bootstrapped(&e) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("lakekeeper bootstrap failed: {e:?}")),
    }
}

/// Whether an error from `bootstrap` indicates the catalog was already
/// bootstrapped (treated as idempotent success).
fn is_already_bootstrapped(e: &lakekeeper::api::IcebergErrorResponse) -> bool {
    let t = &e.error.r#type;
    t == "CatalogAlreadyBootstrapped" || t == "ConcurrentBootstrap"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_user_id_is_stable_and_valid() {
        let a = system_user_id("egret-system").unwrap();
        let b = system_user_id("egret-system").unwrap();
        assert_eq!(a, b, "same system_user must yield the same UserId");
        assert_eq!(a.idp_id(), Some(SYSTEM_USER_IDP));

        // Different names yield different principals.
        let c = system_user_id("other-system").unwrap();
        assert_ne!(a, c);
    }

    #[test]
    fn system_user_id_tolerates_odd_input() {
        // Spaces / arbitrary chars in config still produce a valid UserId because
        // the subject is a v5 UUID, not the raw string.
        let id = system_user_id("Egret System Admin!").unwrap();
        assert_eq!(id.idp_id(), Some(SYSTEM_USER_IDP));
    }
}
