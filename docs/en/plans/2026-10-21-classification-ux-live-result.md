# Classification UX package: live walkthrough result (Task 12b)

Date: 2026-10-22. Branch `feat/classification-ux`, HEAD `9a1bc3ea` (origin/main plus the Classification UX package).
Spec: [`../specs/2026-10-21-classification-ux-design.md`](../specs/2026-10-21-classification-ux-design.md). Plan: [`2026-10-21-classification-ux.md`](2026-10-21-classification-ux.md).
Baseline: the pass-2 walkthrough (N1..N26) that the spec cites.

Every statement is marked **OBSERVED** (seen in the browser, in a DOM read through `getComputedStyle` or `getBoundingClientRect`, or in a network log) or **INFERRED** (reasoned, not seen). Screenshot names (`ux3-*.png`) refer to files kept outside git.

## 1. Environment

| Item | Value |
|---|---|
| Stack | The repository's demo recipe (`deploy/docker-compose.demo.yml`), built from this worktree, Compose project `egret-classux` |
| Address | `http://egret.localtest.me:18280` (port 18280 was free). Mailpit on `127.0.0.1:18225`, because 8025 belongs to another demo stack. |
| Images | `egret-backend:classux` (target `demo`), `egret-frontend:classux`; the renderer reuses the existing `egret-demo-renderer:dev` image without a rebuild. Trino 483, Keycloak 26.0, Postgres 16, RustFS 1.0.1 (pinned in the compose file). |
| Tooling | Docker 28.4.0, Compose v2.39.4. Browser: the Playwright MCP Chromium at 1440x900 and 390x844. |
| Logins | `admin@egret.local` (break-glass), signed in once. `member-sso` (SSO) signed in once, before the admin, for the 403 check. |
| Test data | Made by a scratch script through the API: namespace `analytics` with tables `customers(id, name, email, country)` and `orders(id, customer_id, amount)` in `demo_warehouse` (Iceberg REST, with an engine-role client `uxwalk-engine`), and a local group `data-team` with member `member-sso`. Classifications `uxwalk-pii` and `uxwalk-tbl` were made in the UI. |
| No engine | No engine is attached to `demo_warehouse`. The Overview says so ("1 catalog is attached to no engine"). Every enforcement statement below is judged against that fact. |

### How to reproduce

The build uses a scratch copy of `deploy/docker/backend.Dockerfile` whose cargo-target cache mount has its own id. A shared id across checkouts gives E0432:

```bash
sed -e 's#--mount=type=cache,target=/src/target#--mount=type=cache,id=egret-classux-target,target=/src/target#' \
  deploy/docker/backend.Dockerfile > "$SCRATCH/backend.Dockerfile"
docker build -f "$SCRATCH/backend.Dockerfile" --target demo -t egret-backend:classux .
docker build -t egret-frontend:classux frontend
```

A scratch override file (not committed) sets the images and `EGRET_ENGINES__DOCKER__NETWORK`/`PROJECT` to `egret-classux_default` / `egret-classux`. It also sets `networks.default.name: egret-classux_default`, because the compose file pins the network name `egret-demo_default`, and two stacks would otherwise share one network and its DNS aliases. Mailpit's port is overridden with `ports: !override`.

```bash
export DEMO_PORT=18280 DEMO_URL=http://egret.localtest.me:18280 DEMO_APPS_URL=http://apps.localtest.me:18280
docker compose -p egret-classux --env-file deploy/demo/demo.env \
  -f deploy/docker-compose.demo.yml -f "$SCRATCH/override.yml" up -d --no-build
./deploy/demo/wait.sh && ./deploy/demo/seed.sh
# teardown
docker compose -p egret-classux --env-file deploy/demo/demo.env \
  -f deploy/docker-compose.demo.yml -f "$SCRATCH/override.yml" down -v
```

Build times on this machine: frontend about 4 min; backend about 30 min from a cold cache.

