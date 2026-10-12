# SP4d-4 Task 13 — the live check: A, C and D on a real StarRocks, B in the browser

> **Measured:** 2026-10-12/13 (UTC), branch `feat/sp4d-4-closing-review`, HEAD `09e0b0a11`,
> worktree `egret-sp4a`. Canonical (English). Nothing was fixed; defects are recorded with evidence.

## 1. The stack (Step 1)

| piece | what ran |
|---|---|
| backend | `target/debug/egret serve` built from this worktree (`cargo build -p egret-bin`, 5 m 14 s) on **:8094**, `EGRET_CATALOG__PUBLIC_URL=http://host.docker.internal:8094` |
| database | **`egret_sp4d4_live`** in `egret-dev-postgres-1`, migrated by the branch binary to **184**; URL passed explicitly; `egret init` bootstrapped admin@egret.local |
| frontend | Vite on **:5294** → :8094 (a throwaway `vite.config.sp4d4live.mts`, deleted afterwards, never committed) |
| engine | `docker run` **`sp4d4-live-sr`** from the local `starrocks/allin1-ubuntu:latest`: `SELECT current_version()` = **4.1.4-4a9848e**, `ranger-plugins-common-2.8.0.jar`. FE query port published as 19030 |
| not run | **Trino was not run** (no local image; not pulled). The Trino-only tag <-> path half of Step 3 is **skipped**. Valkey and RustFS were not needed (server starts without them; storage env vars satisfy config validation). |
| disk | `df -h /` free: 103 GiB before the build, 103 GiB after it, 107 GiB after cleanup |

Untouched: ports 8080/5173/5000, the `egret` dev database, the user's `egret-dev-*` containers. `EGRET_ALLOW_DB_SKIP` never set.

Observation about the harness, not a product defect: **egret has no HTTP access log** (even with `tower_http=debug` no request line appears), so the brief's "egret's access log shows `GET /service/plugins/policies/download/<profile>`" cannot be satisfied. Polling was proven from the FE side instead (see 2).

## 2. Step 2 — C: register the external StarRocks the supported way — PASS with findings

Registry catalog `lake` (kind `generic`, StarRocks `iceberg` hadoop catalog over `file:///tmp/sp4d4wh`) and policy profile `sr4live` (starrocks) were created through the API; **the engine was registered in the browser** (Control Plane -> Engines -> Register external engine: name `sr-external`, type StarRocks, host `host.docker.internal`, port 19030, plugin version `2.8.0` from the dropdown (2.5.0 / 2.8.0 / Other...), profile `sr4live`, catalog `lake` ticked). **No SQL INSERT into `engine_mgmt` or `registry` was used.**

- Redirect landed on `?tab=plugin-config`. Header shows `starrocks`, an **External** badge; tabs are **Overview / Catalogs / Plugin config / Settings** (no pods, logs or monitor tabs). Screenshot `sp4d4-s2-plugin-config.png`: service name, download URL, "anonymous" authentication, declared version 2.8.0, an anonymous-download warning, a "You must supply" list, a "What egret assumes" list (no super-users, no group provider, audit off) and three files each with Copy/Download: `fe.conf (add this line)` = `access_control = ranger`, `ranger-starrocks-security.xml`, `ranger-starrocks-audit.xml`.
- I installed the three files into `/data/deploy/starrocks/fe/conf/` (the fe.conf line appended), restarted the container, created `lake` on the FE with the registry name. FE log: `Start Ranger plugin (starrocks - sr4live) success`, `Switched policy engine to [N]` after each policy change within the 30 s poll interval (versions 1,2,3,4 seen as I wrote). That is the plugin downloading from egret on :8094.
- Attachment row: `declared`, `applied_at` NULL, as designed.

Findings:
1. **Stale Catalogs tab after registration (Minor).** Immediately after the post-registration redirect, clicking the Catalogs tab said "No catalogs are declared on this engine yet" although `engine_catalog_attachments` held `lake / declared` and a fresh page load listed it. A query cache is not invalidated by the registration. Observed once.
2. **Download URL shows `http://localhost:8080` (Info).** It is `catalog.public_url`'s default, so a stack on another port renders a URL that does not reach it. The files are only right when `EGRET_CATALOG__PUBLIC_URL` is set; nothing on the page says the URL is derived from that setting. I restarted egret with it set and used the resulting files.
3. The plugin's `getRolesIfUpdated` logs `ERROR ... service not found 404` every poll (FE log). egret answers the roles endpoint 404 and the plugin ignores it; cosmetic but noisy for an operator reading the FE log.
4. Two more FE-side facts learnt while installing: with `access_control = ranger` a user with no policy is denied even for `_file` / `_pos` hidden Iceberg columns (carol: "SELECT privilege on COLUMN _file"), and `root` is not special (as earlier programmes recorded).

