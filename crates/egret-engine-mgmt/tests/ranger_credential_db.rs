//! The per-engine Ranger plugin credential (spec 2026-10-20-ranger-surface-auth
//! §4.3, A10) against a real Postgres: minting, the current/previous rotation
//! window, retiring the previous credential by exact id, and the verifier the
//! `/ranger` surface consults.
//!
//! DB-gated: without `DATABASE_URL` this FAILS unless `EGRET_ALLOW_DB_SKIP` is
//! set, in which case every test prints SKIPPED and asserts nothing.
//!
//! The database is shared with concurrently running tests, so every query here
//! is scoped to the test's own engine id.

use egret_engine_mgmt::ranger_credential::{DbVerifier, RangerCredentialStore};
use egret_engine_mgmt::{Engine, EngineMgmtStore};
use egret_engine_mgmt_api::credential::{plugin_username, EngineCredentialVerifier};
use egret_metastore::Db;
use uuid::Uuid;

struct Fixture {
    db: Db,
    store: EngineMgmtStore,
    creds: RangerCredentialStore,
}

async fn harness() -> Option<Fixture> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    );
    let store = EngineMgmtStore::new(db.clone());
    let creds = RangerCredentialStore::new(store.clone(), secrets);
    Some(Fixture { db, store, creds })
}

macro_rules! harness_or_skip {
    () => {
        match harness().await {
            Some(f) => f,
            None => {
                eprintln!(
                    "SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set"
                );
                return;
            }
        }
    };
}

async fn seed_engine(f: &Fixture) -> Engine {
    f.store
        .create_engine(
            &format!("rc-eng-{}", Uuid::new_v4().simple()),
            "starrocks",
            "egret-engines",
            &serde_json::json!({}),
            None,
        )
        .await
        .expect("seed engine")
}

async fn reload(f: &Fixture, id: Uuid) -> Engine {
    f.store
        .get_engine(id)
        .await
        .expect("get engine")
        .expect("engine exists")
}

/// The ids of this engine's credential rows still in the secret store.
async fn ranger_secret_rows(f: &Fixture, engine_id: Uuid) -> Vec<Uuid> {
    let prefix = format!("engine-ranger-{engine_id}-");
    sqlx::query_scalar(
        "SELECT id FROM workspace_secrets \
         WHERE workspace_id IS NULL AND left(name, length($1)) = $1 ORDER BY id",
    )
    .bind(&prefix)
    .fetch_all(&f.db.pool)
    .await
    .expect("list ranger secret rows")
}

async fn verifies(f: &Fixture, engine_id: Uuid, password: &str) -> bool {
    DbVerifier::new(f.creds.clone())
        .verify(engine_id, password)
        .await
        .expect("verify")
        .is_some()
}

#[tokio::test]
async fn ensure_mints_once_and_reuses() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    assert_eq!(e.ranger_secret_id, None);

    let first = f.creds.ensure(&e).await.expect("ensure");
    let row = reload(&f, e.id).await;
    let second = f.creds.ensure(&row).await.expect("ensure again");

    assert_eq!(first.username, plugin_username(e.id));
    assert!(!first.password.is_empty());
    assert_eq!(first.password, second.password);
    let current = row.ranger_secret_id.expect("current credential recorded");
    assert_eq!(row.ranger_secret_prev_id, None);
    assert_eq!(ranger_secret_rows(&f, e.id).await, vec![current]);
}

#[tokio::test]
async fn ensure_mints_afresh_when_the_recorded_secret_vanished() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    let p1 = f.creds.ensure(&e).await.expect("ensure");
    let row = reload(&f, e.id).await;
    sqlx::query("DELETE FROM workspace_secrets WHERE id = $1")
        .bind(row.ranger_secret_id.unwrap())
        .execute(&f.db.pool)
        .await
        .expect("delete out of band");

    let p2 = f.creds.ensure(&row).await.expect("ensure after loss");

    assert_ne!(p1.password, p2.password);
    assert!(verifies(&f, e.id, &p2.password).await);
}

