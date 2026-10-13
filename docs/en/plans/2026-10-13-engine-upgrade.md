# Engine upgrade (Trino 483, StarRocks 4.1.6) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run Trino 483 and StarRocks 4.1.6 by default. Make egret derive each engine's Ranger plugin version from its exact image. Have the evaluator model Trino@2.8.0 and StarRocks@2.9.0, each proven against the real jars.

**Architecture:** egret-perm holds one table that maps an exact engine image to a plugin version, plus egret's pinned default images. `engine_plugin_version` resolves every engine through that table; the type-only mapping is deleted. `egret-ranger-eval` gains `Ranger2_9_0`. Every real-jar differential runs at four (type, version) pairs, and a pair counts as modelled only when it shows zero disagreements.

**Tech Stack:** Rust (egret-perm, egret-ranger-eval, egret-perm-compile, egret-engine-mgmt, egret-provisioner), sqlx migrations, the Java/Maven harness in `tests/ranger/`, React (control-plane pages), Docker for the live engines.

**Spec:** `docs/en/specs/2026-10-13-engine-upgrade-trino483-starrocks416-design.md`. Read it before any task.

## Global Constraints

**Target versions and pins**
- Target images, verbatim:
  - Trino `483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534` (plugin 2.8.0)
  - `starrocks/fe-ubuntu:4.1.6@sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31` (plugin 2.9.0)
  - `starrocks/cn-ubuntu:4.1.6@sha256:bf0f5ca7fd9fc4e568d4d0e593e362f229b376976b513a5a549b1b922d9ec23b`
  - `starrocks/allin1-ubuntu:4.1.6@sha256:5cbf09c4c788d2ca41a8e38599b650f04dbc6af0de51ebda0952770fb8333e5d`
- Current images that stay known, verbatim:
  - Trino `470@sha256:a591436ddc20cef15a4424704423326828591727b0e7c40c6ea4165d4564df67` (plugin 2.5.0)
  - `starrocks/fe-ubuntu:4.1.4@sha256:0b9099483e52ee66f9ee0244799bd32f3ccb491492857c59ada622ceb15b914b` (plugin 2.8.0)
  - `starrocks/cn-ubuntu:4.1.4@sha256:1cd28b5f3fd81213c78424064a56bfc423d7c0c577a9a6406171938329f9e532`
  - `starrocks/allin1-ubuntu:4.1.4@sha256:faf7ce9c24d9c29c9431b4e8cbd4bb7a74cd169907c63f0c5ebaacc7f9df276b`
- No egret default and no `deploy/` compose file may name a floating tag (`latest`, `*-latest`).

**Plugin versions and the modelled set**
- Use the plugin each engine ships. Trino stays on 2.8.0, with no custom image (user ruling).
- "Modelled" is a (type, version) PAIR. The target set is Trino@2.5.0, Trino@2.8.0, StarRocks@2.8.0 and StarRocks@2.9.0.
  - A pair joins `MODELLED_RANGER_PLUGINS` only in the task whose real-jar differential shows **0 disagreements** for it.
  - One disagreement means STOP and report. Never narrow a corpus or weaken an assertion.
- An image that is not in the table resolves to UNMODELLED, with a sentence naming the engine, its image and the fix. This covers a floating tag, an unknown tag, another repository or mirror, a digest mismatch, and a missing image. Never fall back to a type default.
- egret never changes a running engine's image. Migration 0185 makes today's implicit images explicit, so changing a default upgrades nobody.

**Process and safety**
- Never use the dev DB `egret`. Never set `EGRET_ALLOW_DB_SKIP=1`. DB tests take `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` explicitly.
- Run per-crate `cargo test -p <crate>` only during tasks. Run `cargo clippy -p <crate> --all-targets -- -D warnings` and `cargo fmt` on every crate you touched.
- Regenerate the API client with `cd frontend && npm run gen:api` only, never bare orval. Re-emit `openapi.json` with `target/debug/egret openapi > openapi.json` first.
- Never `git stash`, never `git checkout -- <file>` on uncommitted work, never `git clean`. Do not push.
- Commit identity hoangquocvietbk2.13@gmail.com. End every commit message with the trailer line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Mutations: assert the mutant compiles and the named test fails. Restore with a plain write, and compare SHA-256 on both sides.
- Migration numbers are shared with main. Before creating `0185_*`, run `git ls-tree --name-only origin/main migrations/ | tail -1`. If 0185 is taken, use the next free number and update every reference in this plan's later tasks.
- Live engines run in Docker on a throwaway database. Container names start with `engup-`. Never kill a process by name; kill only PIDs you started. Keep at least 30 GiB free, checking `df -h /` before every image pull and build.
- The frontend follows `docs/en/frontend-rules.md`.

---

## File structure

| File | Responsibility |
|---|---|
| `crates/egret-perm/src/engine_images.rs` (new) | `KNOWN_ENGINE_IMAGES`, `DEFAULT_ENGINE_IMAGES`, image-ref parsing, `plugin_version_for_image`, `stored_engine_image`. Pure, with no I/O. |
| `crates/egret-perm/src/actions.rs` | `MODELLED_RANGER_PLUGINS` grows here, one pair per proving task |
| `migrations/0185_engine_images_explicit.sql` (new) | Writes today's implicit default images into every provisioned engine's `spec` |
| `crates/egret-engine-mgmt/src/engines.rs` | Default constants come from egret-perm. `EngineDto` reports the derived plugin version for every engine. The route lists the modelled pairs. |
| `crates/egret-perm-compile/src/routes.rs` | `engine_plugin_version` resolves through egret-perm. `BoundEngine` carries `spec`. An unbound profile evaluates at the default image's version. |
| `crates/egret-ranger-eval/src/eval.rs` | `PluginVersion::Ranger2_9_0`, `EngineType::models`; `plugin_version()` is deleted; version branches are exhaustive `match` |
| `tests/ranger/pom.xml`, `crates/egret-ranger-eval/tests/differential.rs`, `crates/egret-policy/tests/ranger_engine_gate.rs`, `justfile` | The real-jar harness runs four pairs |
| `docs/en/plans/2026-10-13-ranger-2-9-0-reading.md` (new) | The `javap` comparison of 2.9.0 against 2.8.0 |
| `crates/egret-engine-mgmt-api/src/ranger_config.rs` | Changed only if the new images need different plugin files (Task 6) |
| `frontend/src/routes/control-plane/externalEngine.tsx`, `EngineDetail.tsx`, `EngineRegister.tsx` | Modelled pairs per type; the image and plugin version shown on the detail page |
| `docs/en/guides/engine-versions.md` (new) | Operator notes: what is pinned, what 4.1.6 changes, how to pin |
| `docs/en/plans/2026-10-13-engine-upgrade-live-result.md` (new) | Task 8's re-measurement |

