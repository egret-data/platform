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
    /// Whether the image ships Trino's OPA access-control plugin
    /// (`plugin/opa`), which governance SP2b's OPA mechanism needs.
    pub opa_plugin: bool,
}

/// Measured: each row's plugin jar was read out of the image itself.
pub const KNOWN_ENGINE_IMAGES: [KnownEngineImage; 6] = [
    KnownEngineImage {
        engine_type: "trino",
        repository: "trinodb/trino",
        tag: "483",
        digest: "sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534",
        plugin_version: "2.8.0",
        opa_plugin: true,
    },
    KnownEngineImage {
        engine_type: "starrocks",
        repository: "starrocks/fe-ubuntu",
        tag: "4.1.4",
        digest: "sha256:0b9099483e52ee66f9ee0244799bd32f3ccb491492857c59ada622ceb15b914b",
        plugin_version: "2.8.0",
        opa_plugin: false,
    },
    KnownEngineImage {
        engine_type: "starrocks",
        repository: "starrocks/fe-ubuntu",
        tag: "4.1.6",
        digest: "sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31",
        plugin_version: "2.9.0",
        opa_plugin: false,
    },
    // StarRocks 4.1.7 with egret's security patches (github.com/egret-data/egret-starrocks),
    // the official 4.1.7 FE image with fe-core and fe-spi rebuilt, so it ships the official
    // plugin jars. egret.1 (patches 0001-0002): Iceberg metadata tables need SELECT on the base
    // table and are refused under a mask or row filter; masks and row filters apply to prepared
    // statements. Kept so engines already on it stay modelled.
    KnownEngineImage {
        engine_type: "starrocks",
        repository: "ghcr.io/egret-data/egret-starrocks-fe",
        tag: "4.1.7-egret.1",
        digest: "sha256:da948dad19e98a5a43b0073ec48f87c9e7429246ce4581a000c0955c742af250",
        plugin_version: "2.9.0",
        opa_plugin: false,
    },
    // egret.3 (patches 0001-0007): also `_file`/`_pos` stay out of `SELECT *` under a policy;
    // the HTTP SQL API and Arrow Flight SQL apply masks and row filters; SHOW CREATE CATALOG
    // needs a privilege on the catalog; FILES() needs OPERATE on SYSTEM; no materialized view
    // over a table under its creator's mask or row filter.
    KnownEngineImage {
        engine_type: "starrocks",
        repository: "ghcr.io/egret-data/egret-starrocks-fe",
        tag: "4.1.7-egret.3",
        digest: "sha256:5c74c820f9b741bc8054247f47f3d54dd9e526710b6fc2b5a7873cae6bb2d2b0",
        plugin_version: "2.9.0",
        opa_plugin: false,
    },
    // egret.4 (patches 0001-0009 and the AuditLoader plugin): also EXPLAIN COSTS is refused
    // under a mask or row filter; BROKER LOAD, EXPORT, INTO OUTFILE and FILE tables without
    // credentials of their own need OPERATE on SYSTEM.
    KnownEngineImage {
        engine_type: "starrocks",
        repository: "ghcr.io/egret-data/egret-starrocks-fe",
        tag: "4.1.7-egret.4",
        digest: "sha256:55ec35ecf6df85320557f017e0c3c16056d82a258cf4a4e79a80a6565ac6fdc5",
        plugin_version: "2.9.0",
        opa_plugin: false,
    },
];

pub struct DefaultEngineImages {
    pub trino_image_version: &'static str,
    pub starrocks_fe_image: &'static str,
    pub starrocks_cn_image: &'static str,
}

