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

// ── Task 12: `PATCH /policies/services/{service}` with `{"mechanism": …}` ──

mod route {
    use super::{binding, set_generated, store, trino_profile, uniq};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use egret_auth::groups::GroupRepo;
    use egret_auth::session::SessionService;
    use egret_auth::users::UserRepo;
    use egret_metastore::Db;
    use egret_policy::{
        policy_admin_router, EngineBinding, EngineBindingReader, EngineReadError, PolicyAdminState,
        PolicyHttpError, PolicyStore, ProfileEngineReconfigurer, ProfileRecompiler,
        ReconfigureReport,
    };
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;
    use uuid::Uuid;

    /// The engine seam's binding read, answering a fixed list.
    struct Bindings(Vec<EngineBinding>);

    #[async_trait::async_trait]
    impl EngineBindingReader for Bindings {
        async fn list_engine_bindings(&self) -> Result<Vec<EngineBinding>, EngineReadError> {
            Ok(self.0.clone())
        }
        async fn engine_catalog_names(&self, _: Uuid) -> Result<Vec<String>, EngineReadError> {
            Ok(vec![])
        }
        async fn warehouse_schemas(&self, _: Uuid) -> Result<Vec<String>, EngineReadError> {
            Ok(vec![])
        }
        async fn warehouse_tables(&self, _: Uuid, _: &str) -> Result<Vec<String>, EngineReadError> {
            Ok(vec![])
        }
        async fn warehouse_columns(
            &self,
            _: Uuid,
            _: &str,
            _: &str,
        ) -> Result<Vec<String>, EngineReadError> {
            Ok(vec![])
        }
    }

    /// Records every recompile; answers `overlaps` for the switch check.
    #[derive(Default)]
    struct Recompiles {
        seen: Mutex<Vec<Vec<i64>>>,
        overlaps: Vec<String>,
    }

    #[async_trait::async_trait]
    impl ProfileRecompiler for Recompiles {
        async fn recompile_in_tx(
            &self,
            _tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
            service_ids: &[i64],
        ) -> Result<(), PolicyHttpError> {
            self.seen.lock().unwrap().push(service_ids.to_vec());
            Ok(())
        }
        async fn stored_overlaps_in_tx(
            &self,
            _tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
            _service_id: i64,
        ) -> Result<Vec<String>, PolicyHttpError> {
            Ok(self.overlaps.clone())
        }
    }

    /// Records every reconfigure; answers `report`.
    #[derive(Default)]
    struct Reconfigures {
        seen: Mutex<Vec<i64>>,
        report: ReconfigureReport,
    }

    #[async_trait::async_trait]
    impl ProfileEngineReconfigurer for Reconfigures {
        async fn reconfigure_bound_engines(&self, service_id: i64) -> ReconfigureReport {
            self.seen.lock().unwrap().push(service_id);
            self.report.clone()
        }
    }

    struct AllowAll;
    #[async_trait::async_trait]
    impl egret_core::authz::Pdp for AllowAll {
        async fn authorize(
            &self,
            _s: &egret_core::authz::Subject,
            _a: &str,
            _r: &egret_core::authz::Resource,
        ) -> egret_core::Result<egret_core::authz::Decision> {
            Ok(egret_core::authz::Decision {
                allow: true,
                reasons: vec![],
            })
        }
    }

    struct Harness {
        db: Db,
        store: PolicyStore,
        sessions: SessionService,
        users: UserRepo,
        pdp: Arc<dyn egret_core::authz::Pdp>,
        recompiles: Arc<Recompiles>,
        reconfigures: Arc<Reconfigures>,
        bindings: Vec<EngineBinding>,
        /// Replaces `Bindings(bindings)` as the engine read when set.
        engines: Option<Arc<dyn EngineBindingReader>>,
        opa_enabled: bool,
    }

