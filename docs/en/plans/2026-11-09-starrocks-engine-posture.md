# StarRocks engine posture (package B): implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** close the gaps that let two kinds of StarRocks engines escape egret's enforcement:
- **unbound engines (KI-336):** a StarRocks engine gets a policy profile the way a Trino engine
  does, and an engine left without one says so;
- **legacy engines:** an engine on an unpatched FE image or with a passwordless `root` is flagged,
  and an admin fixes it with one action.

**Spec:** `docs/en/specs/2026-11-09-starrocks-engine-posture-design.md` (and its `docs/vi`
mirror), approved by the user on 2026-11-09. Section 0 below corrects two of its statements
against the code.

**Branch:** `feat/starrocks-engine-posture`, worktree `egret-srb`. Docs go to `egret-data/docs`,
branch `docs/starrocks-engine-posture`.

**Architecture.**
- **Image rule:** `egret-perm::engine_images` holds the one patched-image rule.
- **Backend:** `egret-engine-mgmt` decides the profile at create (`plan_profile`) and computes
  three new DTO fields: `notGoverned`, `securityUpgradeNeeded` and `securityUpgradeReasons`. A new
  module holds the `security-upgrade` route. It reuses `ensure_root_password_secret`, and the
  apply path that `rotate_engine_secrets` uses.
- **Frontend:** reads only the DTO fields.

## 0. Corrections to the spec, checked against `main` 26016f9b1

1. **There is no Trino profile picker in Add engine.** A Trino create auto-creates its profile,
   and `EngineCreate.tsx` has no picker. So the StarRocks picker (Task 5) is a new control, not a
   copy of an existing one. Trino's form is unchanged.
2. **The frontend cannot tell whether Ranger is on.** No endpoint the frontend reads exposes
   `ranger.enabled`. So the backend decides "Not governed" and sends it as the DTO field
   `notGoverned: bool`, true only when `st.ranger_enabled`. It is set in
   `EngineDto::with_runtime`, which already has `CpState`.

3. **The dialog needs the target image before the call.** The DTO gains two more fields (Task 3):
   - `securityUpgradeFeImage`: the FE image the upgrade would set;
   - `securityUpgradeBlockedRelease`: the newer release that keeps the image.

Task 1 updates both spec files (en and vi) to say this.

## Global Constraints

- **Build:** use your own target directory, `CARGO_TARGET_DIR=$PWD/target-srb`.
- **DB tests:** run them with
  `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`.
- **Never run two test suites at once.**
- **Docker and stacks:**
  - never prune Docker;
  - never touch containers you did not start;
  - never run `just up`, `just down` or `just reset-testdata`.
- **Frontend client:** regenerate it only with `npm run gen:api` in `frontend/`. Bare orval drops
  `encodeURIComponent`.
- **Frontend rules:** the frontend follows `docs/en/frontend-rules.md`.
- **Tests:** every behaviour change has a test that fails without it, and a mutant of the change
  that the test kills, recorded in the task report.
- **Migrations:** none is expected. If one is needed, take the next free number above `main` and
  every open PR.
- **Commits:** Conventional Commits, ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Before finishing a task:** `cargo fmt --all -- --check` and `cargo clippy -D warnings` on the
  touched crates are clean, and so are the frontend `npm run lint` and `npm run typecheck` when
  the frontend changed.
- **Permission:** the upgrade route requires `engine:update`. There is no `engine:write`.

---

### Task 1: The patched-image rule moves to `egret-perm` (and the spec corrections)

