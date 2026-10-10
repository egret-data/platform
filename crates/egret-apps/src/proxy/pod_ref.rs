//! The URL segment and pod-name stem for an author's edit pod and a viewer's run pod.
//!
//! Defined once, here, so the redirect after a ticket, the cookie's `Path`, the check on every
//! proxied request and the launcher's pod name cannot drift apart — both the proxy (`relay.rs`,
//! `launcher.rs`) and the Kubernetes launcher (`egret-bin::apps_pod_k8s`) call these, never
//! reimplement the hash.
//!
//! Each function mixes in its own one-byte domain tag before hashing, so an edit ref and a run
//! ref can never collide even if a workspace id and a version id happened to share bytes (D24:
//! the two kinds of pod now sit side by side under one proxy, where a collision would let a
//! request meant for one be accepted as the other).

use egret_core::{UserId, WorkspaceId};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const EDIT_TAG: u8 = b'E';
const RUN_TAG: u8 = b'R';

fn tagged_ref(tag: u8, user: Uuid, scope: Uuid) -> String {
    let mut h = Sha256::new();
    h.update([tag]);
    h.update(user.as_bytes());
    h.update(scope.as_bytes());
    hex::encode(&h.finalize()[..8])
}

/// Stable per (author, workspace): an edit pod is keyed by who is writing, in which workspace.
pub fn edit_pod_ref(user: UserId, ws: WorkspaceId) -> String {
    tagged_ref(EDIT_TAG, user.0, ws.0)
}

/// Stable per (viewer, version): D24 — a run pod serves exactly one viewer's own copy of one
/// published version, never shared with another viewer.
pub fn run_pod_ref(user: UserId, version: Uuid) -> String {
    tagged_ref(RUN_TAG, user.0, version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex16(r: &str) {
        assert_eq!(r.len(), 16);
        assert!(
            r.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "{r}"
        );
    }

    #[test]
    fn edit_ref_is_stable_distinct_per_pair_and_path_safe() {
        let (u1, u2) = (UserId(Uuid::new_v4()), UserId(Uuid::new_v4()));
        let (w1, w2) = (WorkspaceId(Uuid::new_v4()), WorkspaceId(Uuid::new_v4()));
        assert_eq!(edit_pod_ref(u1, w1), edit_pod_ref(u1, w1));
        assert_ne!(edit_pod_ref(u1, w1), edit_pod_ref(u2, w1));
        assert_ne!(edit_pod_ref(u1, w1), edit_pod_ref(u1, w2));
        hex16(&edit_pod_ref(u1, w1));
    }

    #[test]
    fn run_ref_is_stable_distinct_per_pair_and_path_safe() {
        let (u1, u2) = (UserId(Uuid::new_v4()), UserId(Uuid::new_v4()));
        let (v1, v2) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(run_pod_ref(u1, v1), run_pod_ref(u1, v1));
        assert_ne!(run_pod_ref(u1, v1), run_pod_ref(u2, v1));
        assert_ne!(run_pod_ref(u1, v1), run_pod_ref(u1, v2));
        hex16(&run_pod_ref(u1, v1));
    }

    #[test]
    fn run_and_edit_refs_never_collide_for_the_same_raw_uuid_pair() {
        let u = UserId(Uuid::new_v4());
        let shared = Uuid::new_v4(); // used as BOTH a workspace id and a version id
        assert_ne!(edit_pod_ref(u, WorkspaceId(shared)), run_pod_ref(u, shared));
    }
}