---

### Task 1: The exact-image table and its resolver

**Files:**
- Create: `crates/egret-perm/src/engine_images.rs`
- Modify: `crates/egret-perm/src/lib.rs` (add `pub mod engine_images;` and re-export the public items the way `actions` items are re-exported. Read `lib.rs` first.)
- Test: unit tests inside `engine_images.rs`

**Interfaces:**
- Produces (used by Tasks 2, 3 and 7):
  - `pub struct KnownEngineImage { pub engine_type: &'static str, pub repository: &'static str, pub tag: &'static str, pub digest: &'static str, pub plugin_version: &'static str }`
  - `pub const KNOWN_ENGINE_IMAGES: [KnownEngineImage; 4]`
  - `pub struct DefaultEngineImages { pub trino_image_version: &'static str, pub starrocks_fe_image: &'static str, pub starrocks_cn_image: &'static str }` and `pub const DEFAULT_ENGINE_IMAGES: DefaultEngineImages`
  - `pub fn trino_image_ref(image_version: &str) -> String`, which returns `trinodb/trino:{image_version}`
  - `pub fn stored_engine_image(engine_type: &str, spec: &serde_json::Value) -> Option<String>`, which returns the full image ref or `None` when absent or blank
  - `pub fn plugin_version_for_image(engine_type: &str, image_ref: &str) -> Result<&'static str, String>`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_known_image_resolves_with_and_without_its_digest() {
        for k in KNOWN_ENGINE_IMAGES.iter() {
            let bare = format!("{}:{}", k.repository, k.tag);
            let pinned = format!("{bare}@{}", k.digest);
            assert_eq!(plugin_version_for_image(k.engine_type, &bare), Ok(k.plugin_version), "{bare}");
            assert_eq!(plugin_version_for_image(k.engine_type, &pinned), Ok(k.plugin_version), "{pinned}");
            let hub = format!("docker.io/{pinned}");
            assert_eq!(plugin_version_for_image(k.engine_type, &hub), Ok(k.plugin_version), "{hub}");
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
            stored_engine_image("starrocks", &json!({"starrocksConfig": {"feImage": "starrocks/fe-ubuntu:4.1.4"}})),
            Some("starrocks/fe-ubuntu:4.1.4".into())
        );
        assert_eq!(stored_engine_image("trino", &json!({})), None);
        assert_eq!(stored_engine_image("trino", &json!({"imageVersion": "  "})), None);
        assert_eq!(stored_engine_image("starrocks", &json!({"imageVersion": "x"})), None);
        assert_eq!(stored_engine_image("risingwave", &json!({"imageVersion": "1"})), None);
    }

    #[test]
    fn the_defaults_are_pinned_known_images() {
        let d = &DEFAULT_ENGINE_IMAGES;
        assert_eq!(plugin_version_for_image("trino", &trino_image_ref(d.trino_image_version)), Ok("2.8.0"));
        assert_eq!(plugin_version_for_image("starrocks", d.starrocks_fe_image), Ok("2.9.0"));
        for image in [trino_image_ref(d.trino_image_version), d.starrocks_fe_image.into(), d.starrocks_cn_image.into()] {
            assert!(image.contains("@sha256:"), "{image} carries its digest");
        }
    }
}
```

- [ ] **Step 2: Run the tests and confirm they fail**

Run: `cargo test -p egret-perm engine_images`
Expected: compile errors for the missing items.

- [ ] **Step 3: Implement**

```rust
//! Which Ranger plugin an engine image ships, by EXACT image. The only place
//! egret maps an engine to a plugin version: a provisioned engine through its
//! stored image, never through its type alone — `starrocks/fe-ubuntu:4.1-latest`
//! shipped 2.8.0 on 2026-09-28 and 2.9.0 from 2026-10-01, and a type-only map
//! cannot see that (spec `2026-10-13-engine-upgrade-…-design.md`).

pub struct KnownEngineImage {
    pub engine_type: &'static str,
    pub repository: &'static str,
    pub tag: &'static str,
    pub digest: &'static str,
    pub plugin_version: &'static str,
}