**Files:**
- `crates/egret-perm/src/engine_images.rs`
- `crates/egret-perm/src/lib.rs` (re-exports)
- `docs/en/specs/2026-11-09-starrocks-engine-posture-design.md` and its `docs/vi` mirror (section
  0's two corrections)

**Produces:**

```rust
/// Whether `image` (a stored FE image ref, tag and/or digest) is an egret FE that closes every
/// hole the StarRocks warning names.
pub fn is_patched_starrocks_fe(image: &str) -> bool;

/// What the security upgrade does with `current`: the default FE image when moving there is
/// not a downgrade, else `Err(release)` naming the newer release (e.g. "4.1.8").
pub fn starrocks_fe_upgrade_target(current: Option<&str>) -> Result<&'static str, String>;
```

**Requirements:**
- **`is_patched_starrocks_fe`** is true when either of these holds:
  - **by digest:** the ref carries `@sha256:<d>`, and `d` equals the digest of a
    `KNOWN_ENGINE_IMAGES` row with repository `ghcr.io/egret-data/egret-starrocks-fe` whose tag is
    at or after the first complete level. This makes a mirrored repository count;
  - **by tag:** the repository is `ghcr.io/egret-data/egret-starrocks-fe`, the tag is
    `<a>.<b>.<c>-egret.<n>` (with or without a digest), and either the release is 4.1.7 with
    n ≥ 4, or the release is later than 4.1.7.

  The first complete level is a constant beside the table:
  `FIRST_COMPLETE_STARROCKS_FE: ((u32, u32, u32), u32) = ((4, 1, 7), 4)`. This is the same rule
  as the frontend's `isPatchedStarRocksImage`. Read it in
  `frontend/src/routes/control-plane/StarRocksMetadataTablesWarning.tsx`; Task 5 removes the
  frontend copy.
- **`starrocks_fe_upgrade_target`:**
  - It parses a release `<a>.<b>.<c>` from the tag of `current`:
    - `starrocks/fe-ubuntu:4.1.6` → 4.1.6;
    - `…egret-starrocks-fe:4.1.7-egret.3` → 4.1.7;
    - a tag like `4.1-latest`, or no tag, has no release.
  - If that release is greater than the release of `DEFAULT_ENGINE_IMAGES.starrocks_fe_image`, it
    returns `Err("<a>.<b>.<c>")`.
  - Otherwise, including when no release can be parsed, it returns
    `Ok(DEFAULT_ENGINE_IMAGES.starrocks_fe_image)`.
- **Unit tests**, in the module's test block:
  - `4.1.6` official: unpatched;
  - `4.1.7-egret.3` with its digest: unpatched;
  - `4.1.7-egret.4` with and without its digest: patched;
  - `4.1.8-egret.1`: patched;
  - `registry.local/mirror/egret-starrocks-fe@sha256:55ec35ec…` (egret.4's digest): patched;
  - the same mirror with egret.3's digest: unpatched;
  - an unknown image: unpatched;
  - upgrade target: 4.1.6 → `Ok(default)`, `4.1-latest` → `Ok(default)`, `starrocks/fe-ubuntu:4.1.8` → `Err("4.1.8")`.
- **Mutants:**
  - drop the digest branch: the mirror test fails;
  - change `>= 4` to `>= 3`: the egret.3 test fails;
  - flip the downgrade comparison: the 4.1.8 test fails.

### Task 2: KI-336 backend, StarRocks gets a profile at create, and `notGoverned`

**Files:**
- `crates/egret-policy-api/src/port.rs`, `crates/egret-policy/src/port_impl.rs`, and every other
  `impl PolicyPort` (`ensure_starrocks_def`);
- `crates/egret-engine-mgmt/src/engines.rs`:
  - `plan_trino_profile` (about line 3262);
  - `bind_trino_profile` (about 3312);
  - `profile_type_conflict`;
  - `create_starrocks_engine` (about 4650; its bind block is about 4843);
  - the late-bind call in `build_provision_req_with` (about 726);
  - `EngineDto` and `with_runtime`;
- the tests beside the existing StarRocks create tests in the same file. They use the no-op
  `EngineProvisioningPort`, about line 10674.

**Produces:**
- `plan_profile(st, name, engine_type: &str, requested) -> Result<ProfilePlan, CpError>`;
- `bind_profile(st, engine_id, name, engine_type: &str, plan, undo)`;
- `EngineDto.not_governed: bool`, serialised as `notGoverned`.

**Requirements:**
- **Rename and generalise.** `plan_trino_profile` becomes `plan_profile` and
  `bind_trino_profile` becomes `bind_profile`, each taking `engine_type`. Trino behaviour is
  byte-for-byte unchanged:
  - the same plans;
  - the `<name>_tag` tag service;
  - the same messages, apart from the type word.
- **A new `PolicyPort` method.** `async fn ensure_starrocks_def(&self) -> Result<(), PolicyError>`
  goes in `crates/egret-policy-api/src/port.rs`, beside `ensure_trino_def`. It is implemented in
  `crates/egret-policy/src/port_impl.rs` by calling `crate::service_def::seed_starrocks(self)`.
  Every other `impl PolicyPort` gets it too; find them with
  `grep -rn "impl.*PolicyPort for" crates`. In a test fake it is `Ok(())`.
- **StarRocks `Create`:**
  - calls `st.authz.ensure_starrocks_def()`;
  - creates the profile `<name>` with type `starrocks` and config `{}`, with **no** tag service;
  - records `undo.created_profile_id`;
  - binds through `bind_profile_guarded`.
- **Conflict messages:** `profile_type_conflict(name, found, expected)` says "not `<expected>`".
- **`create_starrocks_engine`** replaces its existing-profile-only block with
  `plan_profile(&st, &b.name, "starrocks", b.authz_service_id)` and then `bind_profile`.
  - A failure goes through `compensate_failed_create(&st, undo)`, which must delete a profile the
    create made and never one it found.
  - An `authzServiceId` that names no profile keeps today's 400. Do not fall back to `Create`
    for StarRocks; the 400 is the existing contract.
  - Ranger off and no id gives `Unbound`.
- **The late bind on apply** (`build_provision_req_with`, around line 726) is Trino's path, and
  keeps calling with `"trino"`. StarRocks gets no late bind in this package: the warning covers
  it, as spec part 1 says.
- **`notGoverned`** is true when all of these hold:
  - the engine is provisioned or external;
  - its type is `starrocks`;
  - `authz_service_id` is `None`;
  - `st.ranger_enabled`.

  `From<Engine>` sets it to `false`, and `with_runtime(st)` sets it. Every read that builds the
  DTO already calls `with_runtime`; check that both `GET /engines` and `GET /engines/{id}` do.
  Its doc comment says when it is true.
- **DB tests (StarRocks create):**
  - Ranger on, no id: the profile `<name>` (type `starrocks`) is created and bound;
  - Ranger off, no id: unbound;
  - a named existing profile is bound;
  - a same-name profile of type `trino` gets 409 naming "not starrocks";
  - a same-name profile bound to another engine gets 409;
  - a create that fails after the profile step: the profile it made is gone, and a profile it
    found survives.
- **DB or unit tests for `notGoverned`:** the four combinations of (Ranger on/off) × (bound or
  unbound) for StarRocks, plus a Trino engine, which is always false.
- **Mutants:**
  - make `Create` a no-op for StarRocks: the Ranger-on test fails;
  - drop `st.ranger_enabled` from `notGoverned`: the Ranger-off test fails;
  - make compensation delete any profile: the found-profile test fails.

### Task 3: `securityUpgradeNeeded` and `securityUpgradeReasons` on the engine DTO

**Files:** `crates/egret-engine-mgmt/src/engines.rs` (`EngineDto`, `From<Engine>`).

**Consumes:** `egret_perm::is_patched_starrocks_fe` and `egret_perm::starrocks_fe_upgrade_target`, from Task 1.

**Produces:**
- `EngineDto.security_upgrade_needed: bool`;
- `EngineDto.security_upgrade_reasons: Vec<SecurityUpgradeReason>`;
- `#[serde(rename_all = "camelCase")] pub(crate) enum SecurityUpgradeReason { UnpatchedImage, NoRootPassword }`,
  with a `ToSchema` derive;
- a pure helper,
  `pub(crate) fn security_upgrade_reasons(e: &Engine) -> Vec<SecurityUpgradeReason>`;
- `EngineDto.security_upgrade_fe_image: Option<String>`, serialised as `securityUpgradeFeImage`.
  It is `Some(target)` when `UnpatchedImage` is a reason and
  `starrocks_fe_upgrade_target(current)` is `Ok(target)`, and absent otherwise;
- `EngineDto.security_upgrade_blocked_release: Option<String>`, serialised as
  `securityUpgradeBlockedRelease`. It is `Some(release)` when that call is `Err(release)`, and
  absent otherwise.

**Requirements:**
- **The reasons:** only for a provisioned StarRocks engine (`!is_external(e) && e.is_starrocks()`),
  in this order:
  - `UnpatchedImage` when `stored_engine_image("starrocks", &e.spec)` is `None`, or is not
    `is_patched_starrocks_fe`;
  - `NoRootPassword` when `e.root_secret_id.is_none()`.

  Every other engine gets an empty list.
- **`security_upgrade_needed`** is `!reasons.is_empty()`.
- **Doc comments** say what each reason means and that the upgrade action clears it.
- **Unit tests** on `security_upgrade_reasons`. Build the `Engine` the way the existing DTO tests
  do; see the test near line 8210 that reads `securityReapplyNeeded`. Cases:
  - official 4.1.6 with no root secret: both reasons, in order;
  - egret.4 with a root secret: none;
  - egret.4 with no root secret: `NoRootPassword` only;
  - an external StarRocks engine on 4.1.6: none;
  - a Trino engine: none;
  - a serialisation test that the JSON has `securityUpgradeReasons: ["unpatchedImage","noRootPassword"]`;
  - 4.1.6: `securityUpgradeFeImage` is the default FE image, and there is no blocked release;
  - official 4.1.8 with no root secret: `UnpatchedImage` and `NoRootPassword`, no FE target, and
    `securityUpgradeBlockedRelease = "4.1.8"`.
- **Mutants:**
  - swap the order: the order test fails;
  - drop the external check: the external test fails.

### Task 4: `POST /api/control-plane/engines/{id}/security-upgrade`

**Files:**
- create `crates/egret-engine-mgmt/src/security_upgrade.rs`, and register it in the crate's router
  and OpenAPI the way `monitor.rs` and `external.rs` register theirs (`.route(...)` plus the
  `#[utoipa::path]` list);
- make `ensure_root_password_secret` and the apply helpers it needs `pub(crate)` in `engines.rs`;
- tests beside the existing engine route DB tests (`crates/egret-engine-mgmt/tests/routes.rs`, or
  in-module like the create tests; follow whichever covers `rotate_engine_secrets`);
- `openapi.json` (regenerated the way the repo does it; find the command in the `justfile` or CI).

**Consumes:** Tasks 1 and 3.

**Produces:** the route, answering

```json
{
  "engine": "<EngineDto>",
  "imageChanged": true,
  "rootPasswordSet": true,
  "imageKeptBecause": "No patched image for release 4.1.8 yet: the image was not changed."
}
```

`imageKeptBecause` is omitted when the image was not kept for that reason.

**Requirements:**
- **Authz:** `require(&*st.pdp, &p, "engine:update")` first. A principal without it gets 403.
- **Refusals:**
  - 404 for an unknown engine;
  - 409 with `code = engine_is_external` for an external engine, the way `update_engine` does it;
  - 409 for a non-StarRocks engine;
  - 409 when the engine is deleting (the same conflict `rotate_engine_secrets` returns).
- **Steps, in order:**
  1. If `root_secret_id` is `None`, call `ensure_root_password_secret(st, &engine)` and set
     `rootPasswordSet = true`. The function is idempotent: it recovers by name.
  2. If `UnpatchedImage` is a reason:
     - call `starrocks_fe_upgrade_target(current)`;
     - on `Ok(target)`, write `starrocksConfig.feImage = target` and
       `starrocksConfig.cnImage = DEFAULT_ENGINE_IMAGES.starrocks_cn_image`. Use
       `st.engine_mgmt.update_engine(id, &spec, &status)`; a `false` answer, meaning the status
       moved, is the 409 conflict. Set `imageChanged = true`;
     - on `Err(release)`, keep the image and set `imageKeptBecause` to exactly
       `No patched image for release {release} yet: the image was not changed.`
  3. If either step changed something:
     - re-read the engine;
     - build and apply it exactly as `rotate_engine_secrets` does: `build_provision_req_with` …
       `apply_built(…, ApplyOp::Update)`, with the same error mapping;
     - the provisioner then renders `{name}-initpwd-secret`, `{name}-initpwd-shell` and the
       `{name}-initpwd` Job from `StarRocksConfig.root_password`, which `build_starrocks_provision_req`
       already resolves from `root_secret_id`.

     If nothing changed, nothing is applied.
- **Answer:** the re-read engine as a DTO (`with_runtime`), plus the three fields.
- **utoipa:** the description names every status and the restart.
- **DB tests:**
  - **legacy engine** (4.1.6 FE and CN, no root secret): the answer has `imageChanged` and
    `rootPasswordSet`, and the stored spec has both default images and a `root_secret_id`;
    `securityUpgradeNeeded` is false. A second call answers both false and applies nothing.
    Count `apply` calls on a recording fake provisioner;
  - **downgrade guard** (`starrocks/fe-ubuntu:4.1.8`, no root secret): `rootPasswordSet` is
    true, `imageChanged` is false, `imageKeptBecause` is the exact text, and the spec image is
    unchanged;
  - an external engine gets 409 `engine_is_external`, a Trino engine gets 409, and no
    `engine:update` gets 403.
- **Render test:** for a spec with `root_password` set, the provisioner's StarRocks render
  includes the three `initpwd` objects. If such a test already exists, cite it in the report; if
  not, add one in `crates/egret-provisioner/src/starrocks.rs`'s tests.
- **Mutants:**
  - skip step 1: the legacy test fails;
  - always change the image: the downgrade test fails;
  - drop the `engine:update` check: the 403 test fails.

### Task 5: Frontend

**Files:**
- `frontend/src/api/generated/**` (regenerated with `npm run gen:api` after Tasks 2 to 4 change
  `openapi.json`);
- `frontend/src/api/controlPlane.ts` (the hand-written engine type, if it mirrors the DTO; it
  lists `securityReapplyNeeded`);
- `frontend/src/routes/control-plane/EngineCreate.tsx`;
- `ControlPlaneEngines.tsx`, about line 281;
- `EngineDetail.tsx`, about line 478;
- `StarRocksMetadataTablesWarning.tsx`;
- a new `StarRocksPostureWarnings.tsx` with the two warnings and the dialog;
- their tests under `__tests__/`.

**Requirements:**
- **Add engine (StarRocks branch only):**
  - a "Policy profile" select, labelled for screen readers;
  - the default option is "Create a profile named after the engine", which sends no
    `authzServiceId`;
  - the other options are profiles from `listProfiles` with `type === "starrocks"` and no bound
    engines (`engines.length === 0`); choosing one sends its id;
  - the hint under the select says: "Without a profile, egret's grants, masks and row filters do
    not reach this engine." When Ranger is off the backend binds nothing whatever is chosen, so
    the hint does not claim a profile is created.
- **"Not governed":**
  - when `notGoverned`, the engine page shows a warning callout (`WarningCallout`, as
    `StarRocksMetadataTablesWarning` uses) with the spec's text;
  - the list's status cell shows a `StatusBadge` "Not governed", with the warning text as its
    `title`.
- **Security upgrade:**
  - when `securityUpgradeNeeded`, the list shows a `StatusBadge` "Security upgrade needed", like
    "Re-apply needed";
  - the engine page shows a warning naming each reason:
    - `unpatchedImage`: "Its FE image is not egret-starrocks-fe 4.1.7-egret.4 or later, so
      StarRocks lets users past this engine's profile (Iceberg metadata tables, prepared
      statements, the HTTP SQL API, FILES() and more).";
    - `noRootPassword`: "Its FE `root` account has no password: anyone who reaches port 9030
      can log in as root.";
  - the warning has an **Upgrade security** button.
- **The dialog** (`AlertDialog`) lists:
  - when `securityUpgradeFeImage` is set: `FE image: <engine.image> → <securityUpgradeFeImage>`,
    and "The CN image moves to the same release.";
  - when `securityUpgradeBlockedRelease` is set: "The image is not changed: no patched image for
    release <x> yet.";
  - "root password will be set", when `noRootPassword` is a reason.

  It warns: "The FE and CN pods restart. A client outside egret that logs in as root with no
  password is refused afterwards." Confirm calls the route.
  - On success it toasts what changed. When `imageKeptBecause` is set, it shows that text as a
    warning.
  - It invalidates the engine queries.
- **The old warning:** `ProvisionedStarRocksMetadataTablesWarning` reads the DTO and returns
  `null` when the engine carries `securityUpgradeReasons` including `unpatchedImage`, because the
  new warning says it. `isPatchedStarRocksImage` and its regex are deleted. Every remaining caller
  (`grep -rn isPatchedStarRocksImage frontend/src`) reads the DTO instead:
  - **provisioned engines** read `securityUpgradeReasons`;
  - **external engines** keep `StarRocksMetadataTablesWarning`'s unconditional bound-engine
    warning, as today.
- **Vitest:**
  - the picker: the options listed, and the id sent or omitted;
  - "Not governed": shown when the flag is set, absent otherwise;
  - the list badges;
  - the upgrade warning, for each reason;
  - the dialog: confirm calls the route, and the downgrade text is shown;
  - the old warning is hidden when `unpatchedImage` is present.
- **Mutant:** render the old warning unconditionally. The hiding test fails.

### Task 6: Live check on kind (controller)

Run this on a kind cluster that egret provisions into, never one you did not create. Follow
memory `egret-deploy-kind` / `docs/en` deploy notes for the recipe.
1. Create a StarRocks engine through the API with `starrocksConfig.feImage =
   starrocks/fe-ubuntu:4.1.6` and `cnImage = starrocks/cn-ubuntu:4.1.6`.
2. Clear `root_secret_id` on its row and re-apply, so it is a passwordless legacy engine.
   `mysql -u root` with no password must succeed first.
3. Check that the list shows "Security upgrade needed" and the page shows both reasons.
4. Press **Upgrade security**.
5. Check that:
   - `mysql -h <fe> -P 9030 -u root` with no password is refused;
   - a query through egret on that engine still runs;
   - the badge and the warning are gone;
   - the FE image is the default.
6. Also create a StarRocks engine with Ranger on and no profile chosen, and check that the
   profile `<name>` exists and is bound.

Record the commands and outputs in the scratchpad, and their summary in the PR.

### Task 7: Docs (in `egret-data/docs`)

Branch `docs/starrocks-engine-posture`. Each item goes on its existing page; find them with grep.
- **Engine detail:**
  - the "Not governed" warning;
  - the "Security upgrade needed" badge;
  - the upgrade warning, its two reasons, and the **Upgrade security** action: what it changes,
    that the FE and CN restart, the `root` consequence, and the downgrade guard text.
- **Add engine:** the StarRocks "Policy profile" select. With Ranger on and none chosen, a
  profile named after the engine is created.
- **Upgrade notes:** after upgrading egret, look for "Security upgrade needed" in the engine
  list.
- **Known issues:** KI-336 fixed, with the platform PR. KI-340 and KI-345 point to the upgrade
  action for engines on older images.
- **REST API:**
  - `POST /api/control-plane/engines/{id}/security-upgrade`, with its answer and statuses;
  - `notGoverned`, `securityUpgradeNeeded` and `securityUpgradeReasons` on the engine.
- **Screenshots:** the list with the badges, the engine page with the upgrade warning, the
  dialog, and the Add engine picker. These come from a live stack (the controller's step, with
  Task 6).
- Every code fence has a language, and `npm run build` in the docs repo passes.
