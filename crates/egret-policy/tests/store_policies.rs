//! DB-gated PolicyStore policy tests. Self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAIL.
use egret_metastore::Db;
use egret_policy::PolicyStore;
use serde_json::json;

async fn store() -> Option<PolicyStore> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(PolicyStore::new(db))
}
fn uniq(p: &str) -> String {
    format!("{p}_{}", uuid::Uuid::new_v4().simple())
}

fn def_id_from_name(name: &str) -> i64 {
    // derive a stable i64 from a unique name so each test run uses a fresh service_def id
    let mut h: i64 = 0xcbf29ce484222325_u64 as i64;
    for b in name.bytes() {
        h ^= b as i64;
        h = h.wrapping_mul(0x100000001b3_u64 as i64);
    }
    h.abs()
}

#[tokio::test]
async fn policy_crud_bumps_version() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let defname = uniq("trino");
    let def_id = def_id_from_name(&defname);
    s.upsert_service_def(def_id, &defname, &json!({"name": defname}))
        .await
        .unwrap();
    let svc = s
        .create_service(&uniq("svc"), &defname, &json!({}))
        .await
        .unwrap();
    assert_eq!(svc.policy_version, 1);

    let pol = json!({
        "name": "p1",
        "service": svc.name.clone(),
        "policyType": 0,
        "resources": {
            "catalog": {
                "values": ["hive"],
                "isExcludes": false,
                "isRecursive": false
            }
        },
        "policyItems": [
            {
                "accesses": [{"type": "select", "isAllowed": true}],
                "users": ["alice"]
            }
        ]
    });
    let pid = s.create_policy(svc.id, "p1", &pol).await.unwrap();
    assert_eq!(
        s.get_service(&svc.name).await.unwrap().policy_version,
        2,
        "policy_version should be 2 after create"
    );

    s.update_policy(pid, "p1", &pol, true).await.unwrap();
    assert_eq!(
        s.get_service(&svc.name).await.unwrap().policy_version,
        3,
        "policy_version should be 3 after update"
    );

    assert_eq!(
        s.enabled_policies_for_service(svc.id).await.unwrap().len(),
        1
    );
    s.delete_policy(pid).await.unwrap();
    assert_eq!(
        s.get_service(&svc.name).await.unwrap().policy_version,
        4,
        "policy_version should be 4 after delete"
    );
    assert_eq!(
        s.enabled_policies_for_service(svc.id).await.unwrap().len(),
        0
    );

    // update/delete unknown policy → NotFound
    assert!(matches!(
        s.update_policy(999_999_999, "x", &pol, true)
            .await
            .unwrap_err(),
        egret_policy::PolicyStoreError::NotFound(_)
    ));
    assert!(matches!(
        s.delete_policy(999_999_999).await.unwrap_err(),
        egret_policy::PolicyStoreError::NotFound(_)
    ));
}

#[tokio::test]
async fn list_policies_returns_all_for_service() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let defname = uniq("trino_lp");
    let def_id = def_id_from_name(&defname);
    s.upsert_service_def(def_id, &defname, &json!({"name": defname}))
        .await
        .unwrap();
    let svc = s
        .create_service(&uniq("svc"), &defname, &json!({}))
        .await
        .unwrap();

    let pol = json!({"name": "p1", "service": svc.name.clone()});
    let pid1 = s.create_policy(svc.id, "p1", &pol).await.unwrap();
    s.create_policy(svc.id, "p2", &pol).await.unwrap();
    s.update_policy(pid1, "p1", &pol, false).await.unwrap();

    let rows = s.list_policies(svc.id).await.unwrap();
    assert_eq!(
        rows.len(),
        2,
        "list_policies should include the disabled policy"
    );
    assert_eq!(
        s.enabled_policies_for_service(svc.id).await.unwrap().len(),
        1,
        "enabled_policies_for_service should exclude the disabled policy"
    );

    s.delete_service(svc.id).await.unwrap();
}

/// Whole-branch review, Important 6: `upsert_policy` was the one write path
/// with no guard at all. It keeps its exemption from `refuse_if_generated` —
/// its only caller is the admin-defaults seed, and `engines.rs` turns a seed
/// failure into a 400, so a profile-wide refusal there would break every
/// engine update against a claimed profile — but the reserved-NAME guard is
/// pure, name-only, and closes the drift: a second caller passing a
/// `perm-grant-`/`perm-rule-` name would overwrite compiled output, which the
/// next recompile silently restores.
#[tokio::test]
async fn upsert_policy_refuses_the_compilers_reserved_names_and_allows_every_other() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let defname = uniq("trino_upsert");
    let def_id = def_id_from_name(&defname);
    s.upsert_service_def(def_id, &defname, &json!({"name": defname}))
        .await
        .unwrap();
    let svc = s
        .create_service(&uniq("svc"), &defname, &json!({}))
        .await
        .unwrap();
    let pol = json!({"name": "x", "service": svc.name.clone()});

    for reserved in ["perm-grant-0001", "perm-rule-0001"] {
        let err = s
            .upsert_policy(svc.id, reserved, &pol)
            .await
            .expect_err("a compiler-owned name must be refused here as it is everywhere else");
        assert!(
            matches!(err, egret_policy::PolicyStoreError::CompilerOwned(_)),
            "{reserved}: expected CompilerOwned, got {err:?}"
        );
        assert!(
            s.list_policies(svc.id)
                .await
                .unwrap()
                .iter()
                .all(|p| p.name != reserved),
            "{reserved}: the refusal must write nothing"
        );
    }

    // The seed's own names — the caller whose exemption is load-bearing —
    // must still go through, twice (upsert, not insert).
    let id1 = s
        .upsert_policy(svc.id, "egret-admin-all", &pol)
        .await
        .expect("the admin seed's names collide with no reserved prefix");
    let id2 = s
        .upsert_policy(svc.id, "egret-admin-all", &pol)
        .await
        .expect("upsert must still update in place");
    assert_eq!(id1, id2);

    s.delete_service(svc.id).await.unwrap();
}