## 2. The steward job, counted (A)

Job: mask `email` of `demo_warehouse.analytics.customers` for everyone except group `data-team`, using a new classification `uxwalk-pii`. It starts from the Governance landing. Only clicks count; typing into a field does not.

| # | Click | Where | Note |
|---|---|---|---|
| 1 | Governance | top bar | |
| 2 | Classifications | sidebar | The empty page lists the 3 steps: Create, Attach in Data, Add a rule (C3). OBSERVED |
| 3 | New classification | page | Name is focused. Typing `uxwalk-pii` fills the key. |
| 4 | Create classification | dialog | Lands on the classification page. The Effect strip says "Does nothing yet." with **Add rule**, **Set clearance** and **Attach in Data**. OBSERVED (ux3-04) |
| 5 | Attach in Data | Effect strip | Opens Data with "Pick a catalog". |
| 6 | Go to path box | Data tree | Typed `demo_warehouse.analytics.customers`. |
| 7 | Go | Data tree | Opens the table; the tree expands to it. |
| 8 | `email` | columns table | Column names are now links, so this is no longer a guess. OBSERVED |
| 9 | Add classification | column sheet | |
| 10 | Classification picker | dialog | Each option shows name, key, a "Clearance" badge and "1 rule" when they apply. OBSERVED |
| 11 | `uxwalk-pii` | picker | |
| 12 | Add classification | dialog | The chip shows the colour swatch, links to the classification, and has a remove button (ux3-05). |
| 13 | `uxwalk-pii` chip link | column sheet | Back to the classification page. |
| 14 | Add rule | Effect strip | The dialog opens with Mask a column, Redact, Applies to = Classification `uxwalk-pii`, and name "Mask uxwalk-pii". The footer reason "Preview the rule to enable Create." is tied to Create by `aria-describedby`, and Create is `aria-disabled`. OBSERVED |
| 15 | Include kind | Exempt | The picker defaults to **user**. |
| 16 | `group` | listbox | |
| 17 | Group search | Exempt | Typed `data`. The suggestion list renders below the help line (N26 fixed). |
| 18 | `data-team` | suggestions | |
| 19 | Preview | dialog | "Masks 1 column on 1 table · 1 person exempt". Then "**Not enforced yet.** demo_warehouse has no engine attached, so this rule is stored but no query is masked. Attach an engine in Control Plane > Engines." OBSERVED (ux3-06) |
| 20 | Create | footer | Back on the classification page; the Effect strip lists "Mask · Redact: Mask uxwalk-pii". |

| Measure | Baseline (pass 2) | Now | Target |
|---|---|---|---|
| Clicks for the job | 36 (+15 detour = 51) | **20** (no detour) | ≤ 15 |
| Guesses | 4 | **0** | 0 |
| Silently disabled buttons met | 2 | **0** (every blocked button on the path names its reason) | 0 |
| Unhelpful errors met | 3 | **0** on the path | 0 |
| Surfaces and areas | 8 in 3 areas | 5 (classification page, Data, column sheet, Add classification dialog, rule dialog) in **1 area** | — |
| Effect visible | No | **Honest "Not enforced yet — no engine attached"** at Preview. No per-user result (Package 2). | effect visible |

Verdict: **the job takes 20 clicks, not ≤ 15** (OBSERVED). Five clicks are still avoidable:

- 2 clicks: the Exempt picker defaults to `user`, so a group needs a kind switch. A single search across users, groups and roles would save them.
- 2 clicks: the rule is started on the classification page, so the user goes Data → column sheet → chip link → Add rule. The column sheet's own **Add rule** opens Applies to = Path. Its "Or apply it to <classification>" hint exists in the spec but could not be checked: the classification had been removed from the column by then (INFERRED, it would cost one click either way).
- 1 click: Go to path needs a click into the box. Enter in the box submits, so this click is avoidable but is the natural one.

**Where it still hurts (OBSERVED):**

