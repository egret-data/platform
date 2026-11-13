# Governance SP3a — EP framing, per-EP version, capability-matrix image key — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the capability matrix an egret fork-image-level dimension so StarRocks capabilities that depend on the fork patches (a column mask / row filter enforced through every protocol) are gated on the running image, resolve a mixed-engine EP to its least-capable engine (fail-closed), and rename "profile" to "enforcement point" on the user-facing surface.

**Architecture:** The capability matrix (`egret-perm-ir/src/matrix.rs`) already keys rows by `(EpKind, Ranger plugin version)` and `support()` already asks every bound engine. SP3a adds an orthogonal key — the egret image patch level (`egret.N`) — parsed from the engine's image tag, carried on `BoundEngine`/`EpInput`, and consulted for the StarRocks mask/row-filter rows. Mixed-engine resolution changes from "disagree → not measured" to "most restrictive support wins". The rename is user-facing text only; the table `policy.authz_services` and API field names are unchanged.

**Tech Stack:** Rust (`egret-perm-ir`, `egret-perm-compile`, `egret-engine-mgmt`), SQLx, the existing byte-identical corpus + demo-stack differential, the React frontend.

## Global Constraints

- Stay inside the SP3 spec (`docs/en/specs/2026-11-12-gov-sp3-enforcement-points.md`) and the concepts umbrella. On any conflict, the spec governs.
- **Byte-identical gate:** the Ranger JSON the compiler produces on the corpus and the demo-stack profiles must be unchanged after this package (all those engines run the current fork image, which is at or above every enabling level). A diff there fails the package.
- egret image level is **orthogonal** to the Ranger plugin version; do not conflate them. The plugin version stays `BoundEngine.version: Result<String,String>`.
- Fail closed (concepts D7): below an enabling image level, a gated capability lowers to deny, never to "allowed".
- The table stays `policy.authz_services`; **no table rename migration**. API JSON field names (e.g. `profile_id`) are **not** renamed in SP3a (would break clients); only user-visible UI text changes.
- No new data privileges, no catalog-grant merge (those are SP5).
- Commit convention: end commit bodies with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

---

### Task 1: Parse the egret image level from an image tag

**Files:**
- Modify: `crates/egret-engine-mgmt/src/delegation_default.rs` (it already parses StarRocks image tags; add a sibling parser)
- Test: same file's `#[cfg(test)]` module

**Interfaces:**
- Produces: `pub fn starrocks_egret_image_level(image: Option<&str>) -> Option<u32>` — the `N` in a fork image tag `…-egret.N`, else `None` (official image, Trino, no tag, unparseable).

- [ ] **Step 1: Write the failing test**

Add to the test module:

```rust
#[test]
fn egret_image_level_parse() {
    assert_eq!(starrocks_egret_image_level(Some("ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4")), Some(4));
    assert_eq!(starrocks_egret_image_level(Some("ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.5@sha256:abc")), Some(5));
    assert_eq!(starrocks_egret_image_level(Some("starrocks/allin1-ubuntu:4.1.7")), None); // official, no fork suffix
    assert_eq!(starrocks_egret_image_level(Some("starrocks/fe-ubuntu:4.1.7-egret.notanumber")), None);
    assert_eq!(starrocks_egret_image_level(None), None);
}
```

- [ ] **Step 2: Run it, expect failure**

Run: `cargo test -p egret-engine-mgmt delegation_default::tests::egret_image_level_parse`
Expected: FAIL (function not found).

- [ ] **Step 3: Implement**

Mirror the existing tag-splitting in `starrocks_image_is_pre_3_5` (strip a `@sha256:` digest, take the part after the last `:` as the tag):

```rust
/// The egret fork patch level `N` in an image tag `…-egret.N`, else `None`
/// (an official image, a Trino engine, no tag, or an unparseable suffix).
pub fn starrocks_egret_image_level(image: Option<&str>) -> Option<u32> {
    let image = image?;
    let image = image.split('@').next().unwrap_or(image); // drop a @sha256: digest
    let tag = image.rsplit_once(':').map(|(_, t)| t).unwrap_or(image);
    let suffix = tag.rsplit_once("-egret.").map(|(_, s)| s)?;
    suffix.parse::<u32>().ok()
}
```

