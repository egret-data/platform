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
use egret_machine_idp::{ClientVerifier, IdpState, SigningKeyStore};
use std::sync::Arc;

/// egret's identity and its in-cluster address, derived together from config so
/// neither can drift from the other.
#[derive(Debug, Clone)]
pub struct EgretIdentity {
    issuer: String,
    in_cluster_base_url: String,
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
        Self {
            issuer,
            in_cluster_base_url,
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
        }
    }
}
