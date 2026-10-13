//! No egret default and no deploy compose names a floating image tag: a floating
//! tag is how StarRocks 4.1.6 (Ranger 2.9.0) reached engines egret evaluated
//! as 2.8.0. The count makes an empty scan fail rather than pass.
use std::path::Path;

/// A pinned image: a numeric tag (`483`) for Trino or `N.N.N` for StarRocks, and
/// a `sha256:` digest of 64 hex digits. A positive rule, so a new floating
/// spelling (`stable`, `4.1`, `nightly`) fails too, not just `latest`.
fn pinned(image: &str) -> bool {
    let Some((name, digest)) = image.split_once('@') else {
        return false;
    };
    let hex = digest.strip_prefix("sha256:").unwrap_or("");
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    let Some((repo, tag)) = name.rsplit_once(':') else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if repo.starts_with("trinodb/") {
        digits(tag)
    } else {
        let parts: Vec<&str> = tag.split('.').collect();
        parts.len() == 3 && parts.iter().all(|p| digits(p))
    }
}

fn compose_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            compose_files(&path, out);
        } else if let Some(n) = path.file_name().and_then(|n| n.to_str()) {
            if n.starts_with("docker-compose") && n.ends_with(".yml") {
                out.push(path);
            }
        }
    }
}

#[test]
fn defaults_and_dev_compose_pin_every_engine_image() {
    let d = &egret_perm::engine_images::DEFAULT_ENGINE_IMAGES;
    for image in [
        egret_perm::engine_images::trino_image_ref(d.trino_image_version),
        d.starrocks_fe_image.to_string(),
        d.starrocks_cn_image.to_string(),
    ] {
        assert!(pinned(&image), "{image}");
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    compose_files(&root.join("deploy"), &mut files);
    compose_files(&root.join("crates/egret-workflow/oracle-exec"), &mut files);
    let mut engine_images = Vec::new();
    for f in &files {
        for l in std::fs::read_to_string(f).unwrap().lines() {
            if let Some(i) = l.trim().strip_prefix("image:").map(str::trim) {
                if i.starts_with("trinodb/") || i.starts_with("starrocks/") {
                    engine_images.push((f.display().to_string(), i.to_string()));
                }
            }
        }
    }
    // dev compose (Trino, StarRocks) + the w5b oracle (Trino, StarRocks)
    assert!(
        engine_images.len() >= 4,
        "expected at least the dev and w5b engine services: {engine_images:?}"
    );
    for (file, image) in &engine_images {
        assert!(pinned(image), "{file}: {image}");
    }
}

#[test]
fn the_pin_rule_rejects_every_floating_spelling() {
    let d = "@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534";
    for ok in [
        format!("trinodb/trino:483{d}"),
        format!("starrocks/allin1-ubuntu:4.1.6{d}"),
    ] {
        assert!(pinned(&ok), "{ok}");
    }
    for bad in [
        "trinodb/trino:latest".to_string(),
        "trinodb/trino:483".to_string(),
        format!("trinodb/trino:stable{d}"),
        format!("starrocks/allin1-ubuntu:4.1-latest{d}"),
        format!("starrocks/allin1-ubuntu:4.1{d}"),
        "starrocks/fe-ubuntu:4.1.6@sha256:abc".to_string(),
    ] {
        assert!(!pinned(&bad), "{bad}");
    }
}
