# Engine upgrade: Trino 483, StarRocks 4.1.6, and the Ranger plugins they ship

**Status:** design, drafted 2026-10-13 (awaiting the user's review)
**Programme:** view governance under clearance. This is spec 1 of 2; spec 2 governs views and is written
against the versions this spec lands, using the re-measurement in its last task.
**Depends on (all merged):** SP4d-4 (#664, `f5bbbd879`): modelled `(type, version)` pairs, external
engines, the one plugin-config renderer.
**Measurements this rests on:** `docs/en/plans/2026-10-13-view-clearance-measurement.md`
(rounds 1 and 2, live Trino 470 / StarRocks 4.1.4 / StarRocks 4.1.6).

## Why this exists

The user asked on 2026-10-13 to move Trino, StarRocks and Ranger to their latest versions, after the
view measurement found that StarRocks 4.1.4 lets a user with **no grant at all** read any plain view
(round 1, F-1) and that 4.1.6 fixes it (round 2, B).

egret **is** the Ranger Admin. There is no Ranger server to upgrade. Each engine ships its own
`ranger-plugins-common`, downloads policy from egret, and enforces it. So "upgrade Ranger" means: run
the engine versions that ship the newer plugins, and make egret's evaluator (`egret-ranger-eval`)
model those plugin versions. The evaluator backs `explain` and the SP4d-3/SP4d-4 migration gate.

### User rulings (binding)

| Question | Ruling (2026-10-13) |
|---|---|
| Trino 483 ships Ranger 2.8.0, not 2.9.0 | **Use the plugin each engine ships.** No custom Trino image with a swapped jar. |
| One spec or two | **Two.** Upgrade first, view governance second, written against the upgraded versions. |
| How egret knows an engine's plugin version | **An exact-image table**, approved with this design: floating tags and unknown images are unmodelled (fail-closed). |

## What is already true, measured before designing

### The latest versions (Docker Hub and Maven Central, 2026-10-13)

| | egret today | latest | `ranger-plugins-common` in the latest |
|---|---|---|---|
| Trino | `470` (`DEFAULT_IMAGE_VERSION`, `crates/egret-engine-mgmt/src/engines.rs:47`) | `483`, digest `sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534` | **2.8.0** (`/usr/lib/trino/plugin/ranger/org.apache.ranger_ranger-plugins-common-2.8.0.jar`, read from the image) |
| StarRocks FE | `starrocks/fe-ubuntu:4.1-latest` (`engines.rs:59`) | `4.1.6`, digest `sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31` | **2.9.0** (round 2, `javap` and the FE's lib directory) |
| StarRocks CN | `starrocks/cn-ubuntu:4.1-latest` (`engines.rs:60`) | `4.1.6`, digest `sha256:bf0f5ca7fd9fc4e568d4d0e593e362f229b376976b513a5a549b1b922d9ec23b` | n/a (the plugin runs in the FE) |
| StarRocks all-in-one (dev, tests) | `latest` (`deploy/docker-compose.dev.yml:74`) | `4.1.6`, digest `sha256:5cbf09c4c788d2ca41a8e38599b650f04dbc6af0de51ebda0952770fb8333e5d` | 2.9.0 |
| Apache Ranger | n/a | `2.9.0` (Maven Central lists 2.4.0 … 2.9.0) | |

No StarRocks 4.1.5 or 4.2 image exists on Docker Hub as of 2026-10-13.

### The premise behind egret's plugin version has expired

`EngineType::plugin_version` (`crates/egret-ranger-eval/src/eval.rs:662`) maps an engine **type** to a
plugin version: Trino → 2.5.0, StarRocks → 2.8.0. Its doc comment records the measurement it rests on:
"`starrocks/fe-ubuntu:4.1-latest` ships 2.8.0", taken 2026-09-28. `4.1-latest` is a floating tag and
now points at 4.1.6, which ships **2.9.0**.

`engine_plugin_version` (`crates/egret-perm-compile/src/routes.rs:2283`) is the one place egret derives
an engine's plugin version. For a provisioned engine it uses the type alone. So:

- Any StarRocks engine egret provisioned or restarted since 4.1.6 was published (2026-10-01) may run
  2.9.0, while `explain` and the migration gate evaluate it as 2.8.0. No measurement says they agree.
- Once the Trino default moves to 483, an engine still on 470 would be evaluated as 2.8.0 while it
  runs 2.5.0. A type-only mapping is wrong as soon as two versions of one engine type coexist.

This is a defect in today's main, independent of the upgrade. The upgrade cannot land without
fixing it.

### What changes in StarRocks 4.1.6 that users will notice

Round 2 B measured, on live 4.1.4 and 4.1.6:

- 4.1.4 never asks the plugin a `select` question about a view. A user with no grant reads any plain
  view. **4.1.6 asks, and denies** (StarRocks commit `e0d97ad2659`, #56316, backported as
  `2a36c2d0715`).
- 4.1.4 accepts `SECURITY INVOKER` and does not enforce it. 4.1.6 re-checks the base tables as the
  reader.
- Both versions: a base-column **deny** still does not apply through a plain view, a base-column
  **mask** does (reader identity), and a materialized view carries neither. These are spec 2's
  problem.

So on 4.1.6, a user who today reads views without any grant will start getting denials. That is the
fix working, but it is a visible behaviour change, and operators must be told before they upgrade.

## Design

### 1. Target versions, pinned exactly

- Defaults move to `trinodb/trino:483` and `starrocks/{fe,cn}-ubuntu:4.1.6`, each pinned **with its
  digest** (`repo:tag@sha256:…`). No egret default may name a floating tag (`latest`, `*-latest`).
- The same pins apply to:
  - the default StarRocks config in `crates/egret-engine-mgmt-api/src/lib.rs` (`3.3-latest` today);
  - `deploy/docker-compose.dev.yml` (`trinodb/trino:latest`, `starrocks/allin1-ubuntu:latest`);
  - the live-test fixtures that name an image.

  `crates/egret-workflow/oracle-exec` already pins Trino 483. Its StarRocks pin moves to 4.1.6.
- A test fails if any egret default or the dev compose names a floating tag. It is a grep over the
  constants and the compose file, written so it cannot pass vacuously: it must find the expected
  number of pins.

### 2. One derivation of an engine's plugin version, from its exact image

A new table, `KNOWN_ENGINE_IMAGES`, lives next to `MODELLED_RANGER_PLUGINS` in
`crates/egret-perm/src/actions.rs`. It maps `(engine type, image repository, exact tag)` to a plugin
version:

| type | repository | tag | plugin |
|---|---|---|---|
| trino | `trinodb/trino` | `470` | 2.5.0 |
| trino | `trinodb/trino` | `483` | 2.8.0 |
| starrocks | `starrocks/fe-ubuntu` | `4.1.4` | 2.8.0 |
| starrocks | `starrocks/fe-ubuntu` | `4.1.6` | 2.9.0 |

Each row carries its digest.

`engine_plugin_version` resolves a provisioned engine through this table:

- **The image comes from the engine's stored image reference.** For Trino that is `image_version`,
  which the provisioner composes into `trinodb/trino:{v}`. For StarRocks it is the FE image.
- **A digest suffix is checked.** If the stored reference carries a digest, it must equal the table's
  digest for that tag.
- **Anything else is unmodelled:**
  - a floating tag;
  - an unknown tag;
  - another repository or a private mirror;
  - a digest mismatch.

  The result is a sentence that names the engine, its image and the fix ("pin the engine to one of:
  …"). `explain` and the migration gate already refuse on that sentence for unmodelled external
  engines (SP4d-4 Task 7), so a provisioned engine takes the same path.

`EngineType::plugin_version` is **removed**, so that no caller can derive a version from the type
alone. Nothing else may map an engine to a plugin version.

External engines are unchanged: their operator declares a version. They now pick from the four
modelled pairs.

### 3. The evaluator models 2.9.0, and Trino at 2.8.0

- `PluginVersion` gains `Ranger2_9_0`. `WirePluginVersion`, `plugin_version_label` and the OpenAPI
  enum follow, and the client is regenerated with `npm run gen:api`.
- `MODELLED_RANGER_PLUGINS` becomes four pairs:
  - Trino@2.5.0
  - Trino@2.8.0
  - StarRocks@2.8.0
  - StarRocks@2.9.0

  The external-engine version picker lists them.
- **Read before modelling.** Disassemble `ranger-plugins-common-2.9.0.jar` with `javap -c` and
  compare it with 2.8.0 on every path the crate models today. That is every path the 2.5.0 → 2.8.0
  comparison covered (the `PluginVersion` doc comment lists them), plus the paths SP4d-2b and SP4d-4
  added:
  - the policy evalOrder;
  - the resource trie and `may_be_candidate`;
  - the tag tiers and tag match types;
  - mask and row-filter arbitration;
  - `isSuperUser`.

  Record each comparison in `docs/en/plans/2026-10-13-ranger-2-9-0-reading.md`.
  - A byte-identical path needs no code.
  - A path that differs is either modelled under `Ranger2_9_0` with its own test, or refused for
    2.9.0 with a named reason.
  - Nothing is assumed identical without the reading.
- **Trino@2.8.0 is a new pair, not a free one.** StarRocks@2.8.0 was measured with the StarRocks
  service-def. Trino@2.8.0 combines the Trino service-def (levels, access types, the
  `trinouser`/`sysinfo`/`function`/`procedure` resources) with the 2.8.0 jar. It is modelled only
  once the real-jar differential passes with the Trino service-def on 2.8.0.
- **The real-jar harness runs every pair.** `tests/ranger/pom.xml` already parameterises
  `${ranger.version}`; it gains 2.9.0. Every differential test in `egret-ranger-eval` that runs today
  at 2.5.0 and 2.8.0 runs at all four pairs: the corpus, the tag tiers, the mask/row-filter
  arbitration, the trie candidacy, the editor levels, the migration corpus, and the dropped-probe
  oracle. `just test-ranger-gates` keeps its list prechecks for each.
- **The bar is unchanged: zero disagreements at every pair.** One disagreement means the pair is not
  modelled. The task then stops and reports. It must not narrow the corpus or weaken an assertion.

### 4. Plugin configuration on the new engines

The one renderer (`crates/egret-engine-mgmt-api/src/ranger_config.rs`) and the provisioners' Trino
`access-control.properties` and StarRocks XMLs stay as they are **unless measurement shows a change**.

The upgrade task starts both new images with today's rendered files and records:
- whether each file loads;
- whether the plugin polls egret;
- whether a known allow and a known deny decide as expected.

A key that was renamed or removed between 470 → 483 or 4.1.4 → 4.1.6 is fixed in the renderer, with
its golden test updated. If the old and new engine versions need different files, the renderer takes
the engine's modelled version as input. One renderer, never two.

### 5. Existing engines are not upgraded behind the operator's back

- egret does not touch a running engine's image.
- A Trino engine stored at `470` stays modelled at 2.5.0.
- A StarRocks engine stored at a floating tag becomes **unmodelled** the moment this lands: `explain`
  and the migration gate refuse for its profile, with the sentence from §2. The engine itself keeps
  serving, and its enforcement is unchanged. Only egret's claims about it stop.
- The operator upgrades with the existing `PUT /api/control-plane/engines/{id}` (`imageVersion` /
  `starrocksConfig.feImage` / `cnImage`). The version is re-derived from the new image.
- The engine detail page shows the engine's image and the result of the lookup: a modelled plugin
  version, or the unmodelled sentence. The operator can see why `explain` refuses before they hit it.
- Release notes, in the PR body and in `docs/en/`, state three things:
  - the 4.1.6 view behaviour change (§ "What changes");
  - that floating-tag StarRocks engines become unmodelled;
  - how to pin.

### 6. The last task: re-measure on the new versions

Rerun the view matrix of rounds 1 and 2 on Trino 483 and StarRocks 4.1.6, every cell, predictions
first. Rerun the SP4d-4 live-check essentials on both new engines:
- overlap refusal and the broader rule winning;
- the editor's table-stopping `select` denied at column;
- external registration with plugin polling;
- one pruned migration whose preview matches the engine.

The result doc is the input to spec 2. A cell that changed from the old versions is reported as a
change, never folded into "as before".

## Testing and evidence

| What | How |
|---|---|
| Plugin version derivation | Unit tests over `KNOWN_ENGINE_IMAGES`. Each known row resolves. Each of the following is unmodelled, with the sentence naming the fix: a floating tag, an unknown tag, another repository, and a digest mismatch. Also test that `EngineType::plugin_version` is gone: a compile-fail doc test, or simply no symbol. |
| No floating defaults | A test over the default constants and `deploy/docker-compose.dev.yml` that counts its expected pins. |
| 2.9.0 fidelity | The jar reading doc, plus every real-jar differential at four pairs with 0 disagreements, run through `just test-ranger-gates`. |
| Gates on mixed fleets | DB tests: a profile bound to a Trino@470 engine and a Trino@483 engine proves at both versions (the gate already evaluates per bound engine). A floating-tag StarRocks engine makes `explain` and migrate refuse with the sentence. |
| Real engines | §4 config load and polling on both new images, and §6 on live engines run by docker on a throwaway DB, as SP4d-4 Task 13 did. |

## Out of scope

- View governance under clearance: spec 2.
- Custom engine images, private registry mirrors, and an operator override of the plugin version for
  a provisioned engine. These are unmodelled by design. If a user needs a mirror, it is a later
  table entry with its own measurement.
- Upgrading running engines automatically.
- Retiring Trino@2.5.0 or StarRocks@2.8.0 while engines still run them.
- Trino with Ranger 2.9.0 (user ruling).

## Risks

- **2.9.0 may differ on a modelled path.** The rule in §3 handles it: the pair is modelled only when
  read and proved, otherwise refused. The upgrade can still land, with StarRocks@2.9.0 unmodelled,
  if the user accepts that for a while. That would be a user decision at the time, not a default.
- **Trino 483 may change behaviour that egret's compiled policies rely on**, for example how
  `trinouser`, `sysinfo` or `function` resources are checked. §6's live check and the Trino@2.8.0
  differential are where this surfaces.
- **Floating-tag StarRocks engines become unmodelled** on landing. That is deliberate: they are
  already evaluated at a version nobody measured. The page and the release notes make the fix
  one PUT.