#[tokio::test]
async fn rotate_demotes_the_old_credential_and_both_verify() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    let p1 = f.creds.ensure(&e).await.expect("ensure");
    let before = reload(&f, e.id).await;

    let p2 = f.creds.rotate(&before).await.expect("rotate");

    assert_ne!(p1.password, p2.password);
    let after = reload(&f, e.id).await;
    assert_eq!(after.ranger_secret_prev_id, before.ranger_secret_id);
    assert_ne!(after.ranger_secret_id, before.ranger_secret_id);
    assert!(verifies(&f, e.id, &p1.password).await);
    assert!(verifies(&f, e.id, &p2.password).await);
}

#[tokio::test]
async fn retire_previous_revokes_only_the_old_one() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    let p1 = f.creds.ensure(&e).await.expect("ensure");
    let p2 = f
        .creds
        .rotate(&reload(&f, e.id).await)
        .await
        .expect("rotate");

    f.creds.retire_previous(e.id).await;

    assert!(!verifies(&f, e.id, &p1.password).await);
    assert!(verifies(&f, e.id, &p2.password).await);
    let row = reload(&f, e.id).await;
    assert_eq!(row.ranger_secret_prev_id, None);
    assert_eq!(
        ranger_secret_rows(&f, e.id).await,
        vec![row.ranger_secret_id.unwrap()]
    );
}

#[tokio::test]
async fn retire_previous_with_nothing_to_retire_changes_nothing() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    let p1 = f.creds.ensure(&e).await.expect("ensure");

    f.creds.retire_previous(e.id).await;

    assert!(verifies(&f, e.id, &p1.password).await);
    assert_eq!(ranger_secret_rows(&f, e.id).await.len(), 1);
}

/// The compare-and-set inside `retire_previous`: a caller whose read of the
/// previous id is stale (a rotation moved the column since) must neither clear
/// the column nor delete the secret it named — the stale id may now be the
/// CURRENT credential, which the cluster certainly holds.
#[tokio::test]
async fn retiring_a_stale_previous_id_deletes_nothing() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    let p1 = f.creds.ensure(&e).await.expect("ensure");
    let p2 = f
        .creds
        .rotate(&reload(&f, e.id).await)
        .await
        .expect("rotate");
    let row = reload(&f, e.id).await;
    let stale = row.ranger_secret_id.unwrap(); // not the previous one

    f.creds.retire_previous_if(e.id, stale).await;

    let after = reload(&f, e.id).await;
    assert_eq!(after.ranger_secret_id, row.ranger_secret_id);
    assert_eq!(after.ranger_secret_prev_id, row.ranger_secret_prev_id);
    assert!(verifies(&f, e.id, &p1.password).await);
    assert!(verifies(&f, e.id, &p2.password).await);
    assert_eq!(ranger_secret_rows(&f, e.id).await.len(), 2);
}

#[tokio::test]
async fn a_second_rotation_before_retiring_is_refused_and_changes_nothing() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    let p1 = f.creds.ensure(&e).await.expect("ensure");
    let p2 = f
        .creds
        .rotate(&reload(&f, e.id).await)
        .await
        .expect("rotate");
    let before = reload(&f, e.id).await;
    let rows_before = ranger_secret_rows(&f, e.id).await;

    let refused = f.creds.rotate(&before).await;

    // The cluster may still hold p1 (no successful apply has retired it), so a
    // second rotation must not push it out of the window.
    match refused {
        Err(egret_engine_mgmt::EngineHttpError::Conflict(msg)) => {
            assert!(msg.contains("pending"), "message: {msg}")
        }
        other => panic!("expected a 409 Conflict, got {other:?}"),
    }
    let after = reload(&f, e.id).await;
    assert_eq!(after.ranger_secret_id, before.ranger_secret_id);
    assert_eq!(after.ranger_secret_prev_id, before.ranger_secret_prev_id);
    assert_eq!(ranger_secret_rows(&f, e.id).await, rows_before);
    assert!(verifies(&f, e.id, &p1.password).await);
    assert!(verifies(&f, e.id, &p2.password).await);
}