## 3. Step 3 — A: overlapping mask rules on the real engine — PASS with findings

Users `alice bob carol dave` created on the FE. Grant `read lake.sales` for alice, bob.

- Rule A `rule-a-broad`, path `('sales','*','email')`, **Redact (= MASK)**: created in the browser. Preview: "Would recompile sr4live. created 1". Screenshot `sp4d4-s3-ruleA-preview.png`.
- Rule B `rule-b-narrow`, path `('sales','orders','email')`, Nullify (= MASK_NULL): the dialog's **Preview** shows the refusal and the Create button stays disabled. The text names both rules: *"recompiling profile "sr4live" would emit an order-ambiguous pair ... rule "rule-a-broad" and rule "rule-b-narrow" ... Today Ranger serves rule "rule-a-broad" wherever both match"*. The POST route itself answers **409** with the same body (curl). Screenshot `sp4d4-s3-ruleB-409.png`. (The brief expected a 409 toast on Create; the UI stops one step earlier, at Preview. Better, not a defect.)
- Query as alice with A alone: `SELECT email FROM lake.sales.orders` -> `x@x.xxx` for both rows (MASK).
- The permitted SQL write. First attempt, an INSERT into `perm.rules` (kept only to learn what it does): a rule row with no compile does **not** reach the engine and does **not** appear in `GET /perm/overlaps`; it makes any later recompile (an unrelated grant POST) answer 409. I removed it through the UI delete. The bypass that produces a **stored** pair is a policy row, executed verbatim:

```sql
INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by)
VALUES (1, 'rule-b-narrow-handwritten',
 '{"name": "rule-b-narrow-handwritten", "service": "sr4live", "isEnabled": true, "resources": {"table": {"values": ["orders"], "isExcludes": false, "isRecursive": false}, "column": {"values": ["email"], "isExcludes": false, "isRecursive": false}, "catalog": {"values": ["lake"], "isExcludes": false, "isRecursive": false}, "database": {"values": ["sales"], "isExcludes": false, "isRecursive": false}}, "policyType": 1, "description": "", "policyItems": [], "serviceType": "starrocks", "policyLabels": [], "policyPriority": 0, "denyPolicyItems": [], "dataMaskPolicyItems": [{"roles": [], "users": [], "groups": ["public"], "accesses": [{"type": "select", "isAllowed": true}], "dataMaskInfo": {"dataMaskType": "MASK_NULL"}}]}'::jsonb,
 true, 'operator');
UPDATE policy.authz_services SET policy_version = policy_version + 1 WHERE id = 1;
```
  (the UPDATE is what makes the plugin see a new version).
- Live, both policies served (FE logged `Switched policy engine to [4]`): alice and bob on `orders` get `x@x.xxx` (**MASK from A, the broader rule, not NULL from B**), and on `items` also `x@x.xxx`. The evalOrder premise holds on the real 2.8.0 jar, and the banner's claim "today the engine serves rule-a-broad" equals what the engine did. **No difference between claim and engine.**
- `GET /perm/overlaps` and the banner (screenshot `sp4d4-s3-banner.png`, also on the profile page) list the pair: "1 pair of rules overlaps ... rule rule-a-broad and policy rule-b-narrow-handwritten — today the engine serves rule-a-broad", with "How to fix".
- **Contradiction with the brief:** the brief says an unrelated grant write is refused while the pair is stored. The code (overlap.rs, user ruling fix round 2) and the live system say the opposite: a stored pair "blocks nothing", and the unrelated grant POST for carol answered **201**. The banner copy states it ("so they block nothing"). Followed the code; the brief is out of date. A write that CREATES a pair is refused (409, observed with the perm.rules row above).
- B deleted through the UI (Masking tab, type `delete`): policy version 6, banner gone on reload.
- **Tag <-> path half: skipped (no Trino).**