- After Create, no surface says the rule is not enforced: not the Effect strip ("Mask · Redact: Mask uxwalk-pii"), the Rules tab, the column sheet's Rules section, or the rule page. The honest "Not enforced yet" exists only in the transient preview.
- At 1440x900 the Preview result renders at the bottom of the scroll body, and the "Not enforced yet" box is cut off under the sticky footer until the user scrolls (ux3-06). It is not scrolled into view.

## 3. Effect and verification (B)

| Surface | What it says (OBSERVED) | Honest for this demo? |
|---|---|---|
| Effect strip, empty | "Does nothing yet. Nothing uses this classification yet. Attach it to data, add a rule, or set clearance to give it an effect." | Yes |
| Effect strip, after the job | Rules "Mask · Redact: Mask uxwalk-pii"; Clearance "None: anyone who can read the data can open it."; Where used "0 tables · 1 column · 0 BI objects" | Literally true. It omits that the rule is not enforced, so a reader can take the mask as live. **Gap.** |
| Rule preview, path or classification on `demo_warehouse` | "Masks 1 column on 1 table · 1 person exempt" + "Not enforced yet. demo_warehouse has no engine attached …" | Yes. "1 person exempt" matches data-team's one member. |
| Rule preview, classification attached nowhere | "Masks no column yet · nobody exempt" + "Not enforced yet. This classification is not attached to any table or column a catalog serves yet …" | Yes |
| Clearance tab, before any add | Callout: "Clearance decides who may open data with this classification in egret … A required group is enforced when the data is opened in egret, not in SQL Lab or notebooks. To hide values … add a mask rule as well." with an **add a mask rule** button | Yes. The scope now comes before the first add (N7). |
| Clearance tab, group data-team added | "1 person holds a required role or belongs to a required group" (expands to "Member SSO · group data-team"), "Platform admins can always open it in egret.", "Engines do not enforce this clearance: anyone with SQL access can read the data." Engine details sit in a closed `<details>`. | Yes. It is a group-only clearance and there is no engine. |
| Who can read? (user `admin`, typed) | Verdict first: "Nothing is enforcing — No engine with a policy profile serves this catalog, so nothing is enforced here (this is not a refusal). Attach an engine to govern it." Then the visible line "Masks on Trino and row filters are not shown here." and a closed "How this answer was worked out". | Yes (N19). |
| Who can read? (group data-team) | "Each member's own answer, per policy profile …", four counts all **0**, and "Not asked: Member SSO: No username yet, so no engine knows this user." | Partly. With no profile it says nothing about "nobody enforces". Four zeros read as an answer. |

The mask's **effect** cannot be seen: there is no engine, and per-user mask results are Package 2. The UI now **says so honestly at preview time**, but not afterwards. The longer visible-limit line ("… a manual policy that names a role or a group …") appears only when a profile serves the catalog, so it could not be shown on this demo (see §9).

## 4. Previously reported defects (C)

