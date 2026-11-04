//! The JVM half of the Ranger differential: drives `tests/ranger` (Maven +
//! the real Ranger jars) and asserts the surefire summary. Shared by the
//! differential binary and the OPA-vs-Ranger differential.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

pub fn ranger_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/ranger"))
}

/// [`ask_ranger`] over already-serialised cases (SP4d-4 Task 1), for a request
/// [`Case::to_json`] cannot express — `resourceMatchingScope`, and the `_any`
/// pseudo access type this crate refuses. Every assertion `ask_ranger` makes
/// lives here, so both paths carry them.
pub fn ask_ranger_json(payload: Vec<Value>, ranger_version: &str, tag: &str) -> Vec<Value> {
    let dir = ranger_dir();
    // The scratch path carries the pid: several agents share one temp
    // directory and an unprefixed name is another run's file.
    let work = std::env::temp_dir().join(format!(
        "egret-ranger-diff-{ranger_version}-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&work).expect("create work dir");
    let cases_path = work.join("cases.json");
    let verdicts_path = work.join("verdicts.json");
    // A stale verdicts file from an earlier run would be read as this run's
    // answer if Maven failed to write one.
    let _ = std::fs::remove_file(&verdicts_path);

    let sent = payload.len();
    let service_type = payload
        .first()
        .and_then(|c| c.get("serviceType"))
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    std::fs::write(
        &cases_path,
        serde_json::to_vec(&Value::Array(payload)).expect("serialise cases"),
    )
    .expect("write cases.json");

    eprintln!(
        "differential: {} {tag} cases at ranger {ranger_version} ({service_type}); cases={}",
        sent,
        cases_path.display()
    );

    // ONE `mvn test` at a time in `tests/ranger`. Since SP4d-3 Task 8 two
    // tests in this binary drive the harness, and cargo runs them on parallel
    // threads; two Mavens compiling into the same `target/` race each other.
    // A poisoned lock only means another test panicked, which that test
    // already reports.
    static MAVEN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one_maven = MAVEN.lock().unwrap_or_else(|e| e.into_inner());
    // No `-q`: the per-class surefire summary this asserts on is logged at
    // INFO, and `-q` suppresses it.
    let out = Command::new("mvn")
        .args([
            "test".to_string(),
            format!("-Dranger.version={ranger_version}"),
            format!("-Degret.cases={}", cases_path.display()),
            format!("-Degret.verdicts={}", verdicts_path.display()),
        ])
        .current_dir(&dir)
        .output()
        .expect("run mvn");
    let log = String::from_utf8_lossy(&out.stdout).into_owned()
        + "\n--- stderr ---\n"
        + &String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "mvn failed at ranger {ranger_version} (this run also executes the four hand-written \
         anchor tests, so a failure here may be one of those):\n{log}"
    );
    // A green exit is not evidence the anchors ran; see this function's docs.
    assert!(
        log.lines().any(|l| l.contains("EgretRangerEnforcementTest")
            && l.contains("Tests run: 4, Failures: 0, Errors: 0, Skipped: 0")),
        "mvn exited 0 at ranger {ranger_version} but no line said `Tests run: 4, Failures: 0, \
         Errors: 0, Skipped: 0 ... in EgretRangerEnforcementTest`. The four hand-written anchors \
         did NOT run, so this harness is asserting agreement and nothing else — check \
         `tests/ranger/pom.xml`'s surefire <includes>.\n{log}"
    );

    let verdicts: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(&verdicts_path).expect("read verdicts.json"))
            .expect("parse verdicts.json");
    assert_eq!(
        verdicts.len(),
        sent,
        "ranger {ranger_version} answered {} of {} cases",
        verdicts.len(),
        sent
    );
    verdicts
}