Defect found here (pre-existing, not this package): **D-1, Major — the profile page's Resource column drops the StarRocks `database` level.** `summarize_policy` (`crates/egret-policy/src/admin/mod.rs:173`) joins only `catalog, schema, table, column`, so stored `lake.sales.items.*` is listed as **`lake.items.*`**, rule A as `lake.*.email`, B as `lake.orders.email`. The page names a coordinate that is not the policy's. Screenshot `sp4d4-s3-profile.png`.

## 4. Step 4 — D: a migration that did not fit before — PASS

The brief's "profile bound to the external engine" needed a MANUAL profile, and a profile that has ever been compiled into is `generated` and refuses new hand-written policies (409), even after unbinding. So: profile `sr4batch` with 40 hand-written allow policies (`hw-01-dave-orders` on `lake.sales.orders` for dave, 39 on `t02..t40`), bound to `sr-external` in the browser (Settings tab), the FE pointed at `sr4batch`. The compiler then added 3 compiled policies (43 total).

Finding **D-2 (Major, silent data loss on input):** my first attempt posted Ranger-shaped bodies (`policyItems`) to `POST /api/control-plane/policies`; the route answered 200 and stored every policy with **`policyItems: []`** (the editor form shape is `items` + string accesses). 40 policies were created that grant nothing and the classifier then said "no allow items". No 4xx for an unrecognised body. I deleted that profile and redid it in the form shape.

Migration of `hw-01-dave-orders` through the SP4d-3 page (`/governance/policies/sr4batch/migrate`, screenshots `sp4d4-s4-migrate-page.png`, `-preview.png`, `-migrated.png`):
- Preview (UI): **"Proved equivalent: 6210 probe evaluations agreed ... 0 disagreements; now serves 1 compiled grant policy; no longer serves hw-01-dave-orders"**. Preview request 3.0-3.5 s wall in the UI; API dry-run 2.72-2.87 s (three runs).
- API body of the same dry run: `probes` **6210**, `probes_full` **273240** (44x pruning), `pruning_fallback` absent, `probe_evaluations` 6210.
- Migrate through the UI: **200, committed, equivalent, 3.06 s wall**. Page shows "Migrated". After the poll (version 44) dave's `SELECT id,email,amount FROM lake.sales.orders` returned the same masked rows as before (`x@x.xxx`), as the preview said (equivalent); carol still denied; dave on `items` still denied.
- Undo through the UI: 200, **271 ms**; row back to "Migratable"; dave's query unchanged after the next poll.
- **The page does not show `probes_full` or `pruning_fallback`** (grep: only the generated model and a test reference them; the page prints just "Probes 6210"). The brief expected both on the preview. Minor (the claim is in the API, not the page).
- Near the cap: profile `sr4big` (hand-written policies, then trimmed): 103 policies -> **422 refusal in 0.2 s**: "14985 probe evaluations ... 3086910 units of work ... over the limit of 1600000". 93 and 83 and 78 policies also refused (2.54 M, 2.04 M, 1.81 M units, 0.18-0.2 s each). At **73 policies: 10,935 probes (full 1,727,730, no fallback), dry run 6.98 s, committed migrate 6.72 s**, undo 48 ms. So the planning/proof inside the locked transaction costs about 7 s at the cap, ~3 s at 43 policies; it was never timed before. The refusal itself is cheap (0.2 s). Verdict: acceptable but the migrate holds its profile lock for ~7 s at the cap.
- A 40-policy profile (before: refused by count) fits: 6210 probes x 43 policies.

## 5. Step 5 — B: in the browser — PASS with findings