| Id | Verdict | Evidence (OBSERVED unless marked) |
|---|---|---|
| N1 verify a mask per user | **Not fixed** (out of scope: Package 2) | Preview and Explain now say "Not enforced yet" / "Nothing is enforcing" honestly (ux3-06, ux3-10). |
| N2 remove a classification in Governance | **Fixed** | Remove works on the chip (column sheet, table header) and on each Where used row. The confirm says "Remove uxwalk-pii from demo_warehouse.analytics.customers.email?". Toast: "Removed uxwalk-pii from …". Focus lands on the "Data" heading of Where used. |
| N3 table-level classification vs classification mask | **Fixed** | With a mask on `uxwalk-pii`, Add classification on table `orders` shows "On the whole table, uxwalk-pii would make analytics.orders unreadable." **Add** is aria-disabled (`aria-describedby=add-tag-conflict`), and **Attach to columns instead** switches to column checkboxes. Without a mask, the note "On a whole table, this classification counts for clearance on every column …" appears. Rule dialog with `uxwalk-tbl` on the whole table: "The preview was refused." + one sentence + **Remove from table** (confirm dialog, then automatic re-preview) + **Apply by path instead** + Details (ux3-11). Focus moves to the error heading. |
| N4 second mask on one column | **Fixed** | A path mask on `customers.email`: "…email is already masked by Mask uxwalk-pii (Redact, except group data-team).", the per-exemption lines "group data-team: undecided: the engine applies whichever mask it evaluates first" and "Everyone else: masked (Redact)", a required tick "Create anyway: I understand … will have two masks", and the footer reason "Tick 'Create anyway' …" (ux3-12). |
| N5 preview speaks in effect | **Fixed** | Effect sentence plus "Not enforced yet"; the compile detail sits under "What egret will recompile". |
| N6 reasons, required marks, Column help | **Fixed** | Create and Preview are `aria-disabled`, with a footer reason tied by `aria-describedby` ("Name the rule.", "Choose a classification.", "Preview the rule to enable Create.", "The form changed since that preview — run it again before saving."). "(required)" on Name, Classification and Catalog. "Leave Column empty to mask every column of the table." |
| N7 / F3 clearance copy | **Fixed** | See §3. Measured contrast in §6. |
| N8 duplicate name | **Fixed** | "uxwalk-pii" and "UXWALK-PII" are refused inline with "A classification named uxwalk-pii already exists." and a link. Create is disabled. |
| N9 key rules inline | **Fixed** | Key `uxwalk_tbl` gives "Not allowed in a key: _." as you type. The hint reads "Lowercase letters, digits, - and :". |
| N10 duplicate key | **Fixed** | Server 409 shown under Key: "Key uxwalk-pii is taken by uxwalk-pii." with a link. |
| N11 Add rule on the classification page | **Fixed** | Effect strip and Rules tab both have **Add rule**, prefilled as in step 14. |
| N12 draft kept on Cancel | **Fixed** | Cancel on a dirty form asks "Discard this rule?" (Keep editing / Discard). After leaving the page in-app and coming back, Add rule shows "Restored your unsaved rule · Start over" with the name kept. |
| N13 rule page | **Partially** | The back link reads "← uxwalk-pii" (`?from=classification:`). Deleting the rule lands on the classification's Rules tab with the toast "Rule Mask uxwalk-pii deleted."; there is no "No rule has this id". Still open: the rule page has no "applies to" list and no enforcement status. Its "Open classification" link has `aria-label="Open classification: Mask uxwalk-pii"`, which names the rule, not the classification. |
| N14 blocked classification delete | **Fixed** | "Delete or change these rules first:" with a link to the rule. Delete is aria-disabled with a description, and there is no type field (ux3-13). |
| N15 one delete convention | **Fixed** | Rule: type the rule's name, case-insensitive ("mask uxwalk-pii" accepted). Classification: "Type uxwalk-tbl to confirm". The name equalled the key in this test, so name-vs-key was not told apart (INFERRED from the spec). |
| N16 chips | **Fixed** | Colour swatch, link to the classification, "requires clearance" lock text, remove button (ux3-05). |
| N17 picker rows | **Fixed** | Name, key, "Clearance" badge, "1 rule". |
| N18 vocabulary | **Partially** | Remaining mixed terms: **"Include"** (Exempt sub-label); **"egret grant(s)"** (Overview count, column sheet); tab **"Manual policies"** (spec glossary says "Engine policies"; the column sheet and Explain also say "manual policy"); **"its tags"** (Data landing text); **"every assignment of it"** (classification delete dialog); **"No rule selects it."** (remove and delete dialogs); **"per policy profile"** (Who can read? subtitle); **"every profile it reaches recompiles"** (rule delete dialog). Overview Recent changes: **"tagged … with"**, raw **"mask MASK on tag:uxwalk-pii"**, footnote **"Role assignments and catalog tags"**. "Hand-written", "Masking rules", "Masks & filters", "egret rule", "Selects by", "Carries it" and `MASK_*` were **not seen**. |
| N19 Explain verdict first | **Fixed** | Verdict first; "How this answer was worked out" is a closed `<details>` (ux3-10). |
| N20 390 px | **Partially** | See §5. |
| N21 contrast of the clearance warnings | **Fixed** | See §6. |
| N22 disabled with reason | **Partially** | `aria-disabled` + description on the rule dialog's Preview and Create, the table-conflict Add, and the classification Delete. Still native `disabled`, which takes them out of the tab order: **Create classification**, **Add clearance**, **Add classification** (before a pick), **Explain**. Their reasons are visible as text. Data tree gridcells now have names (e.g. "Expand demo_warehouse Catalog demo_warehouse Iceberg · egret"). |
| N23 403 page | **Fixed** | member-sso on `/governance/classifications`: "403 — Forbidden · You don't have access to this page. · Governance needs a governance permission such as tag:read or policy:read. · Go to the home page" (ux3-01, ux3-02). It still renders outside the app shell (U3, as the spec says). |
| N24 skeletons | Not measured | One 5 s wait for the classification page content timed out once (OBSERVED); not investigated. |
| N25 one-item sidebar groups | **Not fixed** (non-goal, U3) | "DATA > Data", "PEOPLE & GROUPS > People & groups" remain. |
| N26 group suggestions cover help | **Fixed** | The suggestions list renders below the help line. |
| Overview counts | **OK** | "1 catalog · 1 egret grant · 0 rules · 2 classifications" matched the state (the rule had been deleted by then). |

