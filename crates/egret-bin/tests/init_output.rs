//! KI-146: what `egret init` prints, read off the real binary's stdout and
//! stderr against this process's private test database.

use std::process::{Command, Output};

use uuid::Uuid;

/// One `init` at a time: two at once race to create the `default` workspace
/// (`get_by_slug`, then `create`) and the loser fails.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn init(db_url: &str, email: &str, password: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_egret"));
    cmd.args(["init", "--force"])
        .env("EGRET_DATABASE__URL", db_url)
        // `init` never touches storage; `Config::validate` requires it.
        .env("EGRET_STORAGE__BUCKET", "ki146")
        .env("EGRET_STORAGE__ACCESS_KEY", "ki146")
        .env("EGRET_STORAGE__SECRET_KEY", "ki146")
        .env("EGRET_ADMIN_EMAIL", email)
        .env_remove("EGRET_ADMIN_PASSWORD");
    if let Some(p) = password {
        cmd.env("EGRET_ADMIN_PASSWORD", p);
    }
    let out = cmd.output().expect("run egret init");
    assert!(
        out.status.success(),
        "egret init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn printed(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn email() -> String {
    format!("ki146-{}@egret.local", Uuid::new_v4().simple())
}

#[tokio::test]
async fn init_never_prints_a_password_taken_from_the_environment() {
    let _serial = SERIAL.lock().await;
    let Some(url) = egret_test_db::url().await else {
        return;
    };
    let supplied = format!("Ki146-{}", Uuid::new_v4().simple());

    let out = printed(&init(&url, &email(), Some(&supplied)));

    assert!(
        !out.contains(&supplied),
        "the supplied password was printed: {out}"
    );
    assert!(
        out.contains("taken from EGRET_ADMIN_PASSWORD"),
        "says where the password came from: {out}"
    );
}

#[tokio::test]
async fn init_prints_a_generated_password_once() {
    let _serial = SERIAL.lock().await;
    let Some(url) = egret_test_db::url().await else {
        return;
    };

    let stdout = String::from_utf8_lossy(&init(&url, &email(), None).stdout).into_owned();

    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix("Password: "))
        .unwrap_or_else(|| panic!("no password line: {stdout}"));
    assert!(line.len() >= 16, "a generated password: {line}");
    assert_eq!(stdout.matches(line).count(), 1, "{stdout}");
}
