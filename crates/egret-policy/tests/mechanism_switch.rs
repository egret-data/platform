//! Governance SP2b Task 11: the conditions under which a profile may switch
//! from Ranger to OPA, each refused by name (`egret_policy::opa_switch_check`),
//! and the C8 guard on `create_policy` for the tag service of an OPA profile.
//!
//! DB-gated; FAILS without `DATABASE_URL` unless `EGRET_ALLOW_DB_SKIP` is set:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-policy --test mechanism_switch

use egret_metastore::Db;
use egret_policy::opa_guard::StoredStateRefusal;
use egret_policy::{
    opa_switch_check, EngineBinding, PolicyHttpError, PolicyStore, PolicyStoreError,
    ProfileRecompiler, ServiceRow, SwitchCheck, SwitchRefusal,
};
use serde_json::json;
use uuid::Uuid;

/// The editor's recompile seam, stubbed: `stored_overlaps_in_tx` answers
/// what the test gives it (the real one is `egret_perm_compile::PermRecompiler`,
/// exercised in `egret-perm-compile`'s `opa_cache` tests).
struct Overlaps(Vec<String>);

#[async_trait::async_trait]
impl ProfileRecompiler for Overlaps {
    async fn recompile_in_tx(
        &self,
        _tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        _service_ids: &[i64],
    ) -> Result<(), PolicyHttpError> {
        Ok(())
    }
    async fn stored_overlaps_in_tx(
        &self,
        _tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        _service_id: i64,
    ) -> Result<Vec<String>, PolicyHttpError> {
        Ok(self.0.clone())
    }
}

fn none() -> Overlaps {
    Overlaps(Vec::new())
}

fn uniq(p: &str) -> String {
    format!("{p}_{}", Uuid::new_v4().simple())
}

async fn store() -> Option<(Db, PolicyStore)> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&store).await.expect("trino def");
    egret_policy::seed_tag(&store).await.expect("tag def");
    Some((db, store))
}

/// A Trino profile with its own tag service; `generated` when asked.
async fn trino_profile(db: &Db, store: &PolicyStore, generated: bool) -> (ServiceRow, ServiceRow) {
    let tag = store
        .create_service(&uniq("ms_tag"), "tag", &json!({}))
        .await
        .expect("tag service");
    let profile = store
        .create_service(&uniq("ms_trino"), "trino", &json!({"tagService": tag.name}))
        .await
        .expect("trino profile");
    if generated {
        set_generated(db, profile.id).await;
    }
    (store.get_service(&profile.name).await.unwrap(), tag)
}

async fn set_generated(db: &Db, id: i64) {
    sqlx::query("UPDATE policy.authz_services SET origin = 'generated' WHERE id = $1")
        .bind(id)
        .execute(&db.pool)
        .await
        .expect("generated");
}

fn binding(svc: i64, name: &str, origin: &str, image: Option<&str>) -> EngineBinding {
    EngineBinding {
        id: Uuid::new_v4(),
        name: name.to_string(),
        status: "running".into(),
        origin: origin.into(),
        r#type: "trino".into(),
        authz_service_id: Some(svc),
        warehouse_ids: Vec::new(),
        image: image.map(str::to_string),
    }
}

async fn check(
    db: &Db,
    enabled: bool,
    svc: &ServiceRow,
    bindings: &[EngineBinding],
    recompiler: &dyn ProfileRecompiler,
) -> SwitchCheck {
    let mut tx = db.pool.begin().await.expect("tx");
    let out = opa_switch_check(&mut tx, enabled, svc, bindings, recompiler)
        .await
        .expect("the check reads");
    tx.rollback().await.expect("rollback");
    out
}

fn messages(c: &SwitchCheck) -> Vec<String> {
    c.refusals.iter().map(SwitchRefusal::message).collect()
}

#[tokio::test]
async fn the_gate_off_is_refused_first() {
    // Arrange
    let Some((db, store)) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (svc, _) = trino_profile(&db, &store, true).await;

    // Act
    let off = check(&db, false, &svc, &[], &none()).await;
    let gated_and_manual = {
        let manual = ServiceRow {
            origin: "manual".into(),
            ..svc.clone()
        };
        check(&db, false, &manual, &[], &none()).await
    };

    // Assert
    assert_eq!(off.refusals, vec![SwitchRefusal::Disabled]);
    assert_eq!(
        messages(&off),
        vec!["OPA is not enabled on this installation ([governance.opa] enabled)"]
    );
    assert_eq!(
        gated_and_manual.refusals,
        vec![SwitchRefusal::Disabled, SwitchRefusal::NotGenerated],
        "every condition, the gate first"
    );
}

