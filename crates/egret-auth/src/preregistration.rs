//! SSO pre-registration (KI-185): a workspace admin adds an SSO user by email
//! before that user has ever signed in, and the membership binds at the first
//! sign-in.
//!
//! The pending memberships live in `workspace_invites`, owned by
//! `egret-workspace` (a membership write recompiles engine policies there, and
//! this crate cannot depend on it), so the OIDC callback reaches them through
//! the [`PendingMembershipRedeemer`] port that `egret-bin` wires.
//!
//! # What binds, and to whom
//!
//! Only an email the IdP **verified** may claim a pre-registration
//! ([`redeem_verified`]). An unverified `email` claim is whatever the person
//! typed into the IdP's profile form; honouring it would let anyone who can
//! register at the IdP claim a membership by naming someone else's address.
//! A pre-registration can only ever carry a WORKSPACE role: the table pins
//! `role_scope = 'workspace'` in its foreign key to `roles`, so it never grants
//! a platform role.

use egret_core::{Result, UserId};

use crate::oidc::OidcClaims;

/// One pre-registration turned into a membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedeemedInvite {
    pub workspace_id: uuid::Uuid,
    pub role: String,
    /// The admin who added the email; recorded as the grant's `granted_by`.
    pub invited_by: uuid::Uuid,
}

#[async_trait::async_trait]
pub trait PendingMembershipRedeemer: Send + Sync {
    /// Turn every pre-registration for `email` into a membership of `user`,
    /// and remove it. A workspace `user` already belongs to keeps its current
    /// role; the pre-registration is removed all the same. Callers go through
    /// [`redeem_verified`], which holds the verified-email rule.
    async fn redeem(&self, user: UserId, email: &str) -> Result<Vec<RedeemedInvite>>;
}

/// Redeem the pre-registrations for the signed-in user's email, if and only if
/// the IdP verified that email.
pub async fn redeem_verified(
    redeemer: &dyn PendingMembershipRedeemer,
    user: UserId,
    claims: &OidcClaims,
) -> Result<Vec<RedeemedInvite>> {
    if !claims.email_verified {
        return Ok(Vec::new());
    }
    redeemer.redeem(user, &claims.email).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    #[async_trait::async_trait]
    impl PendingMembershipRedeemer for Recorder {
        async fn redeem(&self, _user: UserId, email: &str) -> Result<Vec<RedeemedInvite>> {
            self.0.lock().unwrap().push(email.to_string());
            Ok(Vec::new())
        }
    }

    fn claims(verified: bool) -> OidcClaims {
        OidcClaims {
            subject: "sub".into(),
            email: "new@example.com".into(),
            email_verified: verified,
            name: "New".into(),
            groups: vec![],
        }
    }

    #[tokio::test]
    async fn an_unverified_email_claims_nothing() {
        let r = Recorder::default();
        redeem_verified(&r, UserId(uuid::Uuid::new_v4()), &claims(false))
            .await
            .unwrap();
        assert!(r.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_verified_email_is_redeemed() {
        let r = Recorder::default();
        redeem_verified(&r, UserId(uuid::Uuid::new_v4()), &claims(true))
            .await
            .unwrap();
        assert_eq!(*r.0.lock().unwrap(), vec!["new@example.com".to_string()]);
    }
}