pub const DEFAULT_ENGINE_IMAGES: DefaultEngineImages = DefaultEngineImages {
    trino_image_version: "483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534",
    // The patched FE (KNOWN_ENGINE_IMAGES); no patch touches the CN, which stays the official
    // image of the same release.
    starrocks_fe_image: "ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4@sha256:55ec35ecf6df85320557f017e0c3c16056d82a258cf4a4e79a80a6565ac6fdc5",
    starrocks_cn_image: "starrocks/cn-ubuntu:4.1.7@sha256:39f7320127c9924abb1f1ce0ba0583ab11989cbe5bd1bb6274ada6f89ba5740d",
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

/// The lowest Ranger plugin release egret supports. Below it a plugin cannot
/// send the per-engine credential the Ranger surface requires (measured, spec
/// section 2).
pub const MIN_SUPPORTED_RANGER_PLUGIN: &str = "2.8.0";

/// Images egret used to run and no longer supports: (engine type, repository,
/// tag, the plugin release they ship). Not in [`KNOWN_ENGINE_IMAGES`], so they
/// read as unmeasured; kept here so creating or updating an engine onto one is
/// refused with the reason rather than accepted as an anonymous custom image.
pub const DROPPED_ENGINE_IMAGES: [(&str, &str, &str, &str); 1] =
    [("trino", "trinodb/trino", "470", "2.5.0")];

/// `Some(refusal)` when `image_ref` is an image egret dropped support for.
pub fn unsupported_engine_image(engine_type: &str, image_ref: &str) -> Option<String> {
    let (repo, tag, _digest) = parse(image_ref.trim());
    let (_, _, _, plugin) = DROPPED_ENGINE_IMAGES
        .iter()
        .find(|(t, r, g, _)| *t == engine_type && *r == repo && Some(*g) == tag)?;
    Some(format!(
        "image \"{image_ref}\" ships Ranger plugin {plugin}, which egret no longer supports: \
         the floor is {MIN_SUPPORTED_RANGER_PLUGIN}; use one of: {}",
        known_image_refs(engine_type).join(", ")
    ))
}

/// Whether `image_ref` is a known `engine_type` image that ships Trino's OPA
/// plugin, matched by repository and tag; a digest, when present, must be the
/// row's. Anything unmeasured is `false`.
pub fn ships_opa_plugin(engine_type: &str, image_ref: &str) -> bool {
    let (repo, tag, digest) = parse(image_ref.trim());
    KNOWN_ENGINE_IMAGES.iter().any(|k| {
        k.opa_plugin
            && k.engine_type == engine_type
            && k.repository == repo
            && Some(k.tag) == tag
            && digest.is_none_or(|d| d == k.digest)
    })
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

/// The egret FE repository every patched StarRocks image is published under.
const EGRET_STARROCKS_FE_REPOSITORY: &str = "ghcr.io/egret-data/egret-starrocks-fe";

/// The first patch level of an upstream release that closes every hole the StarRocks warning
/// names (egret-starrocks patches 0001 to 0009 on 4.1.7, egret.4): `((a, b, c), n)`. A later
/// upstream release starts at level 1 with every patch rebased onto it, so any level counts.
pub const FIRST_COMPLETE_STARROCKS_FE: ((u32, u32, u32), u32) = ((4, 1, 7), 4);

/// `a.b.c` to its three numbers; anything else is `None`.
fn parse_release(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.split('.');
    let a = parts.next()?.parse().ok()?;
    let b = parts.next()?.parse().ok()?;
    let c = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((a, b, c))
}

/// `<a>.<b>.<c>-egret.<n>` to ((a, b, c), n).
fn parse_egret_tag(tag: &str) -> Option<((u32, u32, u32), u32)> {
    let (release, level) = tag.split_once("-egret.")?;
    Some((parse_release(release)?, level.parse().ok()?))
}

/// Whether a (release, level) is at or after [`FIRST_COMPLETE_STARROCKS_FE`].
fn is_complete_level(release: (u32, u32, u32), level: u32) -> bool {
    let (first_release, first_level) = FIRST_COMPLETE_STARROCKS_FE;
    release > first_release || (release == first_release && level >= first_level)
}

/// Whether `image` (a stored FE image ref, tag and/or digest) is an egret FE that closes every
/// hole the StarRocks warning names: by digest, the digest of a known egret FE row at or after
/// the first complete level (so a mirrored repository counts); by tag, an egret FE tag at or
/// after it, read only when the ref carries no digest (a ref with a digest is judged by the
/// digest alone, whatever its tag says). Anything else, including an image egret does not know,
/// is unpatched.
pub fn is_patched_starrocks_fe(image: &str) -> bool {
    let (repo, tag, digest) = parse(image.trim());
    let by_digest = digest.is_some_and(|d| {
        KNOWN_ENGINE_IMAGES.iter().any(|k| {
            k.repository == EGRET_STARROCKS_FE_REPOSITORY
                && k.digest == d
                && parse_egret_tag(k.tag).is_some_and(|(r, n)| is_complete_level(r, n))
        })
    });
    let by_tag = repo == EGRET_STARROCKS_FE_REPOSITORY
        && tag
            .and_then(parse_egret_tag)
            .is_some_and(|(r, n)| is_complete_level(r, n));
    // A digest is what the node pulls, so it alone decides; the tag counts only without one.
    digest.map_or(by_tag, |_| by_digest)
}

/// The release `<a>.<b>.<c>` of an image's tag (`4.1.6`, `4.1.7-egret.3`); a tag such as
/// `4.1-latest`, or no tag, has none.
fn image_release(image: &str) -> Option<(u32, u32, u32)> {
    let (_, tag, _) = parse(image.trim());
    let tag = tag?;
    parse_release(tag.split_once("-egret.").map_or(tag, |(r, _)| r))
}

/// What the security upgrade does with `current`: the default FE image when moving there is not
/// a downgrade, else `Err(release)` naming the newer release (e.g. "4.1.8"). A StarRocks FE's
/// metadata cannot be downgraded, so a newer release keeps its image.
pub fn starrocks_fe_upgrade_target(current: Option<&str>) -> Result<&'static str, String> {
    let target = DEFAULT_ENGINE_IMAGES.starrocks_fe_image;
    let current_release = current.and_then(image_release);
    match (current_release, image_release(target)) {
        (Some(c), Some(t)) if c > t => Err(format!("{}.{}.{}", c.0, c.1, c.2)),
        _ => Ok(target),
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
                ("trino", "trinodb/trino", "483", "2.8.0"),
                ("starrocks", "starrocks/fe-ubuntu", "4.1.4", "2.8.0"),
                ("starrocks", "starrocks/fe-ubuntu", "4.1.6", "2.9.0"),
                (
                    "starrocks",
                    "ghcr.io/egret-data/egret-starrocks-fe",
                    "4.1.7-egret.1",
                    "2.9.0"
                ),
                (
                    "starrocks",
                    "ghcr.io/egret-data/egret-starrocks-fe",
                    "4.1.7-egret.3",
                    "2.9.0"
                ),
                (
                    "starrocks",
                    "ghcr.io/egret-data/egret-starrocks-fe",
                    "4.1.7-egret.4",
                    "2.9.0"
                ),
            ]
        );
    }

    #[test]
    fn a_dropped_image_is_refused_naming_the_floor_and_the_way_up() {
        let err = unsupported_engine_image("trino", "trinodb/trino:470").expect("refused");
        assert!(
            err.contains("2.8.0") && err.contains("trinodb/trino:483"),
            "{err}"
        );
        assert!(
            unsupported_engine_image("trino", "docker.io/trinodb/trino:470@sha256:00").is_some()
        );
        assert!(unsupported_engine_image("trino", "trinodb/trino:483").is_none());
        assert!(unsupported_engine_image("starrocks", "trinodb/trino:470").is_none());
        assert!(plugin_version_for_image("trino", "trinodb/trino:470").is_err());
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
            stored_engine_image("trino", &json!({"imageVersion": "483"})),
            Some("trinodb/trino:483".into())
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
                &json!({"trinoConfig": {"coordinator": {}, "image": "trinodb/trino:483"}})
            ),
            Some("trinodb/trino:483".into())
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
                (
                    "ghcr.io/egret-data/egret-starrocks-fe",
                    "4.1.7-egret.1",
                    "sha256:da948dad19e98a5a43b0073ec48f87c9e7429246ce4581a000c0955c742af250"
                ),
                (
                    "ghcr.io/egret-data/egret-starrocks-fe",
                    "4.1.7-egret.3",
                    "sha256:5c74c820f9b741bc8054247f47f3d54dd9e526710b6fc2b5a7873cae6bb2d2b0"
                ),
                (
                    "ghcr.io/egret-data/egret-starrocks-fe",
                    "4.1.7-egret.4",
                    "sha256:55ec35ecf6df85320557f017e0c3c16056d82a258cf4a4e79a80a6565ac6fdc5"
                ),
            ]
        );
        assert_eq!(
            DEFAULT_ENGINE_IMAGES.starrocks_cn_image,
            "starrocks/cn-ubuntu:4.1.7@sha256:39f7320127c9924abb1f1ce0ba0583ab11989cbe5bd1bb6274ada6f89ba5740d"
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
    fn ships_opa_plugin_only_for_the_measured_trino_image() {
        // Arrange
        let pinned = "trinodb/trino:483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534";

        // Act / Assert
        assert!(ships_opa_plugin("trino", "trinodb/trino:483"));
        assert!(ships_opa_plugin("trino", pinned));
        assert!(!ships_opa_plugin("trino", "trinodb/trino:470"));
        assert!(!ships_opa_plugin("trino", "trinodb/trino:483@sha256:0000"));
        assert!(!ships_opa_plugin("starrocks", "trinodb/trino:483"));
        for k in KNOWN_ENGINE_IMAGES
            .iter()
            .filter(|k| k.engine_type == "starrocks")
        {
            let bare = format!("{}:{}", k.repository, k.tag);
            assert!(!ships_opa_plugin(k.engine_type, &bare), "{bare}");
            assert!(
                !ships_opa_plugin(k.engine_type, &format!("{bare}@{}", k.digest)),
                "{bare}"
            );
        }
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(
            plugin_version_for_image("trino", "  trinodb/trino:483  "),
            Ok("2.8.0")
        );
    }

    const EGRET4_DIGEST: &str =
        "sha256:55ec35ecf6df85320557f017e0c3c16056d82a258cf4a4e79a80a6565ac6fdc5";
    const EGRET3_DIGEST: &str =
        "sha256:5c74c820f9b741bc8054247f47f3d54dd9e526710b6fc2b5a7873cae6bb2d2b0";
    const EGRET_FE: &str = "ghcr.io/egret-data/egret-starrocks-fe";

    #[test]
    fn the_patched_rule_follows_level_and_release() {
        assert!(!is_patched_starrocks_fe("starrocks/fe-ubuntu:4.1.6"));
        assert!(!is_patched_starrocks_fe(&format!(
            "{EGRET_FE}:4.1.7-egret.3@{EGRET3_DIGEST}"
        )));
        assert!(!is_patched_starrocks_fe(&format!(
            "{EGRET_FE}:4.1.7-egret.3"
        )));
        assert!(is_patched_starrocks_fe(&format!(
            "{EGRET_FE}:4.1.7-egret.4"
        )));
        assert!(is_patched_starrocks_fe(&format!(
            "{EGRET_FE}:4.1.7-egret.4@{EGRET4_DIGEST}"
        )));
        assert!(is_patched_starrocks_fe(&format!(
            "{EGRET_FE}:4.1.8-egret.1"
        )));
        assert!(!is_patched_starrocks_fe("trinodb/trino:483"));
        assert!(!is_patched_starrocks_fe(""));
    }

    #[test]
    fn a_digest_decides_over_a_patched_tag() {
        assert!(!is_patched_starrocks_fe(&format!(
            "{EGRET_FE}:4.1.7-egret.4@{EGRET3_DIGEST}"
        )));
        assert!(!is_patched_starrocks_fe(&format!(
            "{EGRET_FE}:4.1.7-egret.4@sha256:0000000000000000000000000000000000000000000000000000000000000000"
        )));
        assert!(is_patched_starrocks_fe(&format!(
            "{EGRET_FE}:4.1.7-egret.3@{EGRET4_DIGEST}"
        )));
    }

    #[test]
    fn a_mirrored_image_counts_by_its_digest() {
        assert!(is_patched_starrocks_fe(&format!(
            "registry.local/mirror/egret-starrocks-fe@{EGRET4_DIGEST}"
        )));
        assert!(!is_patched_starrocks_fe(&format!(
            "registry.local/mirror/egret-starrocks-fe@{EGRET3_DIGEST}"
        )));
        assert!(!is_patched_starrocks_fe(
            "registry.local/mirror/egret-starrocks-fe@sha256:0000000000000000000000000000000000000000000000000000000000000000"
        ));
    }

    #[test]
    fn the_upgrade_target_is_the_default_unless_it_would_downgrade() {
        let default = DEFAULT_ENGINE_IMAGES.starrocks_fe_image;
        assert_eq!(
            starrocks_fe_upgrade_target(Some("starrocks/fe-ubuntu:4.1.6")),
            Ok(default)
        );
        assert_eq!(
            starrocks_fe_upgrade_target(Some("starrocks/fe-ubuntu:4.1-latest")),
            Ok(default)
        );
        assert_eq!(
            starrocks_fe_upgrade_target(Some(&format!("{EGRET_FE}:4.1.7-egret.3"))),
            Ok(default)
        );
        assert_eq!(starrocks_fe_upgrade_target(None), Ok(default));
        assert_eq!(
            starrocks_fe_upgrade_target(Some("starrocks/fe-ubuntu:4.1.8")),
            Err("4.1.8".to_string())
        );
    }
}