- [ ] **Step 4: Run it, expect pass**

Run: `cargo test -p egret-engine-mgmt delegation_default::tests::egret_image_level_parse`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-engine-mgmt/src/delegation_default.rs
git commit -m "feat(engine-mgmt): parse the egret fork image level from an image tag

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Carry the image level on the IR's EP input

**Files:**
- Modify: `crates/egret-perm-ir/src/ep.rs` (`BoundEngine`, `EpInput`)
- Modify: `crates/egret-perm-compile/src/ir_load.rs` (`bound_engines`, the initial `EpInput`)
- Test: `crates/egret-perm-ir/src/ep.rs` (a construction test) and the existing `ir_load` tests still compile

**Interfaces:**
- Consumes: `starrocks_egret_image_level` (Task 1).
- Produces: `BoundEngine.egret_image_level: Option<u32>`; `EpInput.default_image_level: Option<u32>`.

- [ ] **Step 1: Add the fields**

In `ep.rs`, add to `BoundEngine`:

```rust
pub struct BoundEngine {
    pub name: String,
    pub version: Result<String, String>,
    /// The egret fork patch level `N` of the engine's image (`…-egret.N`), or
    /// `None` for an official image / Trino. Orthogonal to `version`.
    pub egret_image_level: Option<u32>,
}
```

and to `EpInput`, next to `default_version`:

```rust
    /// The egret image level egret's default image runs, used when `engines` is empty.
    pub default_image_level: Option<u32>,
```

- [ ] **Step 2: Fix the initial `EpInput` and populate from the DB**

In `ir_load.rs`, the initial `EpInput` (around the `default_version: Err(NOT_READ.to_string())` line) gains `default_image_level: None`.

In `bound_engines`, the `SELECT` already returns `spec`. Extract the engine's StarRocks image tag from `spec` the same way `egret-engine-mgmt`'s delegation default does — find the existing extraction by grepping for `sr_image` / the `spec` image accessor on `crate::routes::BoundEngine`, and reuse it. Then set the field:

```rust
    ep.engines = bound
        .iter()
        .map(|e| BoundEngine {
            name: e.name.clone(),
            version: e.plugin_version().map(|v| plugin_version_label(v).to_string()),
            egret_image_level: egret_engine_mgmt::delegation_default::starrocks_egret_image_level(
                e.image_tag().as_deref(), // the accessor used for sr_image; confirm its name
            ),
        })
        .collect();
    ep.default_image_level = None; // egret's default image level is not tracked per type yet; None = not image-gated
```

(If `crate::routes::BoundEngine` has no image accessor, add one that reads the tag out of `spec`, mirroring how `sr_image` is obtained elsewhere; keep it a one-line getter.)

- [ ] **Step 3: Update every other `BoundEngine`/`EpInput` constructor**

Run `cargo build -p egret-perm-ir -p egret-perm-compile` and fix every constructor the new fields break (test fixtures included) by adding `egret_image_level: None` / `default_image_level: None`. These are mechanical.

- [ ] **Step 4: Build**

Run: `cargo build -p egret-perm-ir -p egret-perm-compile`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-perm-ir/src/ep.rs crates/egret-perm-compile/src/ir_load.rs
git commit -m "feat(perm-ir): carry the egret image level on the EP input

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Gate StarRocks mask/row-filter on the image level; mixed-engine = least capable

**Files:**
- Modify: `crates/egret-perm-ir/src/matrix.rs` (`Entry`, `support_for_kind`, `support`)
- Modify: `crates/egret-perm-ir/src/enforcement.rs` (the two `support_for_kind` call sites)
- Test: `crates/egret-perm-ir/src/matrix.rs` test module