    async fn harness() -> Option<Harness> {
        let (db, store) = store().await?;
        let sessions = SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        );
        Some(Harness {
            users: UserRepo::new(db.clone()),
            store,
            sessions,
            pdp: Arc::new(AllowAll),
            recompiles: Arc::default(),
            reconfigures: Arc::default(),
            bindings: Vec::new(),
            engines: None,
            opa_enabled: true,
            db,
        })
    }

    impl Harness {
        fn app(&self) -> axum::Router {
            policy_admin_router().with_state(PolicyAdminState {
                store: self.store.clone(),
                engines: self
                    .engines
                    .clone()
                    .unwrap_or_else(|| Arc::new(Bindings(self.bindings.clone()))),
                users: self.users.clone(),
                groups: GroupRepo::new(self.db.clone()),
                sessions: self.sessions.clone(),
                engine_egret_url: "http://egret:8080".into(),
                engine_opa_url: "http://egret:8082".into(),
                pdp: self.pdp.clone(),
                recompiler: self.recompiles.clone(),
                reconfigurer: self.reconfigures.clone(),
                opa_enabled: self.opa_enabled,
            })
        }

        /// A user's session token; `permission` is the one key a fresh
        /// platform role grants, `None` for a platform admin.
        async fn token(&self, permission: Option<&str>) -> String {
            let email = format!("ms_{}@egret.local", Uuid::new_v4().simple());
            let uid = self
                .users
                .insert_local(&email, "Test", "x")
                .await
                .expect("user");
            let role_id: Uuid = match permission {
                None => sqlx::query_scalar(
                    "SELECT id FROM roles WHERE scope = 'platform' AND name = 'platform_admin'",
                )
                .fetch_one(&self.db.pool)
                .await
                .expect("platform_admin"),
                Some(p) => {
                    let id: Uuid = sqlx::query_scalar(
                        "INSERT INTO roles (scope, name, description) \
                         VALUES ('platform', $1, 'test-only role') RETURNING id",
                    )
                    .bind(uniq("ms_role"))
                    .fetch_one(&self.db.pool)
                    .await
                    .expect("role");
                    sqlx::query(
                        "INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)",
                    )
                    .bind(id)
                    .bind(p)
                    .execute(&self.db.pool)
                    .await
                    .expect("permission");
                    id
                }
            };
            sqlx::query(
                "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, \
                 scope_id) VALUES ('user', $1, $2, 'platform', NULL)",
            )
            .bind(uid.0)
            .bind(role_id)
            .execute(&self.db.pool)
            .await
            .expect("grant");
            self.sessions
                .create(uid, "test", "127.0.0.1")
                .await
                .expect("session")
                .token
        }

        async fn patch(&self, service: &str, mechanism: &str) -> (StatusCode, Value) {
            let token = self.token(None).await;
            patch_as(self.app(), &token, service, mechanism).await
        }

        async fn row(&self, id: i64) -> (String, i64) {
            sqlx::query_as(
                "SELECT mechanism, policy_version FROM policy.authz_services WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&self.db.pool)
            .await
            .expect("row")
        }

        async fn set_mechanism(&self, id: i64, mechanism: &str) {
            sqlx::query("UPDATE policy.authz_services SET mechanism = $2 WHERE id = $1")
                .bind(id)
                .bind(mechanism)
                .execute(&self.db.pool)
                .await
                .expect("mechanism");
        }

        async fn events(&self, id: i64) -> Vec<(String, Value)> {
            sqlx::query_as(
                "SELECT action, detail FROM audit.audit_log \
                  WHERE resource_type = 'policy_profile' AND resource_id = $1 ORDER BY id",
            )
            .bind(id.to_string())
            .fetch_all(&self.db.pool)
            .await
            .expect("events")
        }

        fn recompiled(&self) -> Vec<Vec<i64>> {
            self.recompiles.seen.lock().unwrap().clone()
        }

        fn reconfigured(&self) -> Vec<i64> {
            self.reconfigures.seen.lock().unwrap().clone()
        }
    }

    fn patch_request(token: &str, service: &str, mechanism: &str) -> Request<Body> {
        Request::builder()
            .method("PATCH")
            .uri(format!("/policies/services/{service}"))
            .header("Cookie", format!("egret_session={token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({ "mechanism": mechanism })).unwrap(),
            ))
            .unwrap()
    }

    async fn into_json(resp: axum::response::Response) -> (StatusCode, Value) {
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
        (status, json)
    }

    async fn patch_as(
        app: axum::Router,
        token: &str,
        service: &str,
        mechanism: &str,
    ) -> (StatusCode, Value) {
        into_json(
            app.oneshot(patch_request(token, service, mechanism))
                .await
                .unwrap(),
        )
        .await
    }

    const IMAGE_483: &str = "trinodb/trino:483";

    #[tokio::test]
    async fn switching_a_ready_profile_to_opa_writes_recompiles_and_reconfigures() {
        // Arrange
        let Some(mut h) = harness().await else {
            eprintln!("skip: DATABASE_URL not set");
            return;
        };
        let (svc, _) = trino_profile(&h.db, &h.store, true).await;
        h.bindings = vec![
            binding(svc.id, "trino-483", "provisioned", Some(IMAGE_483)),
            binding(svc.id, "their-trino", "external", None),
        ];
        h.reconfigures = Arc::new(Reconfigures {
            report: ReconfigureReport {
                applied: vec!["trino-483".into()],
                failed: vec![],
            },
            ..Default::default()
        });
        let (_, version_before) = h.row(svc.id).await;

        // Act
        let (status, body) = h.patch(&svc.name, "opa").await;

        // Assert
        assert_eq!(status, StatusCode::OK, "body={body}");
        assert_eq!(
            body,
            json!({
                "name": svc.name,
                "mechanism": "opa",
                "engines_reconfigured": ["trino-483"],
                "external_engines_unchanged": ["their-trino"],
                "external_engines_warning": "external engines not reconfigured: their-trino. \
                    They keep enforcing the Ranger policies until their operator installs the \
                    OPA plugin files from GET /api/control-plane/engines/{id}/opa-plugin-config \
                    (Trino 483 or later with plugin/opa).",
            })
        );
        assert_eq!(
            h.row(svc.id).await,
            ("opa".to_string(), version_before + 1),
            "mechanism written; the switch's own bump is +1 (a Ranger recompile that writes \
             adds one more; this stub recompiler writes nothing)"
        );
        assert_eq!(h.recompiled(), vec![vec![svc.id]]);
        assert_eq!(h.reconfigured(), vec![svc.id]);
        let events = h.events(svc.id).await;
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].0, "profile:mechanism");
        assert_eq!(events[0].1["service"], json!(svc.name), "{events:?}");
        assert_eq!(events[0].1["before"], "ranger", "{events:?}");
        assert_eq!(events[0].1["after"], "opa", "{events:?}");
    }

    /// One refused state: the profile, the harness settings, the message.
    async fn assert_refused(h: &Harness, svc_id: i64, service: &str, message: &str) {
        let (_, version_before) = h.row(svc_id).await;
        let (status, body) = h.patch(service, "opa").await;
        assert_eq!(status, StatusCode::CONFLICT, "{message}: body={body}");
        assert_eq!(body["code"], "mechanism_refused", "body={body}");
        let refusals: Vec<String> = body["refusals"]
            .as_array()
            .expect("refusals")
            .iter()
            .map(|r| r.as_str().unwrap().to_string())
            .collect();
        assert!(
            refusals.iter().any(|r| r == message),
            "{message} not in {refusals:?}"
        );
        assert_eq!(body["error"], json!(refusals[0]), "body={body}");
        assert_eq!(
            h.row(svc_id).await,
            ("ranger".to_string(), version_before),
            "nothing written for: {message}"
        );
        assert!(h.events(svc_id).await.is_empty(), "no event for: {message}");
        assert!(h.recompiled().is_empty(), "no recompile for: {message}");
        assert!(h.reconfigured().is_empty(), "no reconfigure for: {message}");
    }

    #[tokio::test]
    async fn each_refusal_is_a_409_that_names_its_condition_and_writes_nothing() {
        let Some(mut h) = harness().await else {
            eprintln!("skip: DATABASE_URL not set");
            return;
        };

        // The gate off, on an otherwise ready profile.
        let (svc, _) = trino_profile(&h.db, &h.store, true).await;
        h.opa_enabled = false;
        assert_refused(
            &h,
            svc.id,
            &svc.name,
            "OPA is not enabled on this installation ([governance.opa] enabled)",
        )
        .await;
        h.opa_enabled = true;

        // Not Trino.
        egret_policy::seed_starrocks(&h.store)
            .await
            .expect("starrocks def");
        let sr = h
            .store
            .create_service(&uniq("ms_sr"), "starrocks", &json!({}))
            .await
            .expect("starrocks");
        set_generated(&h.db, sr.id).await;
        assert_refused(
            &h,
            sr.id,
            &sr.name,
            "only a Trino profile can use OPA; this profile is of type starrocks",
        )
        .await;

        // Not generated.
        let (manual, _) = trino_profile(&h.db, &h.store, false).await;
        assert_refused(
            &h,
            manual.id,
            &manual.name,
            "only a generated profile can use OPA: hand-written policies are Ranger JSON that \
             OPA cannot evaluate",
        )
        .await;

        // A provisioned engine on an image without the plugin.
        let (imaged, _) = trino_profile(&h.db, &h.store, true).await;
        h.bindings = vec![binding(
            imaged.id,
            "old-trino",
            "provisioned",
            Some("trinodb/trino:470"),
        )];
        assert_refused(
            &h,
            imaged.id,
            &imaged.name,
            "engine old-trino runs trinodb/trino:470, which does not ship the Trino OPA plugin; \
             supported: trinodb/trino:483",
        )
        .await;
        h.bindings.clear();

        // The `all - …` defaults.
        let (defaults, _) = trino_profile(&h.db, &h.store, false).await;
        h.store
            .seed_default_trino_policies(defaults.id, &defaults.name, "admin")
            .await
            .expect("defaults");
        set_generated(&h.db, defaults.id).await;
        assert_refused(
            &h,
            defaults.id,
            &defaults.name,
            "the profile holds the `all - …` default policies (13); delete them first",
        )
        .await;

        // A hand-written row in the tag service.
        let (hand, tag) = trino_profile(&h.db, &h.store, true).await;
        h.store
            .create_policy(
                tag.id,
                "ms hand",
                &json!({"name": "ms hand", "policyType": 0, "isEnabled": true,
                        "resources": {"tag": {"values": ["pii"]}},
                        "policyItems": [], "denyPolicyItems": []}),
            )
            .await
            .expect("hand-written tag policy");
        assert_refused(
            &h,
            hand.id,
            &hand.name,
            &format!(
                "{} holds 1 hand-written policies, which OPA would ignore; delete them first",
                tag.name
            ),
        )
        .await;

        // Stored overlap pairs.
        let (pairs, _) = trino_profile(&h.db, &h.store, true).await;
        h.recompiles = Arc::new(Recompiles {
            overlaps: vec!["mask on lake.s.t.email: perm-rule-a, perm-rule-b".into()],
            ..Default::default()
        });
        assert_refused(
            &h,
            pairs.id,
            &pairs.name,
            "the profile serves 1 mask/row-filter pairs whose winner the artifact does not \
             express (see GET /perm/overlaps); resolve them first: mask on lake.s.t.email: \
             perm-rule-a, perm-rule-b",
        )
        .await;
    }

    #[tokio::test]
    async fn switching_back_to_ranger_is_never_refused_even_with_the_gate_off() {
        // Arrange: every condition the switch to OPA would refuse holds.
        let Some(mut h) = harness().await else {
            eprintln!("skip: DATABASE_URL not set");
            return;
        };
        let (svc, _) = trino_profile(&h.db, &h.store, false).await;
        h.set_mechanism(svc.id, "opa").await;
        h.opa_enabled = false;
        h.bindings = vec![binding(svc.id, "old-trino", "provisioned", None)];
        h.recompiles = Arc::new(Recompiles {
            overlaps: vec!["mask on x: a, b".into()],
            ..Default::default()
        });
        let (_, version_before) = h.row(svc.id).await;

        // Act
        let (status, body) = h.patch(&svc.name, "ranger").await;

        // Assert
        assert_eq!(status, StatusCode::OK, "body={body}");
        assert_eq!(body["mechanism"], "ranger", "body={body}");
        assert_eq!(
            h.row(svc.id).await,
            ("ranger".to_string(), version_before + 1)
        );
        assert_eq!(
            h.recompiled(),
            vec![vec![svc.id]],
            "the Ranger recompile runs"
        );
        assert_eq!(h.reconfigured(), vec![svc.id]);
        let events = h.events(svc.id).await;
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].1["before"], "opa", "{events:?}");
        assert_eq!(events[0].1["after"], "ranger", "{events:?}");
    }

    #[tokio::test]
    async fn the_same_mechanism_again_writes_nothing_but_reconfigures() {
        let Some(h) = harness().await else {
            eprintln!("skip: DATABASE_URL not set");
            return;
        };
        for mechanism in ["opa", "ranger"] {
            // Arrange
            let (svc, _) = trino_profile(&h.db, &h.store, true).await;
            h.set_mechanism(svc.id, mechanism).await;
            let (_, version_before) = h.row(svc.id).await;
            let recompiles_before = h.recompiled().len();
            let reconfigures_before = h.reconfigured().len();

            // Act
            let (status, body) = h.patch(&svc.name, mechanism).await;

            // Assert
            assert_eq!(status, StatusCode::OK, "{mechanism}: body={body}");
            assert_eq!(body["mechanism"], mechanism, "body={body}");
            assert_eq!(
                h.row(svc.id).await,
                (mechanism.to_string(), version_before),
                "{mechanism}: nothing written"
            );
            assert!(h.events(svc.id).await.is_empty(), "{mechanism}: no event");
            assert_eq!(h.recompiled().len(), recompiles_before, "{mechanism}");
            assert_eq!(
                h.reconfigured()[reconfigures_before..],
                [svc.id],
                "{mechanism}: the reconfigure re-runs once"
            );
        }
    }

    #[tokio::test]
    async fn a_partly_failed_reconfigure_is_a_502_naming_both_lists_and_the_switch_stays() {
        // Arrange
        let Some(mut h) = harness().await else {
            eprintln!("skip: DATABASE_URL not set");
            return;
        };
        let (svc, _) = trino_profile(&h.db, &h.store, true).await;
        h.bindings = vec![
            binding(svc.id, "trino-a", "provisioned", Some(IMAGE_483)),
            binding(svc.id, "trino-b", "provisioned", Some(IMAGE_483)),
        ];
        h.reconfigures = Arc::new(Reconfigures {
            report: ReconfigureReport {
                applied: vec!["trino-a".into()],
                failed: vec![("trino-b".into(), "the rollout timed out".into())],
            },
            ..Default::default()
        });
        let (_, version_before) = h.row(svc.id).await;

        // Act
        let (status, body) = h.patch(&svc.name, "opa").await;

        // Assert
        assert_eq!(status, StatusCode::BAD_GATEWAY, "body={body}");
        assert_eq!(
            body,
            json!({
                "error": "the profile is now on opa; engines not reconfigured: trino-b. \
                          Send the same PATCH again to retry.",
                "engines_reconfigured": ["trino-a"],
                "engines_failed": [{"engine": "trino-b", "error": "the rollout timed out"}],
                "external_engines_unchanged": [],
            })
        );
        assert_eq!(
            h.row(svc.id).await,
            ("opa".to_string(), version_before + 1),
            "the switch stays committed"
        );
        assert_eq!(h.events(svc.id).await.len(), 1);
    }

    /// SP2b Task 20F (T15 M7 + m3): after OPA→Ranger an external engine fails
    /// every query until its operator re-installs the Ranger plugin; the 200
    /// says so, and so does a 502, which also lists the external engines.
    #[tokio::test]
    async fn switching_back_to_ranger_warns_that_external_engines_fail_until_reinstalled() {
        // Arrange
        let Some(mut h) = harness().await else {
            eprintln!("skip: DATABASE_URL not set");
            return;
        };
        let (svc, _) = trino_profile(&h.db, &h.store, true).await;
        h.set_mechanism(svc.id, "opa").await;
        h.bindings = vec![
            binding(svc.id, "trino-a", "provisioned", Some(IMAGE_483)),
            binding(svc.id, "ext-1", "external", None),
            binding(svc.id, "ext-2", "external", None),
        ];
        h.reconfigures = Arc::new(Reconfigures {
            report: ReconfigureReport {
                applied: vec!["trino-a".into()],
                failed: vec![],
            },
            ..Default::default()
        });
        let warning = "external engines not reconfigured: ext-1, ext-2. Any of them running \
                       the OPA plugin fails every query until its operator re-installs the \
                       Ranger plugin files from \
                       GET /api/control-plane/engines/{id}/ranger-plugin-config; one still \
                       running the Ranger plugin is unaffected.";

        // Act
        let (status, body) = h.patch(&svc.name, "ranger").await;

        // Assert
        assert_eq!(status, StatusCode::OK, "body={body}");
        assert_eq!(
            body["external_engines_unchanged"],
            json!(["ext-1", "ext-2"])
        );
        assert_eq!(body["external_engines_warning"], warning, "body={body}");

        // Arrange: the same switch, the provisioned engine failing.
        h.reconfigures = Arc::new(Reconfigures {
            report: ReconfigureReport {
                applied: vec![],
                failed: vec![("trino-a".into(), "the rollout timed out".into())],
            },
            ..Default::default()
        });

        // Act
        let (status, body) = h.patch(&svc.name, "ranger").await;

        // Assert: the 502 carries both external fields too.
        assert_eq!(status, StatusCode::BAD_GATEWAY, "body={body}");
        assert_eq!(
            body["external_engines_unchanged"],
            json!(["ext-1", "ext-2"])
        );
        assert_eq!(body["external_engines_warning"], warning, "body={body}");
    }

    #[tokio::test]
    async fn without_an_external_engine_the_200_carries_no_warning() {
        let Some(mut h) = harness().await else {
            eprintln!("skip: DATABASE_URL not set");
            return;
        };
        let (svc, _) = trino_profile(&h.db, &h.store, true).await;
        h.set_mechanism(svc.id, "opa").await;
        h.bindings = vec![binding(svc.id, "trino-a", "provisioned", Some(IMAGE_483))];

        let (status, body) = h.patch(&svc.name, "ranger").await;

        assert_eq!(status, StatusCode::OK, "body={body}");
        assert_eq!(body["external_engines_unchanged"], json!([]));
        assert!(
            body.get("external_engines_warning").is_none(),
            "body={body}"
        );
    }

    #[tokio::test]
    async fn the_patch_locks_the_tag_service_row_too() {
        // Arrange
        let Some(h) = harness().await else {
            eprintln!("skip: DATABASE_URL not set");
            return;
        };
        let (svc, tag) = trino_profile(&h.db, &h.store, true).await;
        let token = h.token(None).await;
        let mut holder = h.db.pool.begin().await.expect("holder");
        sqlx::query("SELECT id FROM policy.authz_services WHERE id = $1 FOR UPDATE")
            .bind(tag.id)
            .execute(&mut *holder)
            .await
            .expect("hold the tag service row");

        // Act
        let (app, service) = (h.app(), svc.name.clone());
        let mut patch = tokio::spawn(async move { patch_as(app, &token, &service, "opa").await });
        let early = tokio::time::timeout(std::time::Duration::from_millis(300), &mut patch).await;

        // Assert
        assert!(
            early.is_err(),
            "the PATCH answered while the tag service row was held"
        );
        holder.rollback().await.expect("release");
        let (status, body) = patch.await.expect("join");
        assert_eq!(status, StatusCode::OK, "body={body}");
        assert_eq!(h.row(svc.id).await.0, "opa");
    }

    /// Answers `bindings`, and records, at each call, whether profile
    /// `service_id`'s row is locked by someone else (`FOR UPDATE NOWAIT`
    /// from its own connection).
    struct LockProbe {
        db: Db,
        service_id: i64,
        bindings: Vec<EngineBinding>,
        locked: Mutex<Vec<bool>>,
    }

    #[async_trait::async_trait]
    impl EngineBindingReader for LockProbe {
        async fn list_engine_bindings(&self) -> Result<Vec<EngineBinding>, EngineReadError> {
            let mut tx = self.db.pool.begin().await.expect("probe tx");
            let free =
                sqlx::query("SELECT id FROM policy.authz_services WHERE id = $1 FOR UPDATE NOWAIT")
                    .bind(self.service_id)
                    .execute(&mut *tx)
                    .await
                    .is_ok();
            tx.rollback().await.expect("probe rollback");
            self.locked.lock().unwrap().push(!free);
            Ok(self.bindings.clone())
        }
        async fn engine_catalog_names(&self, _: Uuid) -> Result<Vec<String>, EngineReadError> {
            Ok(vec![])
        }
        async fn warehouse_schemas(&self, _: Uuid) -> Result<Vec<String>, EngineReadError> {
            Ok(vec![])
        }
        async fn warehouse_tables(&self, _: Uuid, _: &str) -> Result<Vec<String>, EngineReadError> {
            Ok(vec![])
        }
        async fn warehouse_columns(
            &self,
            _: Uuid,
            _: &str,
            _: &str,
        ) -> Result<Vec<String>, EngineReadError> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn the_bindings_the_image_check_judges_are_read_under_the_lock() {
        // Arrange
        let Some(mut h) = harness().await else {
            eprintln!("skip: DATABASE_URL not set");
            return;
        };
        let (svc, _) = trino_profile(&h.db, &h.store, true).await;
        let probe = Arc::new(LockProbe {
            db: h.db.clone(),
            service_id: svc.id,
            bindings: vec![binding(
                svc.id,
                "old-trino",
                "provisioned",
                Some("trinodb/trino:470"),
            )],
            locked: Mutex::new(Vec::new()),
        });
        h.engines = Some(probe.clone());

        // Act
        let (status, body) = h.patch(&svc.name, "opa").await;

        // Assert
        assert_eq!(status, StatusCode::CONFLICT, "body={body}");
        assert_eq!(
            *probe.locked.lock().unwrap(),
            vec![true],
            "the bindings are listed once, after the profile row is locked"
        );
    }

    #[tokio::test]
    async fn without_policy_update_it_is_403() {
        // Arrange
        let Some(mut h) = harness().await else {
            eprintln!("skip: DATABASE_URL not set");
            return;
        };
        let (svc, _) = trino_profile(&h.db, &h.store, true).await;
        let reader = h.token(Some("policy:read")).await;
        let creator = h.token(Some("policy:create")).await;
        let updater = h.token(Some("policy:update")).await;
        h.pdp = Arc::new(
            egret_auth::pdp::CedarPdp::build(h.db.clone(), &egret_events::EventBus::new())
                .await
                .expect("CedarPdp"),
        );
        let (_, version_before) = h.row(svc.id).await;

        // Act + Assert
        for token in [&reader, &creator] {
            let (status, body) = patch_as(h.app(), token, &svc.name, "opa").await;
            assert_eq!(status, StatusCode::FORBIDDEN, "body={body}");
        }
        assert_eq!(h.row(svc.id).await, ("ranger".to_string(), version_before));
        assert!(h.reconfigured().is_empty());
        let (status, body) = patch_as(h.app(), &updater, &svc.name, "opa").await;
        assert_eq!(status, StatusCode::OK, "body={body}");
        assert_eq!(h.row(svc.id).await.0, "opa");
    }
}