/// Once the previous credential is retired, rotating again is allowed.
#[tokio::test]
async fn a_rotation_after_retiring_is_allowed() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    f.creds.ensure(&e).await.expect("ensure");
    let p2 = f
        .creds
        .rotate(&reload(&f, e.id).await)
        .await
        .expect("rotate");
    f.creds.retire_previous(e.id).await;

    let p3 = f
        .creds
        .rotate(&reload(&f, e.id).await)
        .await
        .expect("rotate after retire");

    assert!(verifies(&f, e.id, &p2.password).await);
    assert!(verifies(&f, e.id, &p3.password).await);
}

/// A previous id whose secret is already gone (dangling) does not block a
/// rotation: nothing can present it, so it is overwritten.
#[tokio::test]
async fn a_dangling_previous_id_does_not_block_a_rotation() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    f.creds.ensure(&e).await.expect("ensure");
    let p2 = f
        .creds
        .rotate(&reload(&f, e.id).await)
        .await
        .expect("rotate");
    let row = reload(&f, e.id).await;
    sqlx::query("DELETE FROM workspace_secrets WHERE id = $1")
        .bind(row.ranger_secret_prev_id.unwrap())
        .execute(&f.db.pool)
        .await
        .expect("delete previous out of band");

    let p3 = f
        .creds
        .rotate(&row)
        .await
        .expect("rotate over a dangling previous");

    let after = reload(&f, e.id).await;
    assert_eq!(after.ranger_secret_prev_id, row.ranger_secret_id);
    assert!(verifies(&f, e.id, &p2.password).await);
    assert!(verifies(&f, e.id, &p3.password).await);
}

/// `ensure`'s compare-and-set covers the previous column too: a caller holding
/// a stale read (taken before `retire_previous` cleared and deleted the
/// previous credential) must not write that retired id back onto the row.
#[tokio::test]
async fn ensure_with_a_stale_read_never_writes_back_a_retired_previous_id() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    f.creds.ensure(&e).await.expect("ensure");
    f.creds
        .rotate(&reload(&f, e.id).await)
        .await
        .expect("rotate");
    let stale = reload(&f, e.id).await;
    let retired = stale.ranger_secret_prev_id.unwrap();
    // The current secret vanishes, so `ensure` must mint (and so write).
    sqlx::query("DELETE FROM workspace_secrets WHERE id = $1")
        .bind(stale.ranger_secret_id.unwrap())
        .execute(&f.db.pool)
        .await
        .expect("delete current out of band");
    f.creds.retire_previous(e.id).await;

    let p = f
        .creds
        .ensure(&stale)
        .await
        .expect("ensure from a stale read");

    let row = reload(&f, e.id).await;
    assert_ne!(row.ranger_secret_prev_id, Some(retired));
    assert_eq!(row.ranger_secret_prev_id, None);
    assert!(verifies(&f, e.id, &p.password).await);
}

#[tokio::test]
async fn verify_is_none_for_an_unknown_engine_and_for_a_wrong_password() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    let p1 = f.creds.ensure(&e).await.expect("ensure");
    let other = seed_engine(&f).await;
    let other_pw = f.creds.ensure(&other).await.expect("ensure other");

    assert!(!verifies(&f, Uuid::new_v4(), &p1.password).await);
    assert!(!verifies(&f, e.id, "wrong").await);
    assert!(!verifies(&f, e.id, &format!("{}x", p1.password)).await);
    assert!(!verifies(&f, e.id, &p1.password[..p1.password.len() - 1]).await);
    assert!(!verifies(&f, e.id, "").await);
    // Another engine's valid password is not this engine's.
    assert!(!verifies(&f, e.id, &other_pw.password).await);
    assert!(verifies(&f, e.id, &p1.password).await);
}

#[tokio::test]
async fn verify_is_none_for_an_engine_without_a_credential() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;

    assert!(!verifies(&f, e.id, "").await);
    assert!(!verifies(&f, e.id, "anything").await);
}

