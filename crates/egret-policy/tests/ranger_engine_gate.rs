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

/// Surefire's per-class summary for the anchors, which is what this gate
/// asserts on. The TOTALS line cannot do the job: `mvn` exits 0 with zero
/// anchors run (measured:
/// `mvn -q test -Dtest=EgretCaseRunner -Dsurefire.failIfNoSpecifiedTests=false`
/// exits 0 and prints nothing at all), so "four anchors passed" and "no anchor
/// ran" are indistinguishable from the exit status. `tests/ranger/pom.xml`
/// names surefire `<includes>` by FILENAME, so renaming the class empties the
/// run silently too.
const ANCHOR_CLASS: &str = "EgretRangerEnforcementTest";
const ANCHORS_RAN: &str = "Tests run: 4, Failures: 0, Errors: 0, Skipped: 0";

#[test]
fn ranger_engine_enforces_egret_policies() {
    if std::env::var("EGRET_TEST_RANGER_E2E").ok().as_deref() != Some("1") {
        eprintln!("EGRET_TEST_RANGER_E2E != 1 — skipping Ranger engine gate");
        return;
    }
    let dir = env!("CARGO_MANIFEST_DIR").to_string() + "/../../tests/ranger";
    // Both versions, because a gate at one version certifies one engine:
    // Trino 483 and StarRocks fe-ubuntu:4.1.4 load 2.8.0, measured on the running
    // pods (`docs/en/plans/2026-09-28-sp4d-plugin-version-result.md`); plugin 2.5.0
    // (Trino 470) is no longer supported, so it is no longer run. fe-ubuntu:4.1.6 loads 2.9.0 (`docs/en/plans/2026-10-13-ranger-2-9-0-reading.md`).
    for version in ["2.8.0", "2.9.0"] {
        // No `-q`: the per-class summary line this gate reads is logged at
        // INFO, and `-q` suppresses it. That is the whole reason the flag is
        // gone.
        let out = Command::new("mvn")
            .args(["test", &format!("-Dranger.version={version}")])
            .current_dir(&dir)
            .output()
            .expect("run mvn");
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "Ranger enforcement test failed at ranger-plugins-common {version}\n{text}"
        );
        // A green exit is not evidence the anchors ran. Require the line that
        // names the class AND the count.
        let ran = text
            .lines()
            .any(|l| l.contains(ANCHOR_CLASS) && l.contains(ANCHORS_RAN));
        assert!(
            ran,
            "mvn exited 0 at ranger-plugins-common {version} but no line said \
             `{ANCHORS_RAN} ... in {ANCHOR_CLASS}`. The four hand-written anchors \
             did NOT run — check `tests/ranger/pom.xml`'s surefire <includes>, \
             which names the class by filename.\n{text}"
        );
    }
}
