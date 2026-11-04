//! egret-perm-eval is pure and target-neutral.
#[test]
fn the_evaluator_depends_on_no_database_no_runtime_and_no_target() {
    let manifest = include_str!("../Cargo.toml");
    let deps = manifest.split("[dependencies]").nth(1).unwrap();
    let deps = deps.split("[dev-dependencies]").next().unwrap();
    for forbidden in [
        "sqlx",
        "tokio",
        "egret-policy",
        "egret-ranger-eval",
        "egret-perm-compile",
        "reqwest",
        "axum",
    ] {
        assert!(
            !deps.contains(forbidden),
            "egret-perm-eval must not depend on {forbidden}"
        );
    }
}

#[test]
fn src_does_no_io() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        let text = std::fs::read_to_string(&p).unwrap();
        for word in ["std::fs", "std::net", "std::process", "std::env"] {
            assert!(!text.contains(word), "{p:?} uses {word}");
        }
    }
}