**Interfaces:**
- Consumes: `EpInput.engines[].egret_image_level`, `EpInput.default_image_level` (Task 2).
- Produces: `support_for_kind(kind, version, image_level: Option<u32>, classification_channel, cap)` (new 3rd param); `support(ep, cap)` unchanged signature; a new const `STARROCKS_MASK_MIN_EGRET_IMAGE`.

- [ ] **Step 1: Add the enabling-level constant and the Entry field**

In `matrix.rs`, add the constant (pin it to the fork level where patches 0002 PREPARE, 0003 hidden columns and 0004 HTTP/Arrow are all present; per the fork README's patch-level table that is `egret.3`, and the current default `egret.4` satisfies it — **verify against `egret-starrocks/README.md` + `patches/` when implementing**):

```rust
/// The StarRocks fork image level at which a column mask and a row filter are
/// enforced through every protocol (patches 0002/0003/0004). Below it, egret
/// cannot rely on the engine to apply them, so the matrix lowers them to deny.
pub const STARROCKS_MASK_MIN_EGRET_IMAGE: u32 = 3;
```

Add to `Entry` a field (default `None` = not image-gated):

```rust
    /// The minimum egret image level this row needs; `None` = any image.
    min_egret_image: Option<u32>,
```

- [ ] **Step 2: Set it on the StarRocks mask/row-filter rows; default the rest**

Every `Entry` literal in `ENTRIES` gains `min_egret_image: None` except the StarRocks rows whose capability is a column mask or a row filter (the `STARROCKS` rows keyed on `Capability::ClassificationSelector(RuleEffectKind::Mask)`, `Capability::ClassificationSelector(RuleEffectKind::RowFilter)` and `Capability::MaskType(_)` via `Key::MaskTypes(STARROCKS_MASK_TYPES)`), which get `min_egret_image: Some(STARROCKS_MASK_MIN_EGRET_IMAGE)`.

- [ ] **Step 3: Write the failing tests**

```rust
#[test]
fn starrocks_mask_needs_the_fork_image_level() {
    let cap = Capability::ClassificationSelector(RuleEffectKind::Mask);
    // At or above the enabling level: the normal (lowered-per-column) answer.
    assert_eq!(
        support_for_kind(starrocks(), None, Some(STARROCKS_MASK_MIN_EGRET_IMAGE), false, &cap),
        Support::Lowered(LoweringKind::ExpandPerColumn)
    );
    // Below it (official image, or an older fork): deny, reason not measured.
    assert_eq!(
        support_for_kind(starrocks(), None, Some(0), false, &cap),
        Support::Missing(ReasonCode::NotMeasured)
    );
    assert_eq!(
        support_for_kind(starrocks(), None, None, false, &cap),
        Support::Missing(ReasonCode::NotMeasured)
    );
}

#[test]
fn a_non_mask_starrocks_row_ignores_the_image_level() {
    // impersonate / session caps are not image-gated: None image is fine.
    let cap = Capability::WithholdByClassification;
    assert_eq!(
        support_for_kind(starrocks(), None, None, false, &cap),
        support_for_kind(starrocks(), None, Some(99), false, &cap),
    );
}

#[test]
fn mixed_engine_takes_the_most_restrictive() {
    use crate::ep::{BoundEngine, EpInput, EpKind};
    let cap = Capability::ClassificationSelector(RuleEffectKind::Mask);
    let ep = EpInput {
        profile_id: 1, profile_name: "p".into(),
        kind: EpKind { engine: EngineType::StarRocks, mechanism: Mechanism::Ranger },
        engines: vec![
            BoundEngine { name: "new".into(), version: Ok("2.8.0".into()), egret_image_level: Some(STARROCKS_MASK_MIN_EGRET_IMAGE) },
            BoundEngine { name: "old".into(), version: Ok("2.8.0".into()), egret_image_level: Some(0) },
        ],
        default_version: Err("x".into()), default_image_level: None,
        classification_channel: false, catalogs: Default::default(),
    };
    // One engine can't enforce the mask → the whole EP lowers to deny.
    assert_eq!(support(&ep, &cap), Support::Missing(ReasonCode::NotMeasured));
}
```

- [ ] **Step 4: Run them, expect failure**

Run: `cargo test -p egret-perm-ir matrix::tests`
Expected: FAIL (signature mismatch / no image gate yet).

- [ ] **Step 5: Implement the gate in `support_for_kind`**

Add the `image_level: Option<u32>` parameter after `version`. After finding the matched `Entry e`, before returning its answer, apply the gate:

```rust
    if let Some(min) = e.min_egret_image {
        if image_level.map_or(true, |lvl| lvl < min) {
            return Support::Missing(ReasonCode::NotMeasured);
        }
    }
```

- [ ] **Step 6: Implement least-capable in `support`**

Replace the per-engine fold. Collect `(version, image_level)` per engine (or the defaults when `engines` is empty), call `support_for_kind` for each, and keep the **most restrictive** answer. Define the order Native < Lowered < Missing; at the same tier, two different answers stay a disagreement:

```rust
fn restrictiveness(s: &Support) -> u8 {
    match s { Support::Native => 0, Support::Lowered(_) => 1, Support::Missing(_) => 2 }
}

pub fn support(ep: &EpInput, cap: &Capability) -> Support {
    let pairs: Vec<(Option<&str>, Option<u32>)> = if ep.engines.is_empty() {
        vec![(ep.default_version.as_deref().ok(), ep.default_image_level)]
    } else {
        ep.engines.iter().map(|e| (e.version.as_deref().ok(), e.egret_image_level)).collect()
    };
    let mut chosen: Option<Support> = None;
    for (v, lvl) in pairs {
        let s = support_for_kind(ep.kind, v, lvl, ep.classification_channel, cap);
        chosen = Some(match chosen {
            None => s,
            Some(c) => match restrictiveness(&s).cmp(&restrictiveness(&c)) {
                std::cmp::Ordering::Greater => s,          // more restrictive wins
                std::cmp::Ordering::Less => c,
                std::cmp::Ordering::Equal if s == c => c,  // agree
                std::cmp::Ordering::Equal => Support::Missing(ReasonCode::VersionsDisagree), // same tier, differ
            },
        });
    }
    chosen.unwrap_or(Support::Missing(ReasonCode::NotMeasured))
}
```

- [ ] **Step 7: Fix the `enforcement.rs` call sites**

`enforcement.rs:59` and `:153` call `support_for_kind`. Pass the image level: at `:59`, thread the engine's level from the same `EpInput`/engine the surrounding code already has; at `:153` (the classification-channel probe, which uses `None` version) pass `None` for the image level (it is not an image-gated capability). Build to find the exact locals.

- [ ] **Step 8: Run the suite**

Run: `cargo test -p egret-perm-ir`
Expected: PASS, including the three new tests and the existing matrix/enforcement tests (update any existing `support_for_kind(...)` test call to pass the new `image_level` arg — the StarRocks mask tests should pass `Some(STARROCKS_MASK_MIN_EGRET_IMAGE)` to keep their current expectation).

- [ ] **Step 9: Commit**

```bash
git add crates/egret-perm-ir/src/matrix.rs crates/egret-perm-ir/src/enforcement.rs
git commit -m "feat(perm-ir): gate StarRocks mask/row filter on the fork image level; mixed EP = least capable

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3B (inserted 2026-11-12): Wire the image gate into the compile + enforcement paths

Task 3 added the matrix image dimension but `support(ep, cap)` (the only function reading the real per-engine image level) has no production caller — every compile/enforcement site hardcodes `Some(STARROCKS_MASK_MIN_EGRET_IMAGE)`, so the gate is inert (a below-level StarRocks EP is not denied), which fails this package's "Done when". This task wires the real **effective image level** (most restrictive across the EP's engines) into the lowering path (`lower_rules` → `lower_rule`/`ensure_mask_type_declared`/`needs_tagged_tables`, via a new `EpInput::effective_image_level`) and onto `AttachedEngine` (so `enforces`/the NotEnforced reason matches the output). Byte-identical on fork images must still hold (corpus/demo are egret.4 ≥ 3). Full steps + code: the task-3b brief in this plan's SDD workspace.

**Done when:** a StarRocks EP whose engine runs below egret.3 lowers its mask/row-filter to deny in the compiled output AND in the NotEnforced reason; at/above egret.3 the output is byte-identical to today; `support(ep, cap)` (or the effective level) is consulted by production code.

### Task 4: Prove byte-identical on the corpus and demo stack

**Files:**
- Test only (the existing byte-identical / golden tests)

**Interfaces:**
- Consumes: Tasks 1–3.

- [ ] **Step 1: Find the byte-identical test**

The SP1 byte-identical gate blesses the Ranger JSON corpus (search `crates/egret-perm-compile/tests` for the golden/byte-identical test, e.g. `grep -rn "byte-identical\|golden\|G17\|bless" crates/egret-perm-compile/tests`).

- [ ] **Step 2: Run it**

Run the corpus byte-identical test (the command the test file documents).
Expected: PASS with **no** diff — every corpus/demo engine is on a fork image at or above `STARROCKS_MASK_MIN_EGRET_IMAGE`, so no capability flips. If it diffs, a corpus engine is below the level or carries no image tag; investigate (the corpus fixture must set an image level ≥ the enabling level for StarRocks, matching the real demo stack). Fixing the fixture's image level is in scope; changing the enabling constant to force a pass is not.

- [ ] **Step 3: Commit (if the corpus fixture needed an image level set)**

```bash
git add crates/egret-perm-compile/tests
git commit -m "test(perm-compile): corpus fixtures carry the fork image level (byte-identical holds)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Rename "profile" to "enforcement point" on the user-facing surface

**Files:**
- Modify: frontend user-visible strings (labels, headings, help text) that say "profile"/"Profile" for a governance authz profile
- Modify: user-facing API `message`/`description`/doc strings that say "profile" (NOT JSON field names)

**Interfaces:** none (text only).

- [ ] **Step 1: Inventory the surface strings**

```bash
grep -rniE 'profile' frontend/src --include=*.tsx --include=*.ts | grep -iE 'governance|authz|enforcement|engine|ranger|opa' | grep -viE 'user profile|profile picture'
grep -rniE '"[^"]*profile[^"]*"' crates --include=*.rs | grep -iE 'message|description|warn|error' | grep -iE 'authz|governance|ranger|service'
```

List the hits that mean "the governance authz profile" (exclude user profiles, profile pictures, unrelated uses).

- [ ] **Step 2: Rename the user-visible text**

Change each identified **display string** from "profile" to "enforcement point" (and "Profile" → "Enforcement point"). Do NOT rename: the DB table, Rust/TS struct field names, API JSON keys (`profile_id`, `profileName`, …), route paths, or test identifiers — SP3a is surface text only (SP3 spec).

- [ ] **Step 3: Build the frontend and typecheck**

Run: `cd frontend && npm run build` (or the repo's typecheck/lint command).
Expected: builds; no field renamed by accident (only string literals changed).

- [ ] **Step 4: Live check (screenshot)**

Start a stack, open the governance/engines page that showed "Profile", confirm it reads "Enforcement point". Save a screenshot under `/Users/1pro-l4r8/Documents/github/data-portal/` per the house rule. (If a stack is not available in this run, note it for the docs/live-check pass.)

- [ ] **Step 5: Commit**

```bash
git add frontend crates
git commit -m "feat(ui): call the governance profile an enforcement point

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Out of scope (later SP3 packages / SP5)

- Removing hand-written operator policies and the Migrate page (SP3b).
- clearance→withhold, role→groups, implicit→explicit workspace grant (SP3c).
- The per-EP migration gate and switching the demo stack (SP3d).
- The full privilege vocabulary, catalog-grant merge, iceberg-rest EP (SP5).
- Renaming the `policy.authz_services` table or any API field (deliberately not done).
- Tracking egret's default image level per engine type (`default_image_level` stays `None` until a later package needs it).
