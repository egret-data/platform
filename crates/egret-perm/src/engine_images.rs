//! Which Ranger plugin an engine image ships, by EXACT image. The only place
//! egret maps an engine to a plugin version: a provisioned engine through its
//! stored image, never through its type alone. `starrocks/fe-ubuntu:4.1-latest`
//! shipped 2.8.0 on 2026-09-28 and 2.9.0 from 2026-10-01, and a type-only map
//! cannot see that.

pub struct KnownEngineImage {
    pub engine_type: &'static str,
    pub repository: &'static str,
    pub tag: &'static str,
    pub digest: &'static str,
    pub plugin_version: &'static str,
}

/// Measured: each row's plugin jar was read out of the image itself.
pub const KNOWN_ENGINE_IMAGES: [KnownEngineImage; 4] = [
    KnownEngineImage {
        engine_type: "trino",
        repository: "trinodb/trino",
        tag: "470",
        digest: "sha256:a591436ddc20cef15a4424704423326828591727b0e7c40c6ea4165d4564df67",
        plugin_version: "2.5.0",
    },
    KnownEngineImage {
        engine_type: "trino",
        repository: "trinodb/trino",
        tag: "483",
        digest: "sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534",
        plugin_version: "2.8.0",
    },
    KnownEngineImage {
        engine_type: "starrocks",
        repository: "starrocks/fe-ubuntu",
        tag: "4.1.4",
        digest: "sha256:0b9099483e52ee66f9ee0244799bd32f3ccb491492857c59ada622ceb15b914b",
        plugin_version: "2.8.0",
    },
    KnownEngineImage {
        engine_type: "starrocks",
        repository: "starrocks/fe-ubuntu",
        tag: "4.1.6",
        digest: "sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31",
        plugin_version: "2.9.0",
    },
];

pub struct DefaultEngineImages {
    pub trino_image_version: &'static str,
    pub starrocks_fe_image: &'static str,
    pub starrocks_cn_image: &'static str,
}

pub const DEFAULT_ENGINE_IMAGES: DefaultEngineImages = DefaultEngineImages {
    trino_image_version: "483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534",
    starrocks_fe_image: "starrocks/fe-ubuntu:4.1.6@sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31",
    starrocks_cn_image: "starrocks/cn-ubuntu:4.1.6@sha256:bf0f5ca7fd9fc4e568d4d0e593e362f229b376976b513a5a549b1b922d9ec23b",
};

pub fn trino_image_ref(image_version: &str) -> String {
    format!("trinodb/trino:{image_version}")
}

/// A non-blank string at `v`, trimmed.
fn non_blank(v: Option<&serde_json::Value>) -> Option<&str> {
    v?.as_str().map(str::trim).filter(|s| !s.is_empty())
}

/// The image a Trino spec overrides `imageVersion` with. Mirrors the
/// provisioner: `trinoConfig.image` is read only from the per-role shape
/// (`trinoConfig.coordinator` an object) and only when non-blank; otherwise the
/// engine runs `trinodb/trino:{imageVersion}`.
fn trino_image_override(spec: &serde_json::Value) -> Option<&str> {
    let tc = spec.get("trinoConfig")?;
    if !tc.get("coordinator").is_some_and(|c| c.is_object()) {
        return None;
    }
    non_blank(tc.get("image"))
}

/// The full image ref an engine's stored spec runs, or `None` when the spec
/// carries no image for this engine type (absent or blank). For Trino a
/// `trinoConfig.image` override wins over `imageVersion`, as it does in the
/// provisioner, and is returned as written (trimmed) for the exact-image table
/// to judge.
pub fn stored_engine_image(engine_type: &str, spec: &serde_json::Value) -> Option<String> {
    if engine_type == "trino" {
        if let Some(image) = trino_image_override(spec) {
            return Some(image.to_string());
        }
    }
    let raw = match engine_type {
        "trino" => spec.get("imageVersion")?.as_str()?.trim().to_string(),
        "starrocks" => spec
            .get("starrocksConfig")?
            .get("feImage")?
            .as_str()?
            .trim()
            .to_string(),
        _ => return None,
    };
    if raw.is_empty() {
        return None;
    }
    Some(if engine_type == "trino" {
        trino_image_ref(&raw)
    } else {
        raw
    })
}