/// Measured: each row's plugin jar was read out of the image itself.
pub const KNOWN_ENGINE_IMAGES: [KnownEngineImage; 4] = [
    KnownEngineImage { engine_type: "trino", repository: "trinodb/trino", tag: "470",
        digest: "sha256:a591436ddc20cef15a4424704423326828591727b0e7c40c6ea4165d4564df67", plugin_version: "2.5.0" },
    KnownEngineImage { engine_type: "trino", repository: "trinodb/trino", tag: "483",
        digest: "sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534", plugin_version: "2.8.0" },
    KnownEngineImage { engine_type: "starrocks", repository: "starrocks/fe-ubuntu", tag: "4.1.4",
        digest: "sha256:0b9099483e52ee66f9ee0244799bd32f3ccb491492857c59ada622ceb15b914b", plugin_version: "2.8.0" },
    KnownEngineImage { engine_type: "starrocks", repository: "starrocks/fe-ubuntu", tag: "4.1.6",
        digest: "sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31", plugin_version: "2.9.0" },
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

pub fn stored_engine_image(engine_type: &str, spec: &serde_json::Value) -> Option<String> {
    let raw = match engine_type {
        "trino" => spec.get("imageVersion")?.as_str()?.trim().to_string(),
        "starrocks" => spec.get("starrocksConfig")?.get("feImage")?.as_str()?.trim().to_string(),
        _ => return None,
    };
    if raw.is_empty() {
        return None;
    }
    Some(if engine_type == "trino" { trino_image_ref(&raw) } else { raw })
}

/// `repo[:tag][@digest]` → (repository, tag, digest). `docker.io/` is Docker
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

pub fn plugin_version_for_image(engine_type: &str, image_ref: &str) -> Result<&'static str, String> {
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
            let known: Vec<String> = KNOWN_ENGINE_IMAGES
                .iter()
                .filter(|k| k.engine_type == engine_type)
                .map(|k| format!("{}:{}", k.repository, k.tag))
                .collect();
            Err(format!(
                "image \"{image_ref}\" is not one whose Ranger plugin egret has measured for a \
                 {engine_type} engine; pin the engine to one of: {}",
                known.join(", ")
            ))
        }
    }
}
```

If `Option::is_none_or` is not available on the pinned toolchain, use `digest.map_or(true, |d| d == k.digest)`. Check `rustc --version` and clippy's preference.

- [ ] **Step 4: Run the tests and confirm they pass**

Run: `cargo test -p egret-perm engine_images`. Then run clippy and fmt for egret-perm.

- [ ] **Step 5: Mutate the resolver to show the tests can fail**

Make two mutations, one at a time, and restore each by SHA-256:
1. Drop the `&& digest.is_none_or(..)` clause. `anything_not_in_the_table_is_unmodelled…` must fail on the zero-digest row.
2. Drop the `docker.io/` strip. `every_known_image_resolves…` must fail.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-perm/src/engine_images.rs crates/egret-perm/src/lib.rs
git commit -m "feat(perm): which Ranger plugin an engine image ships, by exact image"
```

---

### Task 2: Pin the defaults and make today's implicit images explicit

**Files:**
- Create: `migrations/0185_engine_images_explicit.sql`. Re-check the number first; see Global Constraints.
- Modify:
  - `crates/egret-engine-mgmt/src/engines.rs:47,59,60`. Delete the three constants and use `egret_perm::engine_images::DEFAULT_ENGINE_IMAGES` at every use (`:701`, `:2972`, `:3193`, `:3199`, `:3720`, `:3726`, `:5371`, and the test at `:7429`).
  - `crates/egret-engine-mgmt-api/src/lib.rs:709-710`, which has `3.3-latest` defaults. Use the same constants if that crate can depend on egret-perm (check `Cargo.toml`). If it cannot without a cycle, write the literal pinned values and add a test in egret-engine-mgmt asserting the two are equal.
  - `deploy/docker-compose.dev.yml:60,74`: `trinodb/trino:483@sha256:db58…` and `starrocks/allin1-ubuntu:4.1.6@sha256:5cbf…`.
  - `crates/egret-workflow/oracle-exec/docker-compose.w5b.yml:39` and `crates/egret-workflow/oracle-exec/w5b-plugins.json:24`: StarRocks to the 4.1.6 all-in-one pin.
  - Any live-test doc comment that names "the image `docker-compose.dev.yml` runs" (`crates/egret-engine-mgmt/tests/starrocks_ddl_live.rs:12,24`). Update the version text only.
- Test: `crates/egret-engine-mgmt/tests/no_floating_images.rs` (new), and a migration test in the crate that already tests migrations against `egret_test`. Find it with `git grep -l "0184_engine_external" crates/*/tests`.

**Interfaces:**
- Consumes: `DEFAULT_ENGINE_IMAGES` and `stored_engine_image` (Task 1).
- Produces: every provisioned engine row has an explicit image in `spec` after 0185. Tasks 3 and 7 rely on this.

- [ ] **Step 1: Write the floating-tag guard test**

```rust
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
    assert_eq!(engine_images.len(), 2, "expected the Trino and StarRocks services: {engine_images:?}");
    for i in engine_images {
        assert!(!floating(i) && i.contains("@sha256:"), "{i}");
    }
}
```

- [ ] **Step 2: Run it and confirm it fails** on `trinodb/trino:latest` in the compose file.

Run: `cargo test -p egret-engine-mgmt --test no_floating_images`

- [ ] **Step 3: Write the migration test (red)**

Insert into `engine_mgmt.engines`, through the same helper the crate's other tests use:
- a provisioned `trino` engine with `spec = {}`;
- a provisioned `trino` engine with `{"imageVersion":"483"}`;
- a provisioned `starrocks` engine with `{"starrocksConfig":{"feReplicas":1}}`;
- an `external` StarRocks engine with `spec = {}`.

Run 0185's SQL (`include_str!`, the way SP4d-3's backfill test ran 0180), scoped to these ids by wrapping each `UPDATE`'s `WHERE` with `AND id = ANY($1)`. If the migration text cannot take a bind, run it on a fresh test DB the way the crate's migration tests already do; read one first.

Assert:
- the empty Trino row → `imageVersion = "470"`;
- the explicit Trino row is unchanged;
- the StarRocks row → `feImage = "starrocks/fe-ubuntu:4.1-latest"` and `cnImage = "starrocks/cn-ubuntu:4.1-latest"`, with `feReplicas` kept;
- the external row is unchanged.

- [ ] **Step 4: Write the migration**

```sql
-- 0185_engine_images_explicit.sql
--
-- An engine whose spec names no image ran the DEFAULT of whichever egret
-- applied it last (`engines.rs`: `.unwrap_or(DEFAULT_IMAGE_VERSION)`,
-- `.unwrap_or_else(|| DEFAULT_SR_FE_IMAGE.into())`). The defaults move to
-- Trino 483 and StarRocks 4.1.6 in the same change, so without this every such
-- engine would be upgraded on its next apply, behind its operator's back.
-- Write the defaults those engines run TODAY into their rows: Trino 470, and
-- StarRocks' floating `4.1-latest` (which is exactly what they run; egret now
-- reports it as unmodelled until the operator pins it).
-- External engines are untouched: egret never ran their image.

UPDATE engine_mgmt.engines
   SET spec = jsonb_set(spec, '{imageVersion}', '"470"')
 WHERE type = 'trino' AND origin = 'provisioned'
   AND coalesce(btrim(spec ->> 'imageVersion'), '') = '';

UPDATE engine_mgmt.engines
   SET spec = jsonb_set(spec, '{starrocksConfig}',
                coalesce(spec -> 'starrocksConfig', '{}'::jsonb)
                || jsonb_build_object('feImage', 'starrocks/fe-ubuntu:4.1-latest'))
 WHERE type = 'starrocks' AND origin = 'provisioned'
   AND coalesce(btrim(spec #>> '{starrocksConfig,feImage}'), '') = '';

UPDATE engine_mgmt.engines
   SET spec = jsonb_set(spec, '{starrocksConfig}',
                coalesce(spec -> 'starrocksConfig', '{}'::jsonb)
                || jsonb_build_object('cnImage', 'starrocks/cn-ubuntu:4.1-latest'))
 WHERE type = 'starrocks' AND origin = 'provisioned'
   AND coalesce(btrim(spec #>> '{starrocksConfig,cnImage}'), '') = '';
```