#[tokio::test]
async fn a_starrocks_profile_is_refused_as_not_trino() {
    // Arrange
    let Some((db, store)) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    egret_policy::seed_starrocks(&store)
        .await
        .expect("starrocks def");
    let created = store
        .create_service(&uniq("ms_sr"), "starrocks", &json!({}))
        .await
        .expect("starrocks profile");
    set_generated(&db, created.id).await;
    let svc = store.get_service(&created.name).await.unwrap();

    // Act
    let out = check(&db, true, &svc, &[], &none()).await;

    // Assert
    assert_eq!(
        out.refusals,
        vec![SwitchRefusal::NotTrino {
            r#type: "starrocks".into()
        }]
    );
    assert_eq!(
        messages(&out),
        vec!["only a Trino profile can use OPA; this profile is of type starrocks"]
    );
}

#[tokio::test]
async fn a_manual_profile_is_refused_as_not_generated() {
    // Arrange
    let Some((db, store)) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (svc, _) = trino_profile(&db, &store, false).await;

    // Act
    let out = check(&db, true, &svc, &[], &none()).await;

    // Assert
    assert_eq!(out.refusals, vec![SwitchRefusal::NotGenerated]);
    assert_eq!(
        messages(&out),
        vec![
            "only a generated profile can use OPA: hand-written policies are Ranger JSON that \
             OPA cannot evaluate"
        ]
    );
}

#[tokio::test]
async fn a_provisioned_engine_on_an_image_without_the_opa_plugin_is_refused_by_name() {
    // Arrange
    let Some((db, store)) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (svc, _) = trino_profile(&db, &store, true).await;
    let bindings = [
        binding(
            svc.id,
            "old-trino",
            "provisioned",
            Some("trinodb/trino:470"),
        ),
        binding(svc.id, "bare-trino", "provisioned", None),
        binding(
            svc.id,
            "good-trino",
            "provisioned",
            Some("trinodb/trino:483"),
        ),
        // Bound to another profile: not this switch's concern.
        binding(svc.id + 1_000_000, "elsewhere", "provisioned", None),
    ];

    // Act
    let out = check(&db, true, &svc, &bindings, &none()).await;

    // Assert
    assert_eq!(
        out.refusals,
        vec![
            SwitchRefusal::EngineImage {
                engine: "old-trino".into(),
                image: Some("trinodb/trino:470".into()),
            },
            SwitchRefusal::EngineImage {
                engine: "bare-trino".into(),
                image: None,
            },
        ]
    );
    assert_eq!(
        messages(&out),
        vec![
            "engine old-trino runs trinodb/trino:470, which does not ship the Trino OPA plugin; \
             supported: trinodb/trino:483",
            "engine bare-trino runs an unknown image, which does not ship the Trino OPA plugin; \
             supported: trinodb/trino:483",
        ]
    );
    assert!(out.external_engines_unchanged.is_empty());
}

#[tokio::test]
async fn an_external_engine_is_listed_not_refused() {
    // Arrange
    let Some((db, store)) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (svc, _) = trino_profile(&db, &store, true).await;
    let bindings = [binding(svc.id, "their-trino", "external", None)];

    // Act
    let out = check(&db, true, &svc, &bindings, &none()).await;

    // Assert
    assert!(out.refusals.is_empty(), "{:?}", out.refusals);
    assert_eq!(out.external_engines_unchanged, vec!["their-trino"]);
}

#[tokio::test]
async fn all_defaults_are_refused_with_their_count() {
    // Arrange
    let Some((db, store)) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (manual, _) = trino_profile(&db, &store, false).await;
    let seeded = store
        .seed_default_trino_policies(manual.id, &manual.name, "admin")
        .await
        .expect("defaults");
    assert_eq!(seeded, 13);
    set_generated(&db, manual.id).await;
    let svc = store.get_service(&manual.name).await.unwrap();

    // Act
    let out = check(&db, true, &svc, &[], &none()).await;

    // Assert
    assert_eq!(
        out.refusals,
        vec![SwitchRefusal::Stored(StoredStateRefusal::AllDefaults {
            count: 13
        })]
    );
    assert_eq!(
        messages(&out),
        vec!["the profile holds the `all - …` default policies (13); delete them first"]
    );
}

