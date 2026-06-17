//! Real Ranger policy-engine enforcement gate. Skips unless EGRET_TEST_RANGER_E2E=1.
//! Requires: a JVM + Maven + network (pulls ranger-plugins-common from Maven Central).
use std::process::Command;

#[test]
fn ranger_engine_enforces_egret_policies() {
    if std::env::var("EGRET_TEST_RANGER_E2E").ok().as_deref() != Some("1") {
        eprintln!("EGRET_TEST_RANGER_E2E != 1 — skipping Ranger engine gate");
        return;
    }
    let dir = env!("CARGO_MANIFEST_DIR").to_string() + "/../../tests/ranger";
    let status = Command::new("mvn")
        .args(["-q", "test"])
        .current_dir(&dir)
        .status()
        .expect("run mvn");
    assert!(status.success(), "Ranger enforcement test failed");
}