Profile `sr4b1` (starrocks), editor at `/governance/policies/sr4b1/access/new`.
- **Empty-level hint:** with Database, Table and Column emptied the editor shows under each: "Empty means every database/table/column — saved as *. The policy then covers every ... under the levels above." Screenshot `sp4d4-s5-empty-levels.png`. Measured (1440 px viewport): each hint 704 x 16 px, font 12 px/line-height 16, one line, right edge = parent's right edge (1338), no overflow, 48 px vertical rhythm between hints.
- The path preview line reads `lake.database.table.column` for empty levels (not `*`), while the hint says "saved as *": mild inconsistency (Minor).
- **Root refusal:** empty Catalog -> field turns invalid, `aria-describedby` points at "Name a catalog. It is required: * here would cover every catalog this profile serves.", Save disabled, footer "Name the required resource level (the first one) before saving." API: empty catalog -> **400** "name a catalog — `*` here would cover every catalog this profile serves"; a gap (table without database) -> **400** naming both levels.
- **Select-above-column warning:** `select` is offered only when a Column level exists (StarRocks def); choose `select`, then set level 4 to "— none —": the callout "Stops above column: grants no column reads, only table-level checks such as count(*). Add the column level (use * for every column) to grant SELECT." appears, Save stays enabled (warning, not blocking). Screenshot `sp4d4-s5-select-above-column.png`. Measured: callout 876 x 32.5 px (two lines at 16 px), icon 14x14 at x 498, text 856 px wide, 8 px gap to the next control, no clipping, no document overflow. Saved -> 200, stored with no `column` key.
- **Flagged list on the profile page:** callout "1 warning on 1 stored policy ... Nothing was changed: the rows are flagged, not rewritten", with the policy sentence; `GET /perm/policies/services/5/warnings` -> `warningCount 1`. Screenshot `sp4d4-s5-profile-flagged.png`. Stored body checked in SQL: no `column` key.
- **Live engine, subject dave** (profile bound, FE pointed at `sr4b1`): the table-stopping `select` policy -> `SELECT count(*)`, `SELECT id`, `SELECT *` all **denied** ("SELECT privilege on COLUMN ___count___ / id / _file"): denied, exactly as the warning says for column reads. Control: PUT with the column emptied -> response `filledLevels:["column"]` + note, stored as `*`, and after the poll dave reads `count(*)` = 2 and `id,email` (masked by A).
- **Discrepancy B-1 — the editor callout overstates what survives (Critical by the brief's rule, fail-closed in direction).** The callout says a table-stopping select "grants only ... table-level checks such as `count(*)`". On the real **StarRocks 4.1.4 / 2.8.0** `SELECT count(*)` is **denied** (it checks column `___count___`). The profile page's longer sentence correctly says "Trino's `count(*)`"; the editor's does not name the engine. The engine is stricter than the page claims, so nothing is exposed, but it is a claim the engine contradicts.
- **B-2 (Minor/Major, layout):** at a **390 px** viewport the level row does not stack: the Catalog input is **38 px wide**, and the "Name a catalog..." hint is 38 x 208 px, one word per line, running off the card (screenshot `sp4d4-s5-empty-catalog-390.png`). No document-level horizontal scroll. Desktop is fine.

## 6. Step 6 — whole-table agreement for declared attachments — PASS (trivially, and says so)

```text
origin | apply_status | count
external | declared | 1
(1 row)
total | 1
```
Every `declared` row is on an `external` engine; no external engine holds another status; **total 1**. The database has exactly one engine (`sr-external`, external) and one attachment, and no provisioned engine can exist here (no Kubernetes), so the "no non-external engine holds `declared`" half has no row that could fail. This agrees; it does not discriminate.

## 7. Defects and discrepancies, by severity

| id | sev | what |
|---|---|---|
| B-1 | Critical per the brief's rule (fail-closed, engine stricter) | editor callout says table-stopping select still allows `count(*)`; StarRocks 4.1.4 denies it |
| D-1 | Major (pre-existing) | profile page Resource column omits StarRocks `database`: `lake.items.*` for `lake.sales.items.*` |
| D-2 | Major | `POST /policies` accepts a Ranger-shaped body with 200 and stores it with empty `policyItems` |
| B-2 | Minor | 390 px: level inputs crushed to 38 px, hint one word per line |
| C-1 | Minor | Catalogs tab stale right after registration |
| D-3 | Minor | migration page shows neither `probes_full` nor `pruning_fallback` |
| C-2 | Info | plugin-config URL defaults to `localhost:8080`; depends on `catalog.public_url`, unexplained |
| B-3 | Minor | path preview shows `database`/`table` words for empty levels while the hint says `*` |
| Brief | Info | a stored overlap does not refuse an unrelated grant write (code and live agree; brief wrong); egret has no access log |

No difference was found between a page's claim and the engine for Part A (MASK served, banner correct) or for Part D (preview "equivalent" = unchanged answers).

## 8. Left behind

Stopped and removed: egret (:8094), Vite (:5294), container `sp4d4-live-sr`, the temporary vite config. Left in place: database **`egret_sp4d4_live`** (migration 184, profiles sr4live/sr4batch/sr4big/sr4b1, engine `sr-external`, registry catalog `lake`, rules/grants above) in `egret-dev-postgres-1`; screenshots `sp4d4-*.png` under `/Users/1pro-l4r8/Documents/github/data-portal/.playwright-mcp/`. The StarRocks container's data was discarded with it.