Tally over N1–N26 (25 judged; N24 not measured, and the Overview row is not counted): **Fixed 19**, **Partially 4** (N13, N18, N20, N22), **Not fixed 2** (N1 and N25, both declared out of scope), **Regressed 0**.

## 5. 390x844 and 1440x900 (D)

| Surface | Result (OBSERVED) |
|---|---|
| Classifications list | Key, Description and BI objects are hidden; Name, Data and Clearance show. The actions column is **clipped**: the table container scrolls sideways (scrollWidth 280 > clientWidth 244) with no cue. There is no page-level scroll (`scrollWidth` = 390). (ux3-14) |
| Create dialog | Full width, 0–390 px, no overflow (ux3-15). |
| Classification page | The Effect strip stacks one fact per row. The tabs still **wrap to 3 rows** (tops 640/672/704); the spec expected 2. No page-level horizontal scroll. (ux3-16) |
| Data columns table | Stacked cards: `thead` hidden, `tr`/`td` display block, `data-label` = Type / Classifications / Rules. No overflow. (ux3-17) |
| Rule dialog | The footer is `position: sticky` and visible (top 663, bottom 756 of 844). Tabbing Name → Kind → Mask type → Applies to → Catalog → Schema → Table → Column → Include kind → Include search never put focus under the footer. The footer wraps **Create onto a second line under Cancel** (ux3-18, ux3-19). |
| Explain card | Alerts are 306 px wide inside the dialog, and nothing extends past 390 px. Only the short no-profile line could be shown (ux3-20). The user-suggestion popup **covers the Explain button**: Playwright's click was intercepted by the "Administrator / no allocated username yet" row, and Enter in the field was needed. |
| Forbidden page | Card fits, link reachable (ux3-02). |
| 1440x900 rule dialog | Dialog 67–832 px; footer sticky at 758–807; the scroll body holds 1291 px of content in 715 px. The tick checkbox gets focus by Tab at y≈662, above the footer. The preview result appears below the fold without being scrolled into view (ux3-06). |

## 6. Keyboard and a11y (E)