/// `repo[:tag][@digest]` to (repository, tag, digest). `docker.io/` is Docker
/// Hub's own name and is stripped; any other registry prefix stays part of the
/// repository, so a mirror never matches a Hub row.
fn parse(image_ref: &str) -> (&str, Option<&str>, Option<&str>) {
    let (name, digest) = match image_ref.split_once('@') {
        Some((n, d)) => (n, Some(d)),
        None => (image_ref, None),
    };
    let name = name.strip_prefix("docker.io/").unwrap_or(name);
    match name.rsplit_once(':') {
        Some((repo, tag)) if !tag.contains('/') => (repo, Some(tag), digest),
        _ => (name, None, digest),
    }
}

/// `repository:tag` of every known image of `engine_type`, in table order: what
/// an operator may pin such an engine to.
pub fn known_image_refs(engine_type: &str) -> Vec<String> {
    KNOWN_ENGINE_IMAGES
        .iter()
        .filter(|k| k.engine_type == engine_type)
        .map(|k| format!("{}:{}", k.repository, k.tag))
        .collect()
}

pub fn plugin_version_for_image(
    engine_type: &str,
    image_ref: &str,
) -> Result<&'static str, String> {
    let (repo, tag, digest) = parse(image_ref.trim());
    let hit = KNOWN_ENGINE_IMAGES.iter().find(|k| {
        k.engine_type == engine_type
            && k.repository == repo
            && Some(k.tag) == tag
            && digest.is_none_or(|d| d == k.digest)
    });
    match hit {
        Some(k) => Ok(k.plugin_version),
        None => {
            let known = known_image_refs(engine_type);
            Err(format!(
                "image \"{image_ref}\" is not one whose Ranger plugin egret has measured for a \
                 {engine_type} engine; pin the engine to one of: {}",
                known.join(", ")
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_known_image_resolves_with_and_without_its_digest() {
        for k in KNOWN_ENGINE_IMAGES.iter() {
            let bare = format!("{}:{}", k.repository, k.tag);
            let pinned = format!("{bare}@{}", k.digest);
            assert_eq!(
                plugin_version_for_image(k.engine_type, &bare),
                Ok(k.plugin_version),
                "{bare}"
            );
            assert_eq!(
                plugin_version_for_image(k.engine_type, &pinned),
                Ok(k.plugin_version),
                "{pinned}"
            );
            let hub = format!("docker.io/{pinned}");
            assert_eq!(
                plugin_version_for_image(k.engine_type, &hub),
                Ok(k.plugin_version),
                "{hub}"
            );
        }
    }

    #[test]
    fn the_table_is_the_measured_one() {
        let rows: Vec<_> = KNOWN_ENGINE_IMAGES
            .iter()
            .map(|k| (k.engine_type, k.repository, k.tag, k.plugin_version))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("trino", "trinodb/trino", "470", "2.5.0"),
                ("trino", "trinodb/trino", "483", "2.8.0"),
                ("starrocks", "starrocks/fe-ubuntu", "4.1.4", "2.8.0"),
                ("starrocks", "starrocks/fe-ubuntu", "4.1.6", "2.9.0"),
            ]
        );
    }

    #[test]
    fn anything_not_in_the_table_is_unmodelled_and_says_how_to_fix_it() {
        for (t, image) in [
            ("starrocks", "starrocks/fe-ubuntu:4.1-latest"),
            ("starrocks", "starrocks/fe-ubuntu:latest"),
            ("starrocks", "starrocks/fe-ubuntu"),
            ("starrocks", "starrocks/fe-ubuntu:4.1.5"),
            ("starrocks", "mirror.example.com/starrocks/fe-ubuntu:4.1.6"),
            ("starrocks", "starrocks/fe-ubuntu:4.1.6@sha256:0000000000000000000000000000000000000000000000000000000000000000"),
            ("trino", "trinodb/trino:latest"),
            ("trino", "trinodb/trino:484"),
            // a Trino image presented for a StarRocks engine is not a StarRocks image
            ("starrocks", "trinodb/trino:483"),
        ] {
            let err = plugin_version_for_image(t, image).unwrap_err();
            assert!(err.contains(image), "names the image: {err}");
            assert!(err.contains("pin"), "names the fix: {err}");
            for k in KNOWN_ENGINE_IMAGES.iter().filter(|k| k.engine_type == t) {
                assert!(err.contains(&format!("{}:{}", k.repository, k.tag)), "lists {}: {err}", k.tag);
            }
        }
    }

    #[test]
    fn stored_image_reads_each_engine_types_own_field() {
        assert_eq!(
            stored_engine_image("trino", &json!({"imageVersion": "470"})),
            Some("trinodb/trino:470".into())
        );
        assert_eq!(
            stored_engine_image(
                "starrocks",
                &json!({"starrocksConfig": {"feImage": "starrocks/fe-ubuntu:4.1.4"}})
            ),
            Some("starrocks/fe-ubuntu:4.1.4".into())
        );
        assert_eq!(stored_engine_image("trino", &json!({})), None);
        assert_eq!(
            stored_engine_image("trino", &json!({"imageVersion": "  "})),
            None
        );
        assert_eq!(
            stored_engine_image("starrocks", &json!({"imageVersion": "x"})),
            None
        );
        assert_eq!(
            stored_engine_image("risingwave", &json!({"imageVersion": "1"})),
            None
        );
    }

    fn trino_spec_with_image(image: &str) -> serde_json::Value {
        json!({
            "imageVersion": "483",
            "trinoConfig": {"coordinator": {"replicas": 1}, "image": image}
        })
    }

    fn trino_report(spec: &serde_json::Value) -> Result<&'static str, String> {
        let image = stored_engine_image("trino", spec).expect("an image");
        plugin_version_for_image("trino", &image)
    }

    #[test]
    fn a_trino_image_override_is_the_image_that_runs() {
        // A custom image is reported as itself, so it is unmodelled and the
        // note names it — never the plugin of `imageVersion`.
        let custom = trino_spec_with_image("registry.corp/trino-custom:x");
        assert_eq!(
            stored_engine_image("trino", &custom),
            Some("registry.corp/trino-custom:x".into())
        );
        let err = trino_report(&custom).unwrap_err();
        assert!(err.contains("registry.corp/trino-custom:x"), "{err}");

        // The override is trimmed and the exact-image table decides.
        assert_eq!(
            trino_report(&trino_spec_with_image("  trinodb/trino:483  ")),
            Ok("2.8.0")
        );
        let wrong = "trinodb/trino:483@sha256:0000000000000000000000000000000000000000000000000000000000000000";
        assert!(trino_report(&trino_spec_with_image(wrong)).is_err());

        // A blank override falls back to `imageVersion`, as the provisioner does.
        for blank in ["", "   "] {
            assert_eq!(
                stored_engine_image("trino", &trino_spec_with_image(blank)),
                Some("trinodb/trino:483".into()),
                "{blank:?}"
            );
        }
        // A non-string override is not an image either.
        assert_eq!(
            stored_engine_image(
                "trino",
                &json!({"imageVersion": "483", "trinoConfig": {"coordinator": {}, "image": 7}})
            ),
            Some("trinodb/trino:483".into())
        );
        // An override with no imageVersion still names the image that runs.
        assert_eq!(
            stored_engine_image(
                "trino",
                &json!({"trinoConfig": {"coordinator": {}, "image": "trinodb/trino:470"}})
            ),
            Some("trinodb/trino:470".into())
        );
    }

    #[test]
    fn a_legacy_flat_trino_spec_ignores_the_override_like_the_provisioner() {
        // The provisioner reads `trinoConfig.image` only from the per-role shape
        // (`trinoConfig.coordinator` an object); the legacy flat spec runs
        // `trinodb/trino:{imageVersion}` whatever `trinoConfig.image` says.
        assert_eq!(
            stored_engine_image(
                "trino",
                &json!({"imageVersion": "483", "trinoConfig": {"image": "registry.corp/x:1"}})
            ),
            Some("trinodb/trino:483".into())
        );
    }

    /// Every image egret knows ships a plugin its evaluator models: a known
    /// image whose pair is not modelled would be reported unmodelled for an
    /// engine pinned exactly as the guide says.
    #[test]
    fn every_known_images_plugin_is_a_modelled_pair() {
        for k in KNOWN_ENGINE_IMAGES.iter() {
            assert!(
                crate::MODELLED_RANGER_PLUGINS
                    .iter()
                    .any(|(t, v)| *t == k.engine_type && *v == k.plugin_version),
                "{}:{} ships {} {}, not in MODELLED_RANGER_PLUGINS",
                k.repository,
                k.tag,
                k.engine_type,
                k.plugin_version
            );
        }
    }

    #[test]
    fn the_defaults_are_pinned_known_images() {
        let d = &DEFAULT_ENGINE_IMAGES;
        assert_eq!(
            plugin_version_for_image("trino", &trino_image_ref(d.trino_image_version)),
            Ok("2.8.0")
        );
        assert_eq!(
            plugin_version_for_image("starrocks", d.starrocks_fe_image),
            Ok("2.9.0")
        );
        for image in [
            trino_image_ref(d.trino_image_version),
            d.starrocks_fe_image.into(),
            d.starrocks_cn_image.into(),
        ] {
            assert!(image.contains("@sha256:"), "{image} carries its digest");
        }
    }

    #[test]
    fn every_digest_is_the_measured_literal() {
        let rows: Vec<_> = KNOWN_ENGINE_IMAGES
            .iter()
            .map(|k| (k.repository, k.tag, k.digest))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "trinodb/trino",
                    "470",
                    "sha256:a591436ddc20cef15a4424704423326828591727b0e7c40c6ea4165d4564df67"
                ),
                (
                    "trinodb/trino",
                    "483",
                    "sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534"
                ),
                (
                    "starrocks/fe-ubuntu",
                    "4.1.4",
                    "sha256:0b9099483e52ee66f9ee0244799bd32f3ccb491492857c59ada622ceb15b914b"
                ),
                (
                    "starrocks/fe-ubuntu",
                    "4.1.6",
                    "sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31"
                ),
            ]
        );
        assert_eq!(
            DEFAULT_ENGINE_IMAGES.starrocks_cn_image,
            "starrocks/cn-ubuntu:4.1.6@sha256:bf0f5ca7fd9fc4e568d4d0e593e362f229b376976b513a5a549b1b922d9ec23b"
        );
    }

    #[test]
    fn near_misses_of_a_known_image_are_unmodelled() {
        let sr_digest = "sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31";
        for (t, image) in [
            ("starrocks", "localhost:5000/starrocks/fe-ubuntu:4.1.6"),
            ("starrocks", &format!("starrocks/fe-ubuntu@{sr_digest}")),
            ("starrocks", "STARROCKS/FE-UBUNTU:4.1.6"),
            ("trino", "docker.io/library/trinodb/trino:483"),
            ("trino", ""),
            ("starrocks", ""),
        ] {
            assert!(plugin_version_for_image(t, image).is_err(), "{t} {image:?}");
        }
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(
            plugin_version_for_image("trino", "  trinodb/trino:483  "),
            Ok("2.8.0")
        );
    }
}
