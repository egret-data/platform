//! A resource profile's download embeds its tag service's policies as
//! `tagPolicies`, and is gated by the PROFILE's `policy_version` alone
//! (`ranger/api/download.rs`: `lastKnownVersion == svc.policy_version` → 304).
//!
//! So a write to a tag service's policies must bump every profile that links
//! that tag service through `config_jsonb.tagService` — otherwise every engine
//! downloading one of those profiles is told "not modified" and keeps enforcing
//! the old tag masks and row filters until something unrelated bumps it.
//!
//! Both cells: the linked profiles move, and a profile that links nothing (and
//! the reverse direction, a write to a linked profile) moves nothing else.
use egret_metastore::Db;
use egret_policy::{seed_tag, seed_trino, PolicyStore};
use serde_json::json;

fn uniq(p: &str) -> String {
    format!("{p}_{}", uuid::Uuid::new_v4().simple())
}

struct Fixture {
    store: PolicyStore,
    db: Db,
    tag: i64,
    linked_a: i64,
    linked_b: i64,
    unlinked: i64,
}

async fn fixture() -> Option<Fixture> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = PolicyStore::new(db.clone());
    seed_tag(&store).await.unwrap();
    seed_trino(&store).await.unwrap();

    let tag_name = uniq("bump_tag");
    let tag = store
        .create_service(&tag_name, "tag", &json!({}))
        .await
        .unwrap()
        .id;
    let linked_a = store
        .create_service(&uniq("bump_a"), "trino", &json!({ "tagService": tag_name }))
        .await
        .unwrap()
        .id;
    let linked_b = store
        .create_service(&uniq("bump_b"), "trino", &json!({ "tagService": tag_name }))
        .await
        .unwrap()
        .id;
    let unlinked = store
        .create_service(&uniq("bump_z"), "trino", &json!({}))
        .await
        .unwrap()
        .id;
    Some(Fixture {
        store,
        db,
        tag,
        linked_a,
        linked_b,
        unlinked,
    })
}

impl Fixture {
    async fn versions(&self) -> [i64; 4] {
        let mut out = [0; 4];
        for (i, id) in [self.tag, self.linked_a, self.linked_b, self.unlinked]
            .iter()
            .enumerate()
        {
            out[i] = sqlx::query_scalar(
                "SELECT policy_version FROM policy.authz_services WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&self.db.pool)
            .await
            .unwrap();
        }
        out
    }

    /// Asserts which of [tag, linked_a, linked_b, unlinked] moved since `before`.
    async fn assert_moved(&self, before: [i64; 4], expect: [bool; 4], step: &str) {
        let after = self.versions().await;
        let moved: Vec<bool> = before
            .iter()
            .zip(after.iter())
            .map(|(b, a)| a > b)
            .collect();
        assert_eq!(
            moved,
            expect.to_vec(),
            "{step}: which of [tag, linked_a, linked_b, unlinked] bumped (before {before:?}, after {after:?})"
        );
    }
}

fn tag_mask(tag: &str) -> serde_json::Value {
    json!({
        "policyType": 1,
        "resources": { "tag": { "values": [tag], "isExcludes": false, "isRecursive": false } },
        "dataMaskPolicyItems": [{
            "users": [], "groups": ["public"], "roles": [],
            "accesses": [{ "type": "select", "isAllowed": true }],
            "dataMaskInfo": { "dataMaskType": "MASK_NULL" }
        }]
    })
}

#[tokio::test]
async fn every_write_to_a_tag_services_policies_bumps_every_profile_that_links_it() {
    let Some(f) = fixture().await else {
        return;
    };
    let linked_only = [true, true, true, false];

    let v = f.versions().await;
    let id = f
        .store
        .create_policy(f.tag, &uniq("mask"), &tag_mask("pii"))
        .await
        .unwrap();
    f.assert_moved(v, linked_only, "create a tag policy").await;

    let v = f.versions().await;
    let name = uniq("mask_renamed");
    f.store
        .update_policy(id, &name, &tag_mask("phi"), true)
        .await
        .unwrap();
    f.assert_moved(v, linked_only, "update a tag policy").await;

    let v = f.versions().await;
    f.store.set_policy_enabled(id, false).await.unwrap();
    f.assert_moved(v, linked_only, "disable a tag policy").await;

    let v = f.versions().await;
    f.store.delete_policy(id).await.unwrap();
    f.assert_moved(v, linked_only, "delete a tag policy").await;
}

/// The other direction: a write to a linked resource profile changes nothing in
/// the tag service, so neither the tag service nor its other linkers may move.
/// Without this cell a fix that bumped "everything" would pass the test above.
#[tokio::test]
async fn a_write_to_a_linked_profile_bumps_only_that_profile() {
    let Some(f) = fixture().await else {
        return;
    };
    let body = json!({
        "resources": {
            "catalog": { "values": ["wh"], "isExcludes": false, "isRecursive": false },
            "schema":  { "values": ["s"],  "isExcludes": false, "isRecursive": false },
            "table":   { "values": ["t"],  "isExcludes": false, "isRecursive": false },
            "column":  { "values": ["*"],  "isExcludes": false, "isRecursive": false }
        },
        "policyItems": [{
            "users": ["alice"], "groups": [], "roles": [],
            "accesses": [{ "type": "select", "isAllowed": true }]
        }]
    });
    let v = f.versions().await;
    f.store
        .create_policy(f.linked_a, &uniq("alice_reads"), &body)
        .await
        .unwrap();
    f.assert_moved(v, [false, true, false, false], "create on a linked profile")
        .await;
}