- **Rule dialog Tab order** (OBSERVED, DOM order with no positive tabindex): Name, Kind, Mask type, Applies to, Catalog, Schema, Table, Column, Include kind, Include search, Preview, the "already masked by" rule link, the Create-anyway checkbox, Cancel, Create, Close. This is logical.
- **Focus after Remove** (Where used): on the "Data" `h2`. Good.
- **Focus after Discard** of the rule dialog, after deleting a rule (navigates), and after deleting a classification (navigates): **`BODY`**. It does not return to the trigger or move to a heading.
- **Reasons announced:** Create (`aria-describedby` → "Preview the rule to enable Create."), Preview (→ "Name the rule."), table-conflict Add (→ `add-tag-conflict`), and classification Delete (described) are all `aria-disabled="true"` and focusable. The native-disabled buttons are listed under N22.
- **Contrast**, measured by compositing the computed colours on a 1×1 canvas (OBSERVED):

| Text | Light | Dark |
|---|---|---|
| Clearance callout paragraphs | 17.44:1 (fg oklch(0.21 0.11 263) on 251,251,252) | 17.44:1 (252,252,252 on 23,23,26) |
| "Platform admins can always open it in egret." (muted) | 6.34:1 | 7.76:1 |
| "Engines do not enforce this clearance …" | 18.03:1 | 19.41:1 |

All pass AA (N21 fixed). The callout's background computes to a near-neutral (light) and to `oklab(0.27 … / 0.5)` (dark), not a visible highlight tint. Contrast is fine, but the "warning" carries little visual weight. This is INFERRED against the spec's `bg-highlight/15`.

## 7. Console and network (F)

Console errors and non-2xx responses on this stack (port 18280), all OBSERVED:

| Request | Status | Cause |
|---|---|---|
| `GET /api/auth/me` (×2) | 401 | Before sign-in; expected. |
| `POST /api/control-plane/perm/explain/principal` | 400 | Who can read? with the free text `data` in Group: the alert shows the raw "bad request: no group is named data". |
| `POST /api/control-plane/tags` | 409 | Duplicate-key test (N10); shown inline. Expected. |
| `POST /api/control-plane/perm/preview` | 400 | Table-level conflict test (N3); shown as the coded short sentence. Expected. |
| `GET /api/control-plane/classifications/{id}/usage` | **404** | Stale refetch right after deleting the classification `uxwalk-tbl` (request 39 after the 204 DELETE). Unexpected. |

The browser's cumulative console also held entries from another session's stack on port 18080 (the shared Playwright browser). They are not attributed to this run.

## 8. Findings requiring action (ranked)

| # | Severity | Finding | Evidence |
|---|---|---|---|
| 1 | **Major** | After Create, nothing says the rule is **not enforced**: not the Effect strip, the Rules tab, the column sheet or the rule page. "Not enforced yet" lives only in the preview, so a steward sees "Mask · Redact: Mask uxwalk-pii" and can believe email is masked. | §2 step 20, §3 |
| 2 | **Major** | Steward job is **20 clicks vs the ≤ 15 target**. The cheapest savings: an Exempt picker that searches every principal kind (−2), and Add rule from the column sheet offering the column's classification directly (−2). | §2 |
| 3 | Minor | Who can read? accepts any free text. A non-existent user `admin` gets a verdict; group `data` gets a raw "bad request: no group is named data". The user picker's suggestion popup covers Explain at 390 px. | §3, §5, §7 |
| 4 | Minor | Group-mode Who can read? with no profile shows four zero counts and no "nothing is enforcing" line. | §3 |
| 5 | Minor | Vocabulary leftovers (N18 list), notably "Include", "egret grant", "Manual policies" vs glossary "Engine policies", "No rule selects it.", and Overview "tagged … with" / "mask MASK on tag:…". | §4 N18 |
| 6 | Minor | Overview Recent changes renders table-level paths with a trailing dot: "tagged demo_warehouse.analytics.orders. with uxwalk-tbl", "removed uxwalk-tbl from demo_warehouse.analytics.orders.". | §4 |
| 7 | Minor | The rule dialog shows "A rule on a classification is catalog-unscoped … every Trino profile …" even when Applies to = Path. | dialog snapshots |
| 8 | Minor | Preview result is not scrolled into view, and the warning is cut off under the sticky footer at 1440x900. | ux3-06 |
| 9 | Minor | Focus goes to `BODY` after Discard, after a rule delete and after a classification delete. | §6 |
| 10 | Minor | Stale `GET …/usage` 404 after a classification delete. No toast was seen within 1 s of that delete (the rule delete did toast). | §7 |
| 11 | Minor | N22 leftovers: Create classification, Add clearance, Add classification (before a pick) and Explain are native `disabled`. | §4 |
| 12 | Minor | 390 px: the classifications list's actions column is clipped in a sideways scroll without a cue; tabs wrap to 3 rows; the rule dialog footer wraps Create under Cancel. | §5 |
| 13 | Minor | The rule page's "Open classification" link has an aria-label naming the rule ("Open classification: Mask uxwalk-pii"). | §4 N13 |
| 14 | Minor | Table-entry rule dialog: Name is not prefilled and Column is not focused (spec C8 table row). The Exempt search keeps the typed text ("data") after a pick. | dialog snapshots |
| 15 | Minor | The Remove confirm leads with "Rules that apply to X stop applying here." then says "No rule selects it.". | §4 N2 |