Before writing it, confirm with `\d engine_mgmt.engines` on `egret_test` that `spec` is `jsonb` and NOT NULL. If it is nullable, guard each `UPDATE` with `coalesce(spec, '{}'::jsonb)`. Also confirm the provisioner reads `starrocksConfig.feImage` / `cnImage` (`engines.rs:3187-3199`). If any other field also chooses the StarRocks image, backfill it too and say so in the migration comment.

- [ ] **Step 5: Replace the constants and pins (Files list above).** Then run the egret-engine-mgmt, egret-engine-mgmt-api and egret-provisioner tests. Tests that pinned `"470"` or `"starrocks/fe-ubuntu:4.1-latest"` as the DEFAULT now expect the new defaults. Tests that pass an explicit image keep theirs.

- [ ] **Step 6: Run everything and confirm it is green.** Both new tests pass. Run per-crate tests, clippy and fmt.

- [ ] **Step 7: Commit**

```bash
git add migrations/0185_engine_images_explicit.sql crates deploy
git commit -m "feat(engine-mgmt): pin Trino 483 and StarRocks 4.1.6 by digest; make every engine's image explicit"
```

---

### Task 3: Resolve every engine's plugin version through its image

**Files:**
- Modify:
  - `crates/egret-ranger-eval/src/eval.rs`:
    - delete `EngineType::plugin_version` (`:655-667`);
    - add `EngineType::models(self, v: PluginVersion) -> bool`, the evaluator's own list of the versions it has been read against for that type: today Trino→{2.5.0}, StarRocks→{2.8.0};
    - change the mismatch check at `:3112-3121` to `!declared.models(version)`;
    - make `UndecidableCause::PluginVersionMismatch` carry `{ engine, declared }` with no `measured`, and update its message to list the modelled versions.
  - `crates/egret-ranger-eval/examples/live_gate.rs:256,268`, which takes the version as an argument or constant.
  - `crates/egret-perm-compile/src/routes.rs`:
    - `engine_plugin_version` (`:2283`): provisioned engines resolve `stored_engine_image(type, spec)` → `plugin_version_for_image` → `ranger_plugin_is_modelled` → variant;
    - `BoundEngine` gains `spec: serde_json::Value` and its query selects `spec`;
    - `profile_plugin_version` (`:2375`), the unbound case, resolves the default image for the profile's type;
    - the tests at `:4500-4540`.
  - `crates/egret-perm-compile/src/explain_wire.rs`, if `PluginVersionMismatch` is mapped there.
  - `crates/egret-perm/src/actions.rs`: add `pub fn plugin_version_variant_name(...)` ONLY if needed. Prefer a `match` in `routes.rs` with an exhaustive arm per known string and an `Err` for the rest.
- Test: the `routes.rs` unit tests, plus a DB test in `crates/egret-perm-compile/tests/` next to the existing explain/migrate engine tests (`git grep -l bound_plugin_versions crates/egret-perm-compile/tests`).

**Interfaces:**
- Consumes: Task 1's functions, and Task 2's guarantee that provisioned engines carry an image.
- Produces: `engine_plugin_version(name, origin, type, declared: Option<&str>, spec: &serde_json::Value) -> Result<PluginVersion, String>`, with the new `spec` parameter. Tasks 4, 5 and 7 call it unchanged.

- [ ] **Step 1: Write the failing unit tests** (replacing `a_provisioned_engine_maps_its_type_and_an_external_one_its_declared_version`):

```rust
#[test]
fn a_provisioned_engine_resolves_through_its_exact_image() {
    let trino470 = serde_json::json!({"imageVersion": "470"});
    assert_eq!(engine_plugin_version("e", "provisioned", "trino", None, &trino470), Ok(PluginVersion::Ranger2_5_0));
    let sr414 = serde_json::json!({"starrocksConfig": {"feImage": "starrocks/fe-ubuntu:4.1.4"}});
    assert_eq!(engine_plugin_version("e", "provisioned", "starrocks", None, &sr414), Ok(PluginVersion::Ranger2_8_0));
}

#[test]
fn a_provisioned_engine_on_a_floating_or_missing_image_is_refused_with_the_fix() {
    let floating = serde_json::json!({"starrocksConfig": {"feImage": "starrocks/fe-ubuntu:4.1-latest"}});
    let err = engine_plugin_version("sr1", "provisioned", "starrocks", None, &floating).unwrap_err();
    assert!(err.contains("sr1") && err.contains("4.1-latest") && err.contains("pin"), "{err}");
    let missing = serde_json::json!({});
    let err = engine_plugin_version("t1", "provisioned", "trino", None, &missing).unwrap_err();
    assert!(err.contains("t1") && err.contains("no image"), "{err}");
}

#[test]
fn an_image_whose_pair_is_not_yet_modelled_is_refused_not_guessed() {
    // Until Task 4/5 prove them, 483 (Trino@2.8.0) and 4.1.6 (StarRocks@2.9.0)
    // resolve to a version that is NOT a modelled pair.
    let trino483 = serde_json::json!({"imageVersion": "483"});
    let err = engine_plugin_version("t", "provisioned", "trino", None, &trino483).unwrap_err();
    assert!(err.contains("2.8.0"), "{err}");
}
```

`an_image_whose_pair_is_not_yet_modelled…` is deliberately temporary. Task 5 inverts it to assert Trino@2.8.0 resolves, and Task 4 adds the StarRocks case. Mark that in a comment naming those tasks.