#[tokio::test]
async fn verify_returns_the_engines_bound_profile() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    // `authz_services.type` is a foreign key onto `authz_service_defs(name)`
    // and no migration seeds a def, so this test seeds its own.
    let def = format!("rc_def_{}", &Uuid::new_v4().simple().to_string()[..12]);
    let def_id = (Uuid::new_v4().as_u128() % 1_000_000_000) as i64 + 1_000_000_000;
    sqlx::query(
        "INSERT INTO policy.authz_service_defs (id, name, def_jsonb) VALUES ($1, $2, '{}')",
    )
    .bind(def_id)
    .bind(&def)
    .execute(&f.db.pool)
    .await
    .expect("seed service def");
    let profile: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type) VALUES ($1, $1) RETURNING id",
    )
    .bind(&def)
    .fetch_one(&f.db.pool)
    .await
    .expect("seed profile");
    f.store
        .set_engine_authz_service(e.id, Some(profile))
        .await
        .expect("bind");
    let p1 = f.creds.ensure(&e).await.expect("ensure");

    let identity = DbVerifier::new(f.creds.clone())
        .verify(e.id, &p1.password)
        .await
        .expect("verify")
        .expect("valid credential");

    assert_eq!(identity.engine_id, e.id);
    assert_eq!(identity.authz_service_id, Some(profile));
}

#[tokio::test]
async fn concurrent_ensure_converges_on_one_credential() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;

    let (a, b) = tokio::join!(f.creds.ensure(&e), f.creds.ensure(&e));

    let (a, b) = (a.expect("ensure a"), b.expect("ensure b"));
    assert_eq!(a.password, b.password);
    let row = reload(&f, e.id).await;
    assert_eq!(
        ranger_secret_rows(&f, e.id).await,
        vec![row.ranger_secret_id.unwrap()]
    );
    assert!(verifies(&f, e.id, &a.password).await);
}

#[test]
fn the_debug_output_never_carries_the_password() {
    let c = egret_engine_mgmt::ranger_credential::PluginCredential {
        username: "engine-x".into(),
        password: "s3cr3t-value".into(),
    };
    let dbg = format!("{c:?}");
    assert!(dbg.contains("engine-x"));
    assert!(!dbg.contains("s3cr3t-value"));
}

/// `retire_previous_rendered`'s KEEP branch: an apply that rendered a credential
/// which is no longer current (a rotation committed a newer one after the apply
/// was built) must not retire the previous slot — that slot may hold exactly
/// what this apply just put on the cluster.
#[tokio::test]
async fn retiring_after_an_apply_of_a_superseded_credential_keeps_the_previous() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    let c1 = f.creds.ensure(&e).await.expect("ensure");
    let c1_id = reload(&f, e.id).await.ranger_secret_id.unwrap();
    let c2 = f
        .creds
        .rotate(&reload(&f, e.id).await)
        .await
        .expect("rotate");
    assert_ne!(c1.password, c2.password);
    let row = reload(&f, e.id).await;
    assert_eq!(row.ranger_secret_prev_id, Some(c1_id));

    // The apply that lands rendered c1, the now-previous credential.
    f.creds.retire_previous_rendered(e.id, &c1.password).await;

    let after = reload(&f, e.id).await;
    assert_eq!(after.ranger_secret_prev_id, Some(c1_id), "previous kept");
    assert_eq!(after.ranger_secret_id, row.ranger_secret_id);
    assert!(
        ranger_secret_rows(&f, e.id).await.contains(&c1_id),
        "the previous credential's secret still exists"
    );
    assert!(verifies(&f, e.id, &c1.password).await);

    // Control: the apply that rendered the current one does retire it.
    f.creds.retire_previous_rendered(e.id, &c2.password).await;
    assert_eq!(reload(&f, e.id).await.ranger_secret_prev_id, None);
    assert!(!ranger_secret_rows(&f, e.id).await.contains(&c1_id));
}

// ---- external engines (Task 7): rotate without an apply, retire on the
// operator's statement of what is installed ----

