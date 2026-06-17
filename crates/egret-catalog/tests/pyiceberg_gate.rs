//! End-to-end gate: drives a real pyiceberg client against a running Egret catalog.
//! Skips unless EGRET_TEST_E2E=1. Requires: Postgres, MinIO (bucket `warehouse`),
//! the egret server running with [catalog].enabled=true, and python+pyiceberg installed.
use std::process::Command;

#[test]
fn pyiceberg_create_append_load() {
    if std::env::var("EGRET_TEST_E2E").ok().as_deref() != Some("1") {
        eprintln!("EGRET_TEST_E2E != 1 — skipping pyiceberg e2e");
        return;
    }
    let status = Command::new("python3")
        .args(["-m", "pytest", "-q", "tests/pyiceberg/test_catalog_e2e.py"])
        .current_dir(env!("CARGO_MANIFEST_DIR").to_string() + "/../..")
        .status()
        .expect("run pytest");
    assert!(status.success(), "pyiceberg e2e failed");
}