- [ ] **Step 2: Run them and confirm they fail.** Run `cargo test -p egret-perm-compile --lib engine_plugin_version`. Expect compile errors: an argument is missing.

- [ ] **Step 3: Implement**

```rust
pub(crate) fn engine_plugin_version(
    name: &str,
    origin: &str,
    r#type: &str,
    declared: Option<&str>,
    spec: &serde_json::Value,
) -> Result<egret_ranger_eval::eval::PluginVersion, String> {
    let version: &str = if origin == "external" {
        declared.ok_or_else(|| format!("engine \"{name}\" is external but declares no Ranger plugin version"))?
    } else {
        let image = egret_perm::engine_images::stored_engine_image(r#type, spec).ok_or_else(|| {
            format!("engine \"{name}\" ({}) names no image, so egret cannot tell which Ranger plugin it runs; pin its image", r#type)
        })?;
        egret_perm::engine_images::plugin_version_for_image(r#type, &image)
            .map_err(|why| format!("engine \"{name}\": {why}"))?
    };
    if !egret_perm::ranger_plugin_is_modelled(r#type, version) {
        return Err(format!(
            "engine \"{name}\" is a {} running Ranger plugin {version}, a (type, version) pair egret's \
             evaluator has not been verified against (it models {}), so a decision cannot be proved on it",
            r#type,
            egret_perm::modelled_ranger_plugins_sentence()
        ));
    }
    use egret_ranger_eval::eval::PluginVersion as V;
    match version {
        "2.5.0" => Ok(V::Ranger2_5_0),
        "2.8.0" => Ok(V::Ranger2_8_0),
        other => Err(format!("engine \"{name}\": plugin {other} is listed as modelled but the evaluator has no variant for it")),
    }
}
```

In `profile_plugin_version`, the unbound case (`[] =>`):

```rust
[] => {
    let d = &egret_perm::engine_images::DEFAULT_ENGINE_IMAGES;
    let (t, spec) = match profile_engine {
        EngineType::Trino => ("trino", serde_json::json!({"imageVersion": d.trino_image_version})),
        EngineType::StarRocks => ("starrocks", serde_json::json!({"starrocksConfig": {"feImage": d.starrocks_fe_image}})),
    };
    engine_plugin_version("(no engine bound; egret's default image)", "provisioned", t, None, &spec)
        .map_err(|s| PermHttpError::Conflict(format!("cannot explain profile #{service_id}: {s}")))
}
```

The `"external"` name check must stay exactly as it is. The external branch keeps every sentence it has today. `rangerPluginVersionModelled` keeps calling `egret_engine_mgmt::external::plugin_version_is_modelled`, which already wraps `ranger_plugin_is_modelled`.

- [ ] **Step 4: Update every caller.** That covers `BoundEngine::plugin_version` (pass `&self.spec`), the `bound_engines` query (`SELECT name, origin, type, ranger_plugin_version, spec …`), `migrate.rs` (unchanged; it calls `e.plugin_version()`), the evaluator's mismatch check, and `live_gate.rs`.

Then grep for `plugin_version()` and confirm no caller of the deleted method remains: `git grep -n "\.plugin_version()" crates` must show only `BoundEngine`'s.

- [ ] **Step 5: Write the DB test.** Set up a profile bound to two provisioned Trino engines, one at `"470"` and one at `"483"`. Explain must refuse with the 2.8.0 sentence; that is correct until Task 5. Then a profile bound to one engine at `"470"`: explain evaluates at 2.5.0, as before.

A provisioned StarRocks engine at `4.1-latest` makes both explain and migrate refuse with the "pin" sentence. Assert the HTTP status and the sentence. Find the fixture helpers in the existing explain/migrate DB tests.

- [ ] **Step 6: Run the tests.** Run per-crate for egret-ranger-eval, egret-perm-compile and egret-engine-mgmt, then clippy and fmt.

- [ ] **Step 7: Mutate to show the tests can fail.** In `engine_plugin_version`, make the provisioned branch return `Ok(V::Ranger2_5_0)` for Trino without reading the image. The floating/missing test and the DB test must fail by name. Restore by SHA-256.

- [ ] **Step 8: Commit**

```bash
git commit -am "fix(perm-compile): an engine's plugin version comes from its exact image, never its type"
```

---

### Task 4: Read Ranger 2.9.0 and model StarRocks@2.9.0

**Files:**
- Create: `docs/en/plans/2026-10-13-ranger-2-9-0-reading.md`
- Modify:
  - `crates/egret-ranger-eval/src/eval.rs`: `PluginVersion::Ranger2_9_0`; `EngineType::models`, where StarRocks→{2.8.0, 2.9.0} only after Step 6; every version-conditional made an exhaustive `match`, including `:2482` `engine.plugin_version != Some(PluginVersion::Ranger2_8_0)`.
  - `crates/egret-perm-compile/src/equivalence.rs:149-153`, `crates/egret-perm-compile/src/explain_wire.rs:147-148`, so that `WirePluginVersion` gains `Ranger2_9_0`.
  - `crates/egret-perm-compile/src/routes.rs`: the `"2.9.0" => V::Ranger2_9_0` arm.
  - `crates/egret-perm/src/actions.rs`: `MODELLED_RANGER_PLUGINS` gains `("starrocks", "2.9.0")` only in Step 6.
  - `tests/ranger/pom.xml`, if 2.9.0 needs a newer `maven.compiler.release` or extra dependencies.
  - `crates/egret-ranger-eval/tests/differential.rs`: a `starrocks_2_9_0()` profile, run by every test that runs `starrocks()`, and every `for version in ["2.5.0", "2.8.0"]` loop gains `"2.9.0"`.
  - `crates/egret-policy/tests/ranger_engine_gate.rs:39`.
  - `justfile` `test-ranger-gates` (the echo line names the versions).
  - `openapi.json` and the generated client, via `npm run gen:api`.

**Interfaces:**
- Consumes: Task 3's `engine_plugin_version` and `EngineType::models`.
- Produces: `PluginVersion::Ranger2_9_0` and `WirePluginVersion::Ranger2_9_0` (serialised as the existing variants are; read `explain_wire.rs`), plus the modelled pair StarRocks@2.9.0.