/// A lost compare-and-set on an external rotation is a 409, never the
/// winner's credential handed out as if it were new.
#[tokio::test]
async fn an_external_rotation_with_a_stale_read_is_a_conflict() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    f.creds.ensure(&e).await.expect("ensure");
    let stale = reload(&f, e.id).await;
    let (id2, p2) = f.creds.rotate_external(&stale).await.expect("rotate");
    let after_first = reload(&f, e.id).await;
    assert_eq!(after_first.ranger_secret_id, Some(id2));
    let rows = ranger_secret_rows(&f, e.id).await;

    // `stale` names no previous, so it passes the pending check and loses
    // the compare-and-set.
    match f.creds.rotate_external(&stale).await {
        Err(egret_engine_mgmt::EngineHttpError::Conflict(msg)) => {
            assert!(msg.contains("concurrent"), "{msg}");
            assert!(!msg.contains(&p2.password));
        }
        other => panic!("expected a 409 Conflict, got {other:?}"),
    }
    let after = reload(&f, e.id).await;
    assert_eq!(after.ranger_secret_id, after_first.ranger_secret_id);
    assert_eq!(
        after.ranger_secret_prev_id,
        after_first.ranger_secret_prev_id
    );
    assert_eq!(
        ranger_secret_rows(&f, e.id).await,
        rows,
        "nothing minted is left"
    );
}

#[tokio::test]
async fn an_external_rotation_while_pending_names_the_retire_route() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    f.creds.ensure(&e).await.expect("ensure");
    f.creds
        .rotate_external(&reload(&f, e.id).await)
        .await
        .expect("rotate");
    match f.creds.rotate_external(&reload(&f, e.id).await).await {
        Err(egret_engine_mgmt::EngineHttpError::Conflict(msg)) => {
            assert!(msg.contains("retire-previous-ranger-credential"), "{msg}");
            assert!(!msg.contains("apply"), "{msg}");
        }
        other => panic!("expected a 409 Conflict, got {other:?}"),
    }
    // The provisioned wording stays about the apply.
    match f.creds.rotate(&reload(&f, e.id).await).await {
        Err(egret_engine_mgmt::EngineHttpError::Conflict(msg)) => {
            assert!(msg.contains("apply"), "{msg}")
        }
        other => panic!("expected a 409 Conflict, got {other:?}"),
    }
}

/// The retire is a compare-and-set on BOTH columns: the current one must be
/// the credential the operator says is installed.
#[tokio::test]
async fn a_confirmed_retire_naming_a_credential_that_is_not_current_deletes_nothing() {
    let f = harness_or_skip!();
    let e = seed_engine(&f).await;
    let p1 = f.creds.ensure(&e).await.expect("ensure");
    let c1 = reload(&f, e.id).await.ranger_secret_id.unwrap();
    let (c2, p2) = f
        .creds
        .rotate_external(&reload(&f, e.id).await)
        .await
        .expect("rotate");
    let row = reload(&f, e.id).await;
    let rows = ranger_secret_rows(&f, e.id).await;

    for wrong in [c1, Uuid::new_v4()] {
        match f.creds.retire_previous_confirmed(&row, wrong).await {
            Err(egret_engine_mgmt::EngineHttpError::Conflict(_)) => {}
            other => panic!("expected a 409 Conflict, got {other:?}"),
        }
    }
    assert_eq!(ranger_secret_rows(&f, e.id).await, rows);
    assert!(verifies(&f, e.id, &p1.password).await);

    // The row read before a concurrent change: the column compare-and-set
    // still refuses (simulated: the current one moved under the read).
    let mut stale = row.clone();
    stale.ranger_secret_id = Some(Uuid::new_v4());
    match f
        .creds
        .retire_previous_confirmed(&stale, stale.ranger_secret_id.unwrap())
        .await
    {
        Err(egret_engine_mgmt::EngineHttpError::Conflict(_)) => {}
        other => panic!("expected a 409 Conflict, got {other:?}"),
    }
    assert!(verifies(&f, e.id, &p1.password).await);

    f.creds
        .retire_previous_confirmed(&row, c2)
        .await
        .expect("matching retire");
    assert!(!verifies(&f, e.id, &p1.password).await);
    assert!(verifies(&f, e.id, &p2.password).await);
    assert_eq!(ranger_secret_rows(&f, e.id).await, vec![c2]);
    // Idempotent with the matching statement; a mismatch is still refused.
    let row = reload(&f, e.id).await;
    f.creds
        .retire_previous_confirmed(&row, c2)
        .await
        .expect("idempotent");
    assert!(f.creds.retire_previous_confirmed(&row, c1).await.is_err());
}
