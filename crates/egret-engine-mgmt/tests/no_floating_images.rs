//! No egret default and no deploy compose names a floating image tag: a floating
//! tag is how StarRocks 4.1.6 (Ranger 2.9.0) reached engines egret evaluated
//! as 2.8.0. The count makes an empty scan fail rather than pass.
use std::path::Path;

fn floating(image: &str) -> bool {
    let name = image.split('@').next().unwrap();
    match name.rsplit_once(':') {
        Some((_, tag)) => tag == "latest" || tag.ends_with("-latest"),
        None => true,
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
        assert!(!floating(&image) && image.contains("@sha256:"), "{image}");
    }
    let compose = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/docker-compose.dev.yml"),
    )
    .unwrap();
    let engine_images: Vec<&str> = compose
        .lines()
        .filter_map(|l| l.trim().strip_prefix("image:").map(str::trim))
        .filter(|i| i.starts_with("trinodb/") || i.starts_with("starrocks/"))
        .collect();
    assert_eq!(
        engine_images.len(),
        2,
        "expected the Trino and StarRocks services: {engine_images:?}"
    );
    for i in engine_images {
        assert!(!floating(i) && i.contains("@sha256:"), "{i}");
    }
}