- [ ] **Step 1: Read the jar.** `mvn dependency:copy -Dartifact=org.apache.ranger:ranger-plugins-common:2.9.0 -DoutputDirectory=<scratch>`, and the same for 2.8.0. For every class and method that the `PluginVersion` doc comment (`eval.rs:670-688`) and the SP4d-2b/SP4d-4 docs name, compare `javap -c -p` output between 2.8.0 and 2.9.0 after stripping constant-pool indices. At minimum:
  - `RangerPolicyEngineImpl` / `PolicyEngine` evaluation order;
  - `RangerDefaultPolicyEvaluator.evaluate`, `getMatchingPolicyItem`, `getEvalOrder` / `compareTo`;
  - `RangerDefaultPolicyResourceMatcher`, `RangerResourceTrie` (`getEvaluatorsForResource` / candidacy);
  - `RangerDefaultDataMaskPolicyItemEvaluator`, `RangerDefaultRowFilterPolicyItemEvaluator`, `updateAccessResult`;
  - `RangerTagEnricher` / service-resource matching and match types;
  - `isSuperUser`;
  - `RangerRequestExprResolver.PATTERN`, `JavaScriptEdits`;
  - `RangerAccessResult` arbitration by `policyPriority`.

  Write each class/method row as byte-identical, or as differing with the exact bytecode difference and what it changes, to `2026-10-13-ranger-2-9-0-reading.md`.

- [ ] **Step 2: Decide per difference.**
  - A difference on a modelled path gets its own `match` arm under `Ranger2_9_0`, plus a unit test that pins the 2.9.0 behaviour.
  - Alternatively, it gets an `UndecidableCause` refusal for 2.9.0 that names the construct.
  - **If more than three paths differ, or any difference reaches the access evalOrder, STOP and report.** The task becomes a design question for the user.

- [ ] **Step 3: Add the variant and make every version branch exhaustive.**
  - Replace `!= Some(PluginVersion::Ranger2_8_0)` at `:2482` with a `match` that names each variant. 2.9.0 takes whatever the reading says.
  - `git grep -n "Ranger2_8_0\|Ranger2_5_0" crates` must show no `==`/`!=` comparison left in non-test code.

- [ ] **Step 4: Run the harness at 2.9.0, before adding the pair to the modelled set.**
  - Add `starrocks_2_9_0()`: a copy of `starrocks()` with `plugin_version: PluginVersion::Ranger2_9_0` and `ranger_version: "2.9.0"`. Every test that iterates profiles includes it.
  - Each `for version in ["2.5.0", "2.8.0"]` gains `"2.9.0"`. For a Java-only loop that is paired with an engine, use the version's engine: 2.9.0 with StarRocks.
  - The `ranger_engine_gate.rs` loop gains 2.9.0.
  - Run `just test-ranger-gates`. It needs a JVM and Maven; `df -h /` first.

  Expected: **0 disagreements, and no `error` entries**, at 2.9.0 on every test, including the corpus, tag tiers, trie candidacy, editor levels, migration corpus and dropped-probe oracle. If any disagreement appears, STOP and report it with the case.

- [ ] **Step 5: Run it at the modelled set's level.** Add a test in `routes.rs` next to the existing "listed as modelled ⇒ has a variant" test (`:4515`): every `MODELLED_RANGER_PLUGINS` pair resolves through `engine_plugin_version("e","external",t,Some(v),&json!({}))`, AND every `EngineType::models` version appears in `MODELLED_RANGER_PLUGINS`. That makes two directions, so the two lists cannot drift.

- [ ] **Step 6: Add the pair.** Add `("starrocks", "2.9.0")` to `MODELLED_RANGER_PLUGINS` (the array length becomes 3) and StarRocks→2.9.0 to `EngineType::models`.

  Invert Task 3's temporary test for StarRocks: a provisioned engine at `starrocks/fe-ubuntu:4.1.6` → `Ok(Ranger2_9_0)`.

  Update the doc comments that say "Trino@2.5.0 and StarRocks@2.8.0 only": `actions.rs`, `engines.rs:319-322`, `external.rs:26`, `:338`, `:450`, `routes.rs:2279`. Rewrite each to describe the list (`modelled_ranger_plugins_sentence()`) rather than enumerate it, wherever the text is a utoipa description or a comment.

- [ ] **Step 7: Regenerate the API.** Run `target/debug/egret openapi > openapi.json`, then `cd frontend && npm run gen:api`. Check that `encodeURIComponent` is still present in the generated client. Run vitest for any frontend test that lists plugin-version enum values.

- [ ] **Step 8: Mutate to show the oracle can fail.** In the 2.9.0 arm of whatever branch the reading changed (or, if nothing changed, at `:2482`), give 2.9.0 the OTHER branch. At least one real-jar test must report a disagreement at 2.9.0. Restore by SHA-256.

  If no path differs and the `:2482` branch cannot be distinguished at 2.9.0, say so with the evidence. Then mutate `EngineType::models` instead, and show the routes drift test fails.

- [ ] **Step 9: Commit**

```bash
git commit -am "feat(ranger-eval): model StarRocks at Ranger 2.9.0, read from the jar and proved on it"
```

---

### Task 5: Model Trino@2.8.0

**Files:**
- Modify:
  - `crates/egret-ranger-eval/tests/differential.rs`: a `trino_2_8_0()` profile (Trino service-def, `PluginVersion::Ranger2_8_0`, `ranger_version: "2.8.0"`), run by every test that runs `trino()`.
  - `crates/egret-ranger-eval/src/eval.rs`: `EngineType::models` gets Trino→{2.5.0, 2.8.0} in Step 3.
  - `crates/egret-perm/src/actions.rs`: `("trino", "2.8.0")`, which makes the array length 4.
  - The Task 3 temporary test, inverted.
  - `frontend/src/routes/control-plane/externalEngine.tsx:28`; see Task 7, and do not touch it here.

**Interfaces:**
- Consumes: Task 4's exhaustive version branches.
- Produces: the modelled pair Trino@2.8.0.