There is no Blocker.

## 9. Screenshots index

Kept outside git: `ux3-01-forbidden-1440.png`, `ux3-02-forbidden-390.png`, `ux3-03-classifications-empty.png`, `ux3-04-tag-page-effect-empty.png`, `ux3-05-column-sheet-chip.png`, `ux3-06-rule-preview-effect.png`, `ux3-07-clearance-light.png`, `ux3-08-clearance-dark.png`, `ux3-09-explain-group.png`, `ux3-10-explain-user-nothing-enforcing.png`, `ux3-11-rule-table-conflict.png`, `ux3-12-second-mask-tick.png`, `ux3-13-delete-blocked.png`, `ux3-14-390-classifications-list.png`, `ux3-15-390-create-dialog.png`, `ux3-16-390-tag-page.png`, `ux3-17-390-data-columns-cards.png`, `ux3-18-390-rule-dialog.png`, `ux3-19-390-rule-preview.png`, `ux3-20-390-explain.png`.

## 10. What could not be done, and why

- **The mask's effect on a real query.** No engine serves `demo_warehouse`, and per-user mask results are Package 2. Attaching an engine (Add engine) was out of scope for this check.
- **The longer Explain visible-limit line, and its wrap at 390 px.** It appears only when a policy profile serves the catalog; without an engine there are no verdicts.
- **A user-level Explain for a real person.** No demo user has an allocated username ("no allocated username yet"). A typed name was used instead.
- **The column-sheet hint "Or apply it to <classification>".** The classification had been removed from `email` when the 390 run reached the column sheet.
- **Classification delete typing name vs key.** Name and key were equal in the test.
- **Skeleton timings (N24)** were not measured.
- **Shared browser.** The Playwright browser is shared: tab 0 was showing another session's login page on port 18080 and was navigated to this run's stack. The other session's containers were not touched.

## 11. Cleanup

- `docker compose -p egret-classux … down -v` removed the containers, the volumes `egret-classux_demo-{pgdata,rustfsdata,appswork}` and the network `egret-classux_default`. All `uxwalk-` data went with the volumes; `uxwalk-pii`'s rule, its attachment and `uxwalk-tbl` had already been deleted in the UI.
- Images `egret-backend:classux` and `egret-frontend:classux` were removed. The BuildKit cache entry with id `egret-classux-target` (3.9 GB) was pruned by its id.
- `docker ps` afterwards showed `egret-demo-*` (10 containers), `egretdocs-*` and `egret-dev-*` all still up.