#[tokio::test]
async fn a_hand_written_row_in_the_tag_service_is_refused() {
    // Arrange
    let Some((db, store)) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (svc, tag) = trino_profile(&db, &store, true).await;
    store
        .create_policy(
            tag.id,
            "ms hand",
            &json!({"name": "ms hand", "policyType": 0, "isEnabled": true,
                    "resources": {"tag": {"values": ["pii"]}},
                    "policyItems": [], "denyPolicyItems": []}),
        )
        .await
        .expect("a hand-written tag policy while the profile is on Ranger");

    // Act
    let out = check(&db, true, &svc, &[], &none()).await;

    // Assert
    assert_eq!(
        out.refusals,
        vec![SwitchRefusal::Stored(StoredStateRefusal::HandWritten {
            service: tag.name.clone(),
            count: 1
        })]
    );
    assert_eq!(
        messages(&out),
        vec![format!(
            "{} holds 1 hand-written policies, which OPA would ignore; delete them first",
            tag.name
        )]
    );
}

#[tokio::test]
async fn a_stored_overlap_is_refused_with_its_pairs() {
    // Arrange
    let Some((db, store)) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (svc, _) = trino_profile(&db, &store, true).await;
    let pair = "mask on lake.s.t.email: perm-rule-a, perm-rule-b".to_string();

    // Act
    let out = check(&db, true, &svc, &[], &Overlaps(vec![pair.clone()])).await;

    // Assert
    assert_eq!(
        out.refusals,
        vec![SwitchRefusal::StoredOverlaps { pairs: vec![pair] }]
    );
    assert_eq!(
        messages(&out),
        vec![
            "the profile serves 1 mask/row-filter pairs whose winner the artifact does not \
             express (see GET /perm/overlaps); resolve them first: mask on lake.s.t.email: \
             perm-rule-a, perm-rule-b"
        ]
    );
}

#[tokio::test]
async fn a_generated_trino_profile_on_483_switches() {
    // Arrange
    let Some((db, store)) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (svc, _) = trino_profile(&db, &store, true).await;
    let pinned =
        "trinodb/trino:483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534";
    let bindings = [binding(svc.id, "trino-483", "provisioned", Some(pinned))];

    // Act
    let out = check(&db, true, &svc, &bindings, &none()).await;

    // Assert
    assert!(out.refusals.is_empty(), "{:?}", messages(&out));
    assert!(out.external_engines_unchanged.is_empty());
}

#[tokio::test]
async fn a_hand_written_policy_in_an_opa_profiles_tag_service_is_refused_at_create() {
    // Arrange
    let Some((db, store)) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (svc, tag) = trino_profile(&db, &store, true).await;
    let body = json!({"name": "ms late", "policyType": 0, "isEnabled": true,
                      "resources": {"tag": {"values": ["pii"]}},
                      "policyItems": [], "denyPolicyItems": []});
    sqlx::query("UPDATE policy.authz_services SET mechanism = 'opa' WHERE id = $1")
        .bind(svc.id)
        .execute(&db.pool)
        .await
        .expect("opa");

    // Act
    let err = store
        .create_policy(tag.id, "ms late", &body)
        .await
        .expect_err("refused");

    // Assert
    match err {
        PolicyStoreError::CompilerOwned(msg) => assert_eq!(
            msg,
            format!(
                "{} is the tag service of a profile on OPA; OPA would ignore a hand-written \
                 policy here",
                tag.name
            )
        ),
        other => panic!("expected CompilerOwned, got {other:?}"),
    }
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM policy.authz_policies WHERE service_id = $1")
            .bind(tag.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(stored, 0, "nothing was written");

    // Back on Ranger, the same write is accepted.
    sqlx::query("UPDATE policy.authz_services SET mechanism = 'ranger' WHERE id = $1")
        .bind(svc.id)
        .execute(&db.pool)
        .await
        .expect("ranger");
    store
        .create_policy(tag.id, "ms late", &body)
        .await
        .expect("accepted on Ranger");
}
