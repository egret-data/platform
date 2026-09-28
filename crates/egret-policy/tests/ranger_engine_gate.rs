//! Real Ranger policy-engine enforcement gate: the four HAND-WRITTEN anchor
//! cases, with known-good expected values, against a real
//! `RangerPolicyEngineImpl`. Skips unless EGRET_TEST_RANGER_E2E=1.
//!
//! This is not the differential gate. `mvn test` with no `-Degret.cases`
//! leaves `EgretCaseRunner` skipped (`Assume`), so what runs here is
//! `EgretRangerEnforcementTest` alone — allow, deny, mask, row filter, each
//! asserting a value someone worked out and wrote down. Generated cases assert
//! only AGREEMENT, which is the weaker claim: two implementations can agree and
//! both be wrong. The generated corpus lives in
//! `crates/egret-ranger-eval/tests/differential.rs` and drives the SAME Maven
//! project, so a failure there may also be one of these four.
//!
//! Requires: a JVM + Maven + the ranger-plugins-common jar (Maven Central, or
//! a warm `~/.m2`).
use std::process::Command;

#[test]
fn ranger_engine_enforces_egret_policies() {
    if std::env::var("EGRET_TEST_RANGER_E2E").ok().as_deref() != Some("1") {
        eprintln!("EGRET_TEST_RANGER_E2E != 1 — skipping Ranger engine gate");
        return;
    }
    let dir = env!("CARGO_MANIFEST_DIR").to_string() + "/../../tests/ranger";
    // Both versions, because a gate at one version certifies one engine:
    // Trino 470 loads 2.5.0 and StarRocks fe-ubuntu:4.1 loads 2.8.0, measured
    // on the running pods (`docs/en/plans/2026-09-28-sp4d-plugin-version-result.md`).
    for version in ["2.5.0", "2.8.0"] {
        let status = Command::new("mvn")
            .args(["-q", "test", &format!("-Dranger.version={version}")])
            .current_dir(&dir)
            .status()
            .expect("run mvn");
        assert!(
            status.success(),
            "Ranger enforcement test failed at ranger-plugins-common {version}"
        );
    }
}
