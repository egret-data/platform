//! `warn_unknown_membership_provenance` logs a group's NAME, and an OIDC
//! group's name comes from an IdP claim. Logged with `%` (Display), a name
//! carrying a newline writes a second line that reads like egret's own. It is
//! logged with `?` (Debug) so the newline is escaped. A promise about a log
//! line is prose until a subscriber reads it back.
//!
//! # Why this is a whole binary for one test
//!
//! Same reason as `egret-dataapi/tests/serving_guard_logs.rs`: `tracing` caches
//! a callsite's `Interest` process-wide, and `set_default` is thread-local. In a
//! multi-test binary another test's subscriber can be the one that first
//! evaluates this `warn!` callsite; if it is not interested, the callsite is
//! cached disabled and this test's subscriber never sees the event.
//!
//! DB-gated; FAILS without DATABASE_URL unless EGRET_ALLOW_DB_SKIP is set.

use egret_auth::groups::GroupRepo;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use std::sync::{Arc, Mutex};

/// Collects `tracing` output so the test can assert on what was written.
#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log lock").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogWriter;
    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(self.0.clone())
    }
}

#[tokio::test]
async fn an_idp_group_name_with_a_newline_cannot_forge_a_log_line() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    // A local user in an OIDC group: the (b) shape, which logs the group name.
    let forged = "FORGED-LINE level=INFO all memberships reviewed";
    let name = format!("t-prov-log-{suffix}\n{forged}");
    let gid = GroupRepo::new(db.clone())
        .upsert_oidc_group(&name)
        .await
        .expect("oidc group");
    let user = UserRepo::new(db.clone())
        .insert_local(&format!("t-prov-log-{suffix}@test.local"), "L", "x")
        .await
        .expect("local user");
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(user.0)
        .bind(gid)
        .execute(&db.pool)
        .await
        .expect("membership");

    let capture = LogCapture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_ansi(false)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    egret_auth::groups::warn_unknown_membership_provenance(&db.pool)
        .await
        .expect("provenance pass");
    drop(guard);

    let log = String::from_utf8(capture.0.lock().expect("log lock").clone()).expect("utf8");
    let line = log
        .lines()
        .find(|l| l.contains(&gid.to_string()))
        .unwrap_or_else(|| panic!("the pass must log the seeded membership: {log}"));
    assert!(
        line.contains(&format!("\\n{forged}")),
        "the newline is escaped inside the event's own line: {line}"
    );
    assert!(
        !log.lines().any(|l| l.starts_with(forged)),
        "no line of the log may begin with the IdP-supplied text: {log}"
    );
}
