//! `shadow-compile`: every recompile compiles a second time on the same
//! transaction and asserts byte-identical output. During a sub-project 1
//! family task the second compile is the OLD path; between tasks it is the same
//! path (a determinism check, the precondition for a golden).
//!
//! **Limitation: run shadow sweeps with `-- --test-threads=1`.** The second
//! compile shares the caller's READ COMMITTED transaction, so a row a
//! concurrent test commits between the two compiles (a gated tag or a tag
//! rule, which every Trino profile reads) is seen by the second compile only,
//! and reads as a mismatch. Measured in Task 1: 11 such failures in a
//! parallel workspace run, none single-threaded.
use crate::trigger::{compile_desired, Compiled};
use egret_perm::CompileError;

fn bytes(c: &Compiled) -> String {
    let desired: Vec<_> = c
        .desired
        .iter()
        .map(|((sid, name), dp)| {
            (
                sid,
                name,
                serde_json::to_string(&dp.body).unwrap(),
                dp.source_kind as u8,
                dp.grant_id,
                dp.rule_id,
            )
        })
        .collect();
    let skipped: Vec<_> = c
        .skipped
        .iter()
        .map(|s| (s.source_kind as u8, s.id, s.reason.clone()))
        .collect();
    format!("{desired:?}\n{skipped:?}\n{:?}\n{}", c.sweep, c.uncounted)
}

/// Assert two compiles of one profile agree byte for byte: every desired
/// policy (key, body, source), every skip (kind, id, reason, in order), the
/// sweep set and the uncounted total.
pub(crate) fn assert_same(context: &str, a: &Compiled, b: &Compiled) {
    let (a, b) = (bytes(a), bytes(b));
    assert_eq!(a, b, "shadow compile of {context} differs");
}

pub(crate) async fn compare(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    first: &Compiled,
) -> Result<(), CompileError> {
    let second = compile_desired(conn, service_id)
        .await?
        .expect("the same profile compiled the first time");
    assert_same(&format!("service {service_id}"), first, &second);
    if let Some(path) = std::env::var_os("EGRET_TEST_SHADOW_LOG") {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("shadow log");
        writeln!(f, "{service_id}").ok();
    }
    Ok(())
}
