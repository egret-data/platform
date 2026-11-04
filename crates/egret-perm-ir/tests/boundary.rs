//! The IR is target-neutral: no Ranger dependency, no Ranger vocabulary, no I/O.
#[test]
fn the_ir_depends_on_no_target_and_no_database() {
    let manifest = include_str!("../Cargo.toml");
    for forbidden in [
        "egret-policy",
        "egret-policy-api",
        "egret-ranger-eval",
        "sqlx",
        "tokio",
    ] {
        assert!(
            !manifest.contains(forbidden),
            "egret-perm-ir must not depend on {forbidden}"
        );
    }
}

#[test]
fn no_public_ranger_vocabulary() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap();
            for line in text.lines().filter(|l| l.trim_start().starts_with("pub ")) {
                for word in [
                    "Ranger",
                    "Policy",
                    "policyItems",
                    "tagService",
                    "serviceType",
                ] {
                    if word == "Ranger" && line.contains("Mechanism") {
                        continue;
                    }
                    assert!(
                        !line.contains(word),
                        "{p:?}: public item uses {word:?}: {line}"
                    );
                }
            }
        }
    }
}
