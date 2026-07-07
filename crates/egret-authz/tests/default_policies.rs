//! DB-gated tests for `seed_default_trino_policies`. Self-skip without
//! DATABASE_URL — do NOT point this at the live demo Postgres on :5432.
use egret_authz::AuthzStore;
use egret_metastore::Db;
use serde_json::json;

async fn store() -> Option<AuthzStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(AuthzStore::new(db))
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
async fn seed_default_trino_policies_is_idempotent_and_faithful() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let defname = uniq("trino_seed");
    let def_id = def_id_from_name(&defname);
    s.upsert_service_def(def_id, &defname, &json!({"name": defname}))
        .await
        .unwrap();
    let svc = s
        .create_service(&uniq("svc"), &defname, &json!({}))
        .await
        .unwrap();

    let created = s
        .seed_default_trino_policies(svc.id, &svc.name, "admin")
        .await
        .unwrap();
    assert!(created > 0, "should create at least one default policy");
    assert_eq!(
        created, 13,
        "expected one default policy per Trino resource hierarchy"
    );

    // Idempotent: calling again on a service that already has policies is a no-op.
    let again = s
        .seed_default_trino_policies(svc.id, &svc.name, "admin")
        .await
        .unwrap();
    assert_eq!(
        again, 0,
        "re-seeding an already-seeded service must be a no-op"
    );

    let rows = s.list_policies(svc.id).await.unwrap();
    assert_eq!(rows.len(), created as usize);

    let trinouser_policy = rows
        .iter()
        .find(|r| r.name == "all - trinouser")
        .expect("expected a default policy named 'all - trinouser'");
    let accesses = trinouser_policy.policy_jsonb["policyItems"][0]["accesses"]
        .as_array()
        .unwrap();
    assert!(
        accesses
            .iter()
            .any(|a| a["type"] == "impersonate" && a["isAllowed"] == true),
        "trinouser default policy should grant impersonate"
    );
    assert_eq!(
        trinouser_policy.policy_jsonb["policyItems"][0]["groups"][0],
        "admin"
    );

    s.delete_service(svc.id).await.unwrap();
}