- [ ] **Step 1: Find every Trino-specific branch.** List every place `eval.rs` branches on `EngineType::Trino` or on a Trino service-def construct (`trinouser`, `sysinfo`, `function`, `procedure`, `impliedGrants` `all`). For each, record whether it also branches on `PluginVersion`, and if it does, which arm 2.8.0 takes. Write this list into the Task 5 section of `2026-10-13-ranger-2-9-0-reading.md`.

  The question this answers: does any Trino-only path assume 2.5.0 behaviour that 2.8.0 changed? The 2.5.0→2.8.0 comparison in the `PluginVersion` doc comment covers the shared paths.

- [ ] **Step 2: Run the harness for Trino at 2.8.0.** Add the profile, run `just test-ranger-gates`, and expect 0 disagreements and no `error` entries. On any disagreement, STOP and report it with the case.

- [ ] **Step 3: Add the pair.** Add it to `MODELLED_RANGER_PLUGINS` and to `EngineType::models`. Invert Task 3's temporary test: a provisioned Trino at `"483"` → `Ok(Ranger2_8_0)`. The Task 3 DB test of a mixed 470/483 profile now expects "bound engines run different Ranger plugin versions" (`routes.rs` `profile_plugin_version`) for explain. For migrate (`bound_plugin_versions`), it expects a proof at BOTH versions. Update both assertions.

- [ ] **Step 4: Mutate to show the tests can fail.** Remove `("trino","2.8.0")` from `MODELLED_RANGER_PLUGINS` only. The Task 4 drift test must fail. Restore by SHA-256.

- [ ] **Step 5: Run per-crate tests, clippy and fmt, then commit**

```bash
git commit -am "feat(ranger-eval): model Trino at Ranger 2.8.0 (Trino 483), proved on the real jar"
```

---

### Task 6: The plugin files on the new engines

**Files:**
- Modify (only if Step 2 finds a change): `crates/egret-engine-mgmt-api/src/ranger_config.rs` and its golden files under `crates/egret-engine-mgmt-api/tests/golden/`, `crates/egret-provisioner/src/trino.rs`, and `deploy/starrocks/ranger/*.xml`. The latter is pinned to the renderer by the test from `fix/dg-backend-small`, if merged; otherwise compare by hand.
- Create: section 1 of `docs/en/plans/2026-10-13-engine-upgrade-live-result.md`.

