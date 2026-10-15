//! Who egret *is*, and where engines *dial it* — resolved once, in one place a
//! test can drive.
//!
//! These are two questions with two answers, and they used to share one value:
//!
//! * **Identity** is the `iss` of every token egret mints, the `issuer` of its
//!   OIDC discovery document, and the `required_issuer` a StarRocks security
//!   integration byte-compares against. OIDC Discovery requires all of those to
//!   be the same string AND to equal the URL the document was fetched from, so
//!   identity has to be a name a client outside the cluster can resolve.
//! * **Address** is what an engine pod resolves to fetch keys and the catalog —
//!   a Service DNS name, deliberately. Sending those fetches out through the
//!   ingress and back would make every query depend on the ingress being
//!   healthy, to obtain keys that are byte-identical either way.
//!
//! One value cannot be both once the deployment has an ingress, which is why
//! `idp.issuer` exists. This type keeps the two apart by NAME: nothing here
//! hands out a bare `String` that could be either, and the accessors say which
//! question they answer. It is the same address-vs-claim split `DataApiConfig`
//! made for the Data API's consumer URLs (#525).

use egret_config::Config;
use egret_core::ratelimit::RateLimiter;
use egret_machine_idp::{ClientVerifier, IdpState, SigningKeyStore, VerifiedCache};
use std::sync::Arc;

/// egret's identity and its in-cluster address, derived together from config so
/// neither can drift from the other.
#[derive(Debug, Clone)]
pub struct EgretIdentity {
    issuer: String,
    in_cluster_base_url: String,
    consumer_base_url: String,
}

impl EgretIdentity {
    /// Derive both from config. `catalog.public_url` is the in-cluster address
    /// (and the historical do-everything value); `idp.issuer` is the identity,
    /// falling back to that same address when unset — so a deployment that sets
    /// nothing keeps exactly today's `iss`.
    ///
    /// Trailing slashes are trimmed on BOTH, at the source. `main.rs` used to
    /// carry the reason and it belongs with the derivation: StarRocks compares
    /// `required_issuer` raw, so a trailing slash silently breaks StarRocks JWT
    /// auth while Trino — which never reads the issuer — keeps working.
    pub fn from_config(config: &Config) -> Self {
        let in_cluster_base_url = config
            .catalog
            .public_url
            .clone()
            .unwrap_or_else(|| "http://localhost:8080".to_string())
            .trim_end_matches('/')
            .to_string();
        let issuer = config.idp.issuer_url(&in_cluster_base_url);
        // The Data API's consumer origin falls back to the ISSUER, not to the
        // in-cluster address. Both are "somewhere outside the cluster reaches
        // us", and the document a consumer receives puts them side by side: its
        // `iss`/`tokenUrl` come from the issuer and its `servers[0].url` and
        // `token_endpoint` come from here. Falling back to the Service DNS name
        // would let a deployment that sets `EGRET_IDP__ISSUER` and nothing else
        // hand out an externally-valid `iss` next to a `token_endpoint` on a
        // host the consumer cannot resolve — a document that is internally
        // contradictory and simply cannot be used. `data_api.public_url` still
        // wins when set, for the deployment that really does serve its Data API
        // on a third origin.
        let consumer_base_url = config.data_api.consumer_base_url(&issuer);
        Self {
            issuer,
            in_cluster_base_url,
            consumer_base_url,
        }
    }

    /// The `iss` claim and the OIDC `issuer`. Every token this deployment signs
    /// carries this, and every check that compares against an `iss` uses it —
    /// they move together or a token minted under one is rejected by a check
    /// using another.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// The address an ENGINE POD resolves: the catalog URI baked into engine
    /// ConfigMaps, and the base of [`Self::engine_jwks_uri`]. Not an identity —
    /// do not put this in a token or a discovery document.
    pub fn in_cluster_base_url(&self) -> &str {
        &self.in_cluster_base_url
    }

    /// The origin a Data API CONSUMER outside the cluster dials: the generated
    /// OpenAPI document's `servers[0].url`, the `token_endpoint` handed back
    /// with a new client, and the URL and `curl` line Try-it shows the author.
    ///
    /// An ADDRESS, like [`Self::in_cluster_base_url`] — but an external one, so
    /// its default is [`Self::issuer`] rather than the Service DNS name. It is a
    /// separate key because the two audiences (a consumer, and this deployment's
    /// own identity) are allowed to differ; it is not a separate DEFAULT,
    /// because when only one of them is configured the other has no business
    /// pointing somewhere unreachable.
    pub fn consumer_base_url(&self) -> &str {
        &self.consumer_base_url
    }

    /// The JWKS URL an engine is configured to fetch keys from — an ADDRESS, so
    /// it stays in-cluster even when the issuer is an ingress hostname. The
    /// discovery document's own `jwks_uri` deliberately differs: that one is
    /// dialled by an external client, so it follows the issuer.
    pub fn engine_jwks_uri(&self) -> String {
        format!("{}/.well-known/jwks.json", self.in_cluster_base_url)
    }

    /// Build the machine-IdP's state.
    ///
    /// A constructor rather than a struct literal at the composition root, so
    /// that "the IdP's identity is [`Self::issuer`]" is a line a test executes.
    /// Written inline in `main.rs` it was a line no test could reach, and it is
    /// exactly the line whose being wrong produces the defect this whole change
    /// is about: a discovery document that answers with an address nobody
    /// outside the cluster can resolve.
    pub fn idp_state(
        &self,
        keys: Arc<dyn SigningKeyStore>,
        verifiers: Vec<Arc<dyn ClientVerifier>>,
        rate_limiter: RateLimiter,
    ) -> IdpState {
        IdpState {
            keys,
            issuer: self.issuer.clone(),
            verifiers,
            rate_limiter,
            verified_cache: VerifiedCache::default(),
        }
    }
}
