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
    if repo.starts_with("trinodb/") || repo.contains("/trinodb/") {
        digits(tag)
    } else {
        let parts: Vec<&str> = tag.split('.').collect();
        parts.len() == 3 && parts.iter().all(|p| digits(p))
    }
}

/// An engine image ref with Docker Hub's own `docker.io/` prefix removed, or
/// `None` when `image` names no Trino or StarRocks repository. Any OTHER
/// registry prefix is kept (a mirror is still an engine image and must pin).
fn engine_image(image: &str) -> Option<&str> {
    let i = image.trim().trim_matches(|c| c == '"' || c == '\'');
    let i = i.strip_prefix("docker.io/").unwrap_or(i);
    let named = |repo: &str| i.starts_with(&format!("{repo}/")) || i.contains(&format!("/{repo}/"));
    (named("trinodb") || named("starrocks")).then_some(i)
}

/// Every literal Trino/StarRocks image ref in TS/TSX source. A ref that ends in
/// an interpolation (`trinodb/trino:${v}`, `trinodb/trino:{version}`) is built
/// from a variable, not a literal default, and is skipped.
fn literal_engine_refs(src: &str) -> Vec<String> {
    let ref_char = |c: char| c.is_ascii_alphanumeric() || "._/:@-".contains(c);
    let mut out = Vec::new();
    for needle in ["trinodb/", "starrocks/"] {
        let mut from = 0;
        while let Some(off) = src[from..].find(needle) {
            let mut start = from + off;
            // Take in any registry prefix (`docker.io/`, a mirror host).
            while start > 0 && ref_char(src[..start].chars().next_back().unwrap()) {
                start -= src[..start].chars().next_back().unwrap().len_utf8();
            }
            let end = src[start..]
                .find(|c: char| !ref_char(c))
                .map_or(src.len(), |n| start + n);
            from = end.max(from + off + needle.len());
            let r = &src[start..end];
            let interpolated = r.ends_with(':') && src[end..].starts_with(['$', '{']);
            // `a/starrocks/b` inside a longer path matches both needles' scan
            // at the same start; collect each ref once.
            if !interpolated && !out.iter().any(|o: &String| o == r) {
                out.push(r.to_string());
            }
        }
    }
    out
}

fn frontend_sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            // Generated client and tests may quote any image as data.
            if name != "generated" && name != "__tests__" {
                frontend_sources(&path, out);
            }
        } else if (name.ends_with(".ts") || name.ends_with(".tsx"))
            && !name.contains(".test.")
            && !name.contains(".spec.")
        {
            out.push(path);
        }
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
            if let Some(i) = l.trim().strip_prefix("image:").and_then(engine_image) {
                engine_images.push((f.display().to_string(), i.to_string()));
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

#[test]
fn a_hub_prefixed_or_mirrored_engine_image_is_still_collected() {
    assert_eq!(
        engine_image(" docker.io/starrocks/fe-ubuntu:4.1-latest"),
        Some("starrocks/fe-ubuntu:4.1-latest")
    );
    assert_eq!(
        engine_image("\"docker.io/trinodb/trino:latest\""),
        Some("trinodb/trino:latest")
    );
    assert_eq!(
        engine_image("mirror.example.com/starrocks/fe-ubuntu:4.1-latest"),
        Some("mirror.example.com/starrocks/fe-ubuntu:4.1-latest")
    );
    assert_eq!(engine_image("postgres:16"), None);
    assert!(!pinned("starrocks/fe-ubuntu:4.1-latest"));
}

#[test]
fn the_frontend_scan_sees_every_literal_and_skips_interpolations() {
    let src = r#"
        const a = "starrocks/fe-ubuntu:4.1-latest";
        const b = 'docker.io/trinodb/trino:470';
        const c = `trinodb/trino:${v}`;
        // overrides the default `trinodb/trino:{version}`
        const d = "mirror.corp/starrocks/cn-ubuntu:4.1.6";
    "#;
    assert_eq!(
        literal_engine_refs(src),
        vec![
            "docker.io/trinodb/trino:470".to_string(),
            "starrocks/fe-ubuntu:4.1-latest".to_string(),
            "mirror.corp/starrocks/cn-ubuntu:4.1.6".to_string(),
        ]
    );
}

/// The frontend never carries a floating or unpinned engine image, and the
/// defaults it DISPLAYS are the server's pins (it never sends them: an
/// untouched image field is omitted so `DEFAULT_ENGINE_IMAGES` applies).
#[test]
fn the_frontend_names_only_pinned_engine_images_and_displays_the_server_defaults() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../frontend/src");
    let mut files = Vec::new();
    frontend_sources(&root, &mut files);
    assert!(files.len() > 100, "scanned {} files", files.len());
    let mut refs = Vec::new();
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        for r in literal_engine_refs(&src) {
            let image = engine_image(&r).unwrap_or(&r).to_string();
            refs.push((f.display().to_string(), image));
        }
    }
    for (file, image) in &refs {
        assert!(pinned(image), "{file}: {image}");
    }
    // The count makes an empty scan fail rather than pass: the two displayed
    // StarRocks defaults.
    assert!(refs.len() >= 2, "{refs:?}");

    let d = &egret_perm::engine_images::DEFAULT_ENGINE_IMAGES;
    let shown =
        std::fs::read_to_string(root.join("routes/control-plane/ControlPlaneEngines.tsx")).unwrap();
    for (name, value) in [
        ("DEFAULT_TRINO_IMAGE_VERSION", d.trino_image_version),
        ("DEFAULT_SR_FE_IMAGE", d.starrocks_fe_image),
        ("DEFAULT_SR_CN_IMAGE", d.starrocks_cn_image),
    ] {
        let decl = shown
            .split(&format!("export const {name} ="))
            .nth(1)
            .unwrap_or_else(|| panic!("ControlPlaneEngines.tsx declares {name}"));
        let literal = decl.split(';').next().unwrap().trim();
        assert_eq!(literal, format!("\"{value}\""), "{name}");
    }
}