**Interfaces:**
- Consumes: the pinned images (Task 2) and the rendered files (`GET /api/control-plane/engines/{id}/ranger-plugin-config` for an external engine; read SP4d-4's live result §2 for the procedure).
- Produces: a statement, with evidence, that the files egret renders work on Trino 483 and StarRocks 4.1.6, or the fix.

- [ ] **Step 1: Bring up the stack.**
  - Create a throwaway DB `egret_engup_live` in `egret-dev-postgres-1`.
  - Build `cargo build -p egret-bin` and run `target/debug/egret serve` on port 8096, with `EGRET_CATALOG__PUBLIC_URL=http://host.docker.internal:8096`. Round 2 of the view measurement found that egret needs dummy `EGRET_STORAGE__*` env vars to boot; see `docs/en/plans/2026-10-13-view-clearance-measurement.md`.
  - Start `engup-trino` (the Trino 483 pin, memory connector, HTTP with no auth) and `engup-sr` (the 4.1.6 all-in-one pin).
  - Record `df -h /` before and after.

- [ ] **Step 2: Register both as external engines through the UI or API** (`POST /api/control-plane/engines/external`). Use plugin version 2.8.0 for Trino and 2.9.0 for StarRocks. Install exactly the files the Plugin config tab gives. Record for each engine:
  1. **The plugin loads.** Check the engine log for plugin init, with no unknown-property warnings; quote them if any.
  2. **The plugin polls egret.** Check the engine's log for `Switched policy engine` or `Updated tags-cache`, plus the policy version.
  3. **A known allow returns rows and a known deny returns the engine's denial.** Use one hand-written policy each, as SP4d-4 Task 13 did.

  Any warning about an unknown or removed key is a renderer fix: make it, update the golden test, and re-run 1–3.

- [ ] **Step 3: Repeat the three checks on a provisioned-shape configuration for Trino.** Trino's `access-control.properties` comes from the provisioner (`crates/egret-provisioner/src/trino.rs:132-135`, `:360-367`) and can differ from the external renderer. Render it with the provisioner's own function in a small test or example, mount it, and check plugin load and polling.

- [ ] **Step 4: Write section 1 of the live-result doc.** Include versions proven by output (`SELECT current_version()`, `/v1/info`, and the jar names), each check's evidence, and any fix with its commit.

- [ ] **Step 5: Stop your containers and processes by PID.** Keep the DB, and record what was left.

- [ ] **Step 6: Commit** the doc and any renderer fix.

```bash
git commit -m "docs(plans): the plugin files egret renders, on Trino 483 and StarRocks 4.1.6"
```

---

### Task 7: Show it to the operator: the image, the plugin version, the modelled pairs

**Files:**
- Modify:
  - `crates/egret-engine-mgmt/src/engines.rs`: the `EngineDto` doc and fields at `:315-350`.
  - `frontend/src/routes/control-plane/externalEngine.tsx:28`, `EngineRegister.tsx`, `EngineDetail.tsx`, and their tests.
  - `openapi.json` and the generated client.
- Create: `docs/en/guides/engine-versions.md`
- Test:
  - Rust: the DTO conversion unit test next to `From<Engine> for EngineDto`, and a route test for the new list.
  - Frontend: `EngineRegister.test.tsx`, plus a test for `EngineDetail`.

**Interfaces:**
- Consumes: `engine_plugin_version` (Task 3). It lives in egret-perm-compile, so `EngineDto` cannot call it. Use the egret-perm functions `stored_engine_image`, `plugin_version_for_image` and `ranger_plugin_is_modelled` directly, which are the same three steps without the variant.
- Produces:
  - `EngineDto` gains:
    - `image: Option<String>`: provisioned engines only, the full stored image ref;
    - `ranger_plugin_version: Option<String>`: today external only, now also the version a provisioned engine's image resolves to;
    - `ranger_plugin_version_modelled: Option<bool>`: now for every engine;
    - `ranger_plugin_version_note: Option<String>`: the unmodelled sentence, when not modelled.
  - `GET /api/control-plane/engines/modelled-ranger-plugins` → `[{ "type": "trino", "version": "2.5.0" }, …]`, straight from `MODELLED_RANGER_PLUGINS`, requiring the same permission as listing engines.

- [ ] **Step 1: Write the Rust tests.**
  - A provisioned StarRocks engine at `4.1-latest` → `rangerPluginVersionModelled: false` and a note containing "pin".
  - A provisioned Trino at `483` → `rangerPluginVersion: "2.8.0"`, `modelled: true`.
  - An external engine is unchanged.
  - The list route returns exactly `MODELLED_RANGER_PLUGINS`, in order.

  Run them and confirm they fail; then implement and confirm they pass.

- [ ] **Step 2: Wire the frontend.**
  - Replace `OFFERED_PLUGIN_VERSIONS` with data from the new route, filtered by the chosen engine type. The register picker shows only that type's modelled versions, plus "Other…" as today.
  - The engine detail page shows `image` and the plugin version. When the version is not modelled, it shows the note in the page's existing warning-callout style (look at how `EngineDetail.tsx` shows other warnings).
  - Write the tests first:
    - the picker lists 2.8.0 and 2.9.0 for StarRocks and not 2.5.0;
    - the detail page renders the note for an unmodelled engine and no note for a modelled one.

- [ ] **Step 3: Regenerate and check.** Re-emit `openapi.json`, run `npm run gen:api`, check `encodeURIComponent`, then run vitest on the touched files, `npm run lint` and `npx tsc --noEmit`.

- [ ] **Step 4: Write the operator guide** `docs/en/guides/engine-versions.md`, with these sections:
  - **What egret pins.** The four known images with digests, and the plugin each ships.
  - **What StarRocks 4.1.6 changes.** Quote the view measurement's round 2 B: a user with no grant can no longer read a plain view, and `SECURITY INVOKER` is now enforced. Tell operators to expect denials where views were read without grants.
  - **Engines on a floating tag** are now reported as unmodelled. `explain` and migrations refuse for their profile, and enforcement is unchanged.
  - **How to pin.** `PUT /api/control-plane/engines/{id}`, with exact JSON for Trino (`imageVersion`) and StarRocks (`starrocksConfig.feImage` / `cnImage`). Moving an engine to a new image restarts it; say what the existing update route does (read it).

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(control-plane): every engine shows its image and Ranger plugin version; the picker offers modelled pairs"
```

---

### Task 8: Re-measure on the new versions

**Files:**
- Modify: `docs/en/plans/2026-10-13-engine-upgrade-live-result.md` (sections 2 and 3).

**Interfaces:**
- Consumes: everything above, and `docs/en/plans/2026-10-13-view-clearance-measurement.md` (rounds 1 and 2: their cells, SQL and users).
- Produces: the view matrix on Trino 483 and StarRocks 4.1.6. This is spec 2's input.

- [ ] **Step 1: Write predictions for every cell first and commit them.** The cells are round 1's Q1–Q5 and round 2's A (1–10) and B, on Trino 483 and StarRocks 4.1.6. Each cell is predicted as "same as the old version" or as a stated change. Then commit:

```bash
git commit -m "docs(plans): engine-upgrade re-measurement predictions, written before observing"
```

- [ ] **Step 2: Observe every cell.** Use the same users, SQL and policies, and the stack from Task 6. Deviations from the round 1 and 2 recipes (the SQL-inserted tag service for an external Trino, the stub warehouse rows) are allowed only as labelled deviations, quoted from the earlier doc.

- [ ] **Step 3: Rerun the SP4d-4 live essentials** on both engines, through egret's UI or API:
  - the 409 overlap refusal, and the broader rule winning live;
  - a table-stopping `select` denied at column depth;
  - one pruned migration whose preview matches the engine, with undo.

  A difference between what a page or preview claims and what the engine does is **Critical**.

- [ ] **Step 4: Write sections 2 and 3.**
  - Every cell, with old result | new result | changed?.
  - A short "what spec 2 should assume" list.
  - Every cell that changed is listed at the top.

- [ ] **Step 5: Stop containers and processes by PID.** List what is left.

- [ ] **Step 6: Commit**

```bash
git commit -m "docs(plans): view matrix and live essentials on Trino 483 and StarRocks 4.1.6"
```

---

## Self-review

**Spec coverage.**

| Spec section | Task |
|---|---|
| §1 pins | Task 2 |
| §2 exact-image table and the deletion of the type-only map | Tasks 1 and 3 |
| §3 2.9.0 reading, the four pairs, harness at every pair | Tasks 4 and 5 |
| §4 plugin files on the new images | Task 6 |
| §5 no upgrades behind the operator's back | Task 2's migration 0185 |
| §5 detail page and release notes | Task 7 |
| §6 re-measure | Task 8 |
| Testing table: mixed fleet | Tasks 3 and 5 |
| Testing table: floating tag refuses | Task 3 |
| Testing table: no floating defaults | Task 2 |

**Placeholders.** Some facts were not measured while writing this plan:
- whether 2.9.0 needs a newer Java release;
- whether `engine_mgmt.engines.spec` is nullable;
- whether egret-engine-mgmt-api can depend on egret-perm;
- the exact fixture helpers in each crate's DB tests.

For each, the step names the file to read and what to do with each answer.

**Type consistency.** These names are used identically everywhere:
- `KNOWN_ENGINE_IMAGES`, `DEFAULT_ENGINE_IMAGES`, `stored_engine_image`, `plugin_version_for_image`, `trino_image_ref` (Task 1; used in 2, 3, 7);
- `engine_plugin_version(name, origin, type, declared, spec)` (Task 3; used in 4, 5);
- `EngineType::models` (Task 3; extended in 4, 5);
- `PluginVersion::Ranger2_9_0` and `WirePluginVersion::Ranger2_9_0` (Task 4).

**Order risk.** Tasks 2–5 all touch the perm/eval crates; run them strictly in order. Task 3 leaves Trino 483 and StarRocks 4.1.6 unmodelled on purpose until Tasks 4 and 5 prove them, and each of those tasks inverts a named temporary test.
