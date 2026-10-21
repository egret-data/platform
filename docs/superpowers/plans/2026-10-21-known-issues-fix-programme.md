# Known-issues fix programme: bugs and UX

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. This is the programme plan: each package below gets its own short spec and task plan when it is picked up (as WP-A, WP-B and WP-C of the Docker demo stack did).

**Goal:** Fix the 165 open bug, defect, security, consistency and UX rows of the docs' Known issues (KI-001 to KI-235), and close each row with evidence. The 53 open feature gaps and the 8 docs rows are out of scope (see Deferred).

**Architecture:** 23 packages in three waves. Wave 1 clears the S1 rows and the security finding. Wave 2 fixes S2 bugs, one code area per package. Wave 3 is UX: first one shared UI foundation (U1), then the per-area polish that builds on it. Packages are small enough for one PR each; two run in parallel at most.

**Tech stack:** Rust workspace (axum, sqlx, Cedar), React + Vite frontend with the shared `ui/*` components, Playwright for live checks, the Docker demo stack (`just demo-up`) as the live environment, `egret-data/docs` for the user documentation.

## Global constraints

- Conventional Commits; `Closes #N` only in PR descriptions; trailer `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Never commit secrets; scratch files go to the scratchpad, never into git.
- Never merge without the user's explicit "merge đi". A package is ready for that when its CI is green.
- Frontend work follows `docs/en/frontend-rules.md` (FE-PAGE, FE-FORM, FE-VISUAL, FE-A11Y); a change to a rule lands with its lint or test.
- Migration numbers are a shared namespace: take the next free number at merge time, and rebase if another package took it.
- One cargo command at a time; long builds in the background; tests that need Postgres use the per-process test DB.
- A fix that touches an authorization gate is tested as the unprivileged principal, not as admin.
- Docs rule: a PR that changes user-visible behaviour updates `egret-data/docs`. Screenshots that change are re-shot through `scripts/shots/*.mjs`. Known-issues rows are closed in one docs PR per wave (not per package) to avoid conflicts, each with the platform PR number.

## The loop every package follows

1. **Re-verify the premise.** Reproduce each row on current `main` with the demo stack before writing code. Many rows were found on an older instance. A row that no longer reproduces is closed by the docs PR; a row marked *verify* (KI-006, KI-021, KI-129) is reproduced or dropped here.
2. **Spec and plan** for the package: a half-page spec (what each row will do, decisions taken) and a task plan with a failing test per row.
3. **Red test first.** Backend: an integration test through the real handler (and the Cedar PDP for authorization rows). Frontend: vitest for logic, Playwright for layout and behaviour, with the check that fails before the change. Run the test against the old code once to see it fail (the KI-175 fix did this with an empty `PATH`).
4. **Fix** in small commits; `cargo fmt`, `clippy -D warnings` for the touched crates, `npm run lint`, `tsc`, vitest.
5. **Live check** on the demo stack as the user the row names (admin, or `member-sso` for non-admin rows), light and dark, and at 1440 px and 768 px for layout rows. A review finding is a claim until the browser shows it.
6. **PR** with the rows it fixes (KI ids) and the evidence; CI green; wait for "merge đi".
7. **Docs** after the wave: close the rows, refresh screenshots and pages that described the old behaviour.

## Programme at a glance

| Pkg | Wave | Title | Rows | Severity | Size | Needs |
|---|---|---|---|---|---|---|
| F1 | 1 | Workspace connections for non-admins | 1 | S1 1 · S2 0 · S3 0 | S | - |
| F2 | 1 | Storage browse and system secrets | 3 | S1 0 · S2 1 · S3 2 | M | - |
| F3 | 1 | Workflow worker: parallel claims and visible "worker off" | 2 | S1 2 · S2 0 · S3 0 | M | - |
| F4 | 1 | Secrets hygiene | 3 | S1 1 · S2 0 · S3 2 | M | - |
| F5 | 1 | Dashboards: lost input and lost filters | 4 | S1 2 · S2 1 · S3 1 | M | - |
| F6 | 1 | First run from a clean checkout | 3 | S1 1 · S2 0 · S3 2 | S | - |
| B1 | 2 | SQL Lab correctness | 10 | S1 0 · S2 6 · S3 4 | M | - |
| B2 | 2 | Datasets and metrics integrity | 12 | S1 0 · S2 3 · S3 9 | M | - |
| B3 | 2 | Chart rendering bugs | 12 | S1 0 · S2 5 · S3 7 | M | - |
| B4 | 2 | Alerts and reports delivery | 7 | S1 0 · S2 4 · S3 3 | M | - |
| B5 | 2 | Data API correctness | 4 | S1 0 · S2 2 · S3 2 | S | - |
| B6 | 2 | Engines and monitor | 9 | S1 0 · S2 3 · S3 6 | L | - |
| B7 | 2 | Authentication, sessions and roles | 12 | S1 0 · S2 3 · S3 9 | M | - |
| U1 | 3 | UI foundation: page structure | 6 | S1 0 · S2 0 · S3 6 | L | - |
| U2 | 3 | UI foundation: controls | 4 | S1 0 · S2 0 · S3 4 | M | U1 |
| U3 | 3 | UI foundation: shell, states and small screens | 7 | S1 0 · S2 1 · S3 6 | M | U1 |
| U4 | 3 | SQL Lab UX | 14 | S1 0 · S2 0 · S3 14 | M | U1 |
| U5 | 3 | Dashboard UX | 10 | S1 0 · S2 1 · S3 9 | M | U1 |
| U6 | 3 | Chart editor UX | 7 | S1 0 · S2 0 · S3 7 | M | U1 |
| U7 | 3 | Alerts and reports UX | 6 | S1 0 · S2 0 · S3 6 | M | U1, U2 |
| U8 | 3 | Data API and Apps UX | 9 | S1 0 · S2 1 · S3 8 | S | U1 |
| U9 | 3 | Workflows validation and display | 11 | S1 0 · S2 1 · S3 10 | M | U1 |
| U10 | 3 | Control Plane, Governance and settings UX | 9 | S1 0 · S2 1 · S3 8 | M | U1 |

Total: 165 rows (S1 7, S2 33, S3 125) in 23 packages. Sizes: S about one day of work, M two to three, L a week; they are estimates to order the work, not a promise.

### Order and parallelism

- **Wave 1:** F1 with F5; then F3 with F2; then F4 with F6. F1 and F2 both touch authorization, so they do not run together.
- **Wave 2:** B1 with B2 (SQL Lab and Datasets share the dataset create path, so keep their PRs small), B3 with B4, B5 with B7, then B6. The B packages do not depend on each other; B6 needs the Docker provisioner and goes last so the demo stack is quiet.
- **Wave 3:** U1 alone (it touches almost every route; mechanical changes only), then U2 and U3 together, then U4 to U10 two at a time. U7 needs U2 for the select replacement.
- Waves can overlap once Wave 1 is merged: a Wave 2 package that does not need U1 may run beside U1.

## Decisions needed before the packages that depend on them

| # | Decision | Recommendation | Blocks |
|---|---|---|---|
| D1 | One page-title scale. Today: 13, 16, 18, 20 and 24 px. | 20 px for every page `h1`; 16 px for compact tool headers (SQL Lab, chart editor, endpoint detail). | U1 |
| D2 | Page widths. Today: full width, 896 px, 768 px and 256 px offsets. | Lists, tables and Control Plane: full width. Forms, editors and Workspace settings: a 960 px column starting at the same left edge as lists. | U1 |
| D3 | Tab styles. Today: five. | Two, documented in the rulebook: pill strip for page sections (dataset, alert, catalog, flow), underline for panels inside an editor (chart Data/Display, endpoint tabs). | U1 |
| D4 | Status badges and entity links. Today: three badge styles, teal vs black names. | One `StatusBadge` (light tint, sentence case: *Active*); names in lists are links with the same colour everywhere (black, underline on hover) in Explore and in Control Plane. | U1 |
| D5 | Vocabulary for KI-007: *Engines* vs *databases* vs *connections*. | Workspace level: **Connections**. Control Plane: **Engines**. API names stay; the docs map them. | U10 |
| D6 | KI-185: an SSO user lands on *No workspaces yet*. | Let a workspace admin add a member by email before the first sign-in (a pending membership that binds at first login); keep the better empty state as well. | B7 |
| D7 | KI-045: row chart order. | Sort by value (descending) by default, as the gallery text says. | B3 |
| D8 | KI-098: deleting a dataset deletes its charts. | Refuse while charts use it, listing them; offer "delete with charts" only as a typed confirmation. | B2 |
| D9 | KI-159: which permission lists a workspace's connections. | `query:run` or `dataset:read` lists the connections granted to the workspace (name, engine type, no host or credentials); creating and editing stays admin. | F1 |

If a decision is not made, the package uses the recommendation and records it in its spec.

## Packages

The paragraph under each title is the intended approach, a proposal to confirm against the code in the package's own spec. File and function names come from the Known-issues rows; they were not re-read for this plan.

### Wave 1: S1 and security (small, independent)

#### F1: Workspace connections for non-admins

**Size** S · **Needs** nothing · **Rows** 1 (S1 1 · S2 0 · S3 0)

Members get 403 on `GET /api/ws/{ws}/databases` (`registry/routes.rs:109`) so SQL Lab has no database. Decide the permission (`query:run` lists the connections granted to the workspace, without host or credentials), add the test as a non-admin Cedar principal, and give the picker a message for "none granted". Verify as `member-sso` on the demo stack.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-159 | S1 | SQL Lab / Workspace settings | Only workspace admins can list a workspace's connections. Members with `ws_editor` (direct or through a group), `ws_viewer` or a custom role with `query:run` get 403 o... |

#### F2: Storage browse and system secrets

**Size** M · **Needs** nothing · **Rows** 3 (S1 0 · S2 1 · S3 2)

Stored SQL Lab results (`query-results/*.jsonl`) must not be listed or downloadable with `storage:read`: hide the results prefix from Browse and `download`, or require a stronger permission; honour the connection's Key prefix in Browse (KI-129, still unverified: reproduce first); mark system-managed secrets (`engine-internal-*`, `engine-monitor-*`, `engine-tls-*`, `catalog-client-*`) read-only, out of the Password-secret picker, and hide them from rotate and delete.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-129 | S3 | Control plane / Storage | On a connection created with Key prefix `sandbox`, the Browse tab lists the bucket root, not the prefix.  |
| KI-206 | S2 | Control plane / Storage | **Storage → \<connection\> → Browse** lists `query-results/*.jsonl`, the stored SQL Lab results of every user, and `GET /storage/{id}/download?key=…` returns them to a... |
| KI-207 | S3 | Control plane / Secrets | System-managed secrets (`engine-internal-*`, `engine-monitor-*`, `engine-tls-*`, `catalog-client-*`) are listed with user secrets, can be rotated or deleted, and are o... |

#### F3: Workflow worker: parallel claims and visible "worker off"

**Size** M · **Needs** nothing · **Rows** 2 (S1 2 · S2 0 · S3 0)

`tick_with` awaits `run_one` per claimed run, so one `Sleep` stalls the replica: run the claimed runs concurrently with a bound (config key), keep the claim lease alive, and test with a slow task beside a fast one. KI-013: when the worker is off, Run, Flow and Executions must say so, not only Triggers.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-013 | S1 | Workflows | With the default config the workflow worker is **off**. *Run* succeeds and the execution sits in *Running* indefinitely (one waited 53 s until the worker was enabled).... |
| KI-187 | S1 | workflows | One long task stalls every workflow on the replica: the claim loop runs the task runs it claimed one after another (`tick_with` awaits `run_one` per run) and only then... |

#### F4: Secrets hygiene

**Size** M · **Needs** nothing · **Rows** 3 (S1 1 · S2 0 · S3 2)

Workflow SECRET inputs are stored and returned in clear text (execution API, Overview tab): store through the secret store and return a mask. `egret init` must not print a password that came from `EGRET_ADMIN_PASSWORD`. The Helm Keycloak dev realm must not be what `oidc.enabled` installs by default (or must say loudly that it is a dev realm). May need a migration: take the next number at merge time.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-112 | S1 | workflows | SECRET inputs are stored and returned in clear text (execution API, Overview tab).  |
| KI-146 | S3 | CLI | `egret init` prints the password even when it was supplied in `EGRET_ADMIN_PASSWORD`, so it lands in terminal history and CI logs.  |
| KI-152 | S3 | Helm | The bundled Keycloak (with `oidc.enabled`) imports the dev realm: `sslRequired: none`, `*` redirect URI, password grant, known user passwords and the client secret `eg... |

#### F5: Dashboards: lost input and lost filters

**Size** M · **Needs** nothing · **Rows** 4 (S1 2 · S2 1 · S3 1)

KI-059: the card wrapper handles Enter and Space with `preventDefault` and the keydown bubbles from the input; stop at the input. KI-060: `useDashboardParams` must read back the object a date / all-options filter writes. KI-061: a viewer must be able to clear a default; make "Reset all filters" count defaults. KI-063: `interpolateParams` must format date objects. Each gets a vitest and a Playwright check.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-059 | S1 | dashboards / edit | A space or Enter typed into a heading or text card in edit mode is not inserted. The card wrapper (`role="button"`) handles Enter and Space with `preventDefault` and t... |
| KI-060 | S1 | dashboards / filters | A value chosen in a `date` / `all-options` filter is lost. The picker emits an object, the URL stores it as JSON, and `useDashboardParams` reads it back as a plain str... |
| KI-061 | S2 | dashboards / filters | A filter that has a default value cannot be cleared by a viewer: the chip's X removes the URL parameter, then the default is applied again. "Reset all filters" is offe... |
| KI-063 | S3 | dashboards / cards | Text and heading cards print `[object Object]` for a `{{slug}}` bound to an all-options date filter (`interpolateParams` stringifies the object).  |

#### F6: First run from a clean checkout

**Size** S · **Needs** nothing · **Rows** 3 (S1 1 · S2 0 · S3 2)

`.env.example` sets `AWS_*` but not `EGRET_STORAGE__*`, and `Config::validate` demands them: make `cp .env.example .env; just up; just serve` start (CI job that does exactly this). Unknown `EGRET_*` keys: warn at start. `server.port` must move `catalog.public_url` with it, or the doc must say it cannot.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-136 | S1 | Quick start | A clean `cp .env.example .env; just up; just serve` cannot start. `.env.example` sets `AWS_*` but not `EGRET_STORAGE__BUCKET/ACCESS_KEY/SECRET_KEY`, and `Config::valid... |
| KI-140 | S3 | Configuration | Unknown keys and misspelled `EGRET_*` variables are silently ignored at the top level and in most sections (only four sub-tables use `deny_unknown_fields`).  |
| KI-141 | S3 | Configuration | Changing `server.port` leaves `catalog.public_url`, and with it the machine IdP `issuer` and token endpoint, at `http://localhost:8080`.  |

### Wave 2: S2 bugs by area

#### B1: SQL Lab correctness

**Size** M · **Needs** nothing · **Rows** 10 (S1 0 · S2 6 · S3 4)

`ORDER BY` is lost because a single SELECT is wrapped as `SELECT * FROM (...) LIMIT n`: apply the limit without a subquery on Trino (and keep `truncated` correct, KI-081). `Save as dataset`: trailing `;` and duplicate column names (500 -> 400 with a message). Cmd/Ctrl K inside Monaco. A cancelled run shows a state. Error line numbers (+1) and the doubled `engine: engine:` prefix. Pre-start errors as a readable message, not raw JSON. The 60 s limit named in the error. Float display noise. KI-006 is unverified: reproduce with a real keyboard or close it.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-004 | S3 | SQL Lab | Floating-point results are shown with representation noise (`63339475.32000001`). Provide display rounding or a number format in results. |
| KI-006 | S2 | SQL Lab | verify: typing a long statement quickly into the editor via browser automation produced scrambled characters (autocomplete interference). Inserting the text at once wo... |
| KI-078 | S2 | SQL Lab | A cancelled run leaves the Results panel on "No results yet." with no badge or message; only History shows status *cancelled*.  |
| KI-079 | S2 | SQL Lab | The command palette shortcut (Cmd/Ctrl K) does not open while the text cursor is in the editor: Monaco treats it as a chord prefix. It only works with focus outside th... |
| KI-081 | S2 | SQL Lab | A single SELECT/WITH is wrapped in `SELECT * FROM (...) LIMIT n`, so the engine returns exactly n rows with `truncated=false`. The "Results were limited" chip never ap... |
| KI-086 | S3 | SQL Lab | Errors found before the query starts (bad parameter, forbidden) appear as a toast with raw JSON: `Error: {"error":"invalid input: parameter 'n' is not a number: abc"}`.  |
| KI-087 | S3 | SQL Lab | Engine error positions are one line too high for wrapped queries (a typo on line 1 is reported on line 2); some messages carry a doubled prefix `engine: engine:` (data... |
| KI-095 | S2 | SQL Lab | **Save as dataset** fails for SQL ending in a semicolon (works in SQL Lab) and returns HTTP 500 "internal error" for duplicate column names.  |
| KI-154 | S2 | SQL Lab | `ORDER BY` is ignored on Trino: Egret wraps a single SELECT as `SELECT * FROM (<query>) AS _egret_lim LIMIT n`, and Trino drops the ordering of a subquery, so rows com... |
| KI-211 | S3 | SQL Lab | SQL Lab runs every query with `query_max_run_time = 60s`; a longer statement (for example the first CTAS on a fresh engine) fails with the bare error *timeout* and no... |

#### B2: Datasets and metrics integrity

**Size** M · **Needs** nothing · **Rows** 12 (S1 0 · S2 3 · S3 9)

Dimension and Filter switches must be read by the chart builder and the backend (KI-096). Deleting a dataset must not silently delete its charts: confirm with the count, or refuse (KI-098); the same for renaming or deleting a used metric (KI-099) and for trashing (KI-111). Reject a non-aggregate metric on save (KI-100) and an empty column name in the API (KI-101). Refresh keeps the Time switch the user set. Display names in table headers and Preview. Confirmations for column and metric delete. Duplicate for physical datasets. Singular "1 metric"; Usage icon by chart type.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-096 | S2 | Datasets | The **Dimension** and **Filter** switches are stored but not read by the chart builder or the backend: a column with Dimension off still appears in the X-axis picker.  |
| KI-098 | S2 | Datasets | Deleting a dataset (header trash button or **Delete permanently**) also deletes every chart built on it. The confirmation says only that the table is not affected.  |
| KI-099 | S2 | Datasets | Renaming a metric that charts use breaks those charts (`unknown metric: old_name`); deleting a used metric is allowed without warning.  |
| KI-100 | S3 | Datasets | A metric whose expression is not an aggregate (for example `totalprice`) is accepted and only fails inside a chart: `'totalprice' must be an aggregate expression or ap... |
| KI-101 | S3 | Datasets | `POST /datasets/{id}/columns` accepts an empty column name (the UI blocks it). I could not delete the resulting column through the API (empty path segment).  |
| KI-102 | S3 | Datasets | **Refresh** silently resets the **Time** switch of every column to what the engine type says.  |
| KI-103 | S3 | Datasets | Table chart headers and the Preview tab show the column name, not the **Display name**, while the chart editor field lists show the Display name.  |
| KI-105 | S3 | Datasets | **Delete** on a calculated column and on a metric acts with no confirmation.  |
| KI-110 | S3 | Datasets | **Duplicate** is only offered for virtual datasets; there is no way to copy a physical dataset's column settings and metrics.  |
| KI-111 | S3 | Datasets | Trashing a dataset hides it but its charts keep running; nothing tells the owner of those charts.  |
| KI-158 | S3 | Datasets | The dataset header says "1 metrics" (no singular).  |
| KI-166 | S3 | Datasets | The **Usage** tab shows the bar-chart icon for every chart, whatever its type.  |

#### B3: Chart rendering bugs

**Size** M · **Needs** nothing · **Rows** 12 (S1 0 · S2 5 · S3 7)

Object detail export grabs the wrong `<svg>` (KI-043). An empty Progress goal is "no goal" (KI-044). Row chart order (KI-045, decide: sort by value by default). Dropped map regions get a note (KI-046). **Change** type keeps the shared settings namespace (KI-047). Export popover states the real 10,000-row limit in both the chart and dashboard exports (KI-049, KI-072). Hide Row limit where it has no effect (KI-051). Waterfall swatch, scatter axis colour, funnel labels, box-plot clipping. Compare against Metabase for each, as the chart rulebook asks.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-043 | S2 | Charts / Object detail | The **Export chart** popover offers `.png` for Object detail, and the downloaded file is a 286-byte image of the pager arrow icon instead of the record (the exporter g... |
| KI-044 | S2 | Charts / Progress | An empty **Goal** is treated as `0`, not as "no goal": the bar reads **Goal 0** and **Goal exceeded**. The message **No goal set** can never appear for an empty goal.  |
| KI-045 | S2 | Charts / Row | The gallery says rows are "sorted by value", but a row chart is alphabetical until a **Sort** is added. With **Sort** = revenue descending, the largest bar is drawn at... |
| KI-046 | S2 | Charts / Map | A region value that matches no region is dropped silently. There is no note (pin maps do have one), and the legend range still includes the dropped rows' values. Easy... |
| KI-047 | S2 | Charts / Editor | **Change** (chart type) clears the X-axis, Y-axis and all other fields, even between bar, line, area and combo which share one settings namespace. Source comments say... |
| KI-049 | S3 | Charts / Editor | The export popover says **The maximum download size is 1 million rows.** A chart fetches at most 10,000 rows, so the limit that applies is 10,000.  |
| KI-051 | S3 | Charts / Editor | **Row limit** is shown on Big number, Progress and Gauge, but the query always returns one row, so the box has no effect.  |
| KI-057 | S3 | Charts / Waterfall | The **Total color** swatch shows slate (`#4C5773`) while the total bar is drawn in the dark text color by default, so the swatch does not match the bar until a color i... |
| KI-072 | S3 | dashboards / export | The download popover promises "The maximum download size is 1 million rows", but the file is built in the browser from the rows the card loaded, which a chart caps at... |
| KI-163 | S3 | Charts / Scatter | Scatter axis titles (`customers`, `avg_balance`) are drawn in a very light grey, almost invisible on the white plot, unlike the axis titles of bar, line and combo char... |
| KI-164 | S3 | Charts / Funnel | With **Funnel type** = **Bar**, the X axis drops every other stage label at the default 1440 px editor width (only *Visited site*, *Added to cart*, *Paid*), so two bar... |
| KI-165 | S3 | Charts / Box plot | With **Show values on data points** on, the last box's label is clipped at the right edge (`31.6` instead of `31.6K`).  |

#### B4: Alerts and reports delivery

**Size** M · **Needs** nothing · **Rows** 7 (S1 0 · S2 4 · S3 3)

`Run now` must update last state (KI-032). A run whose delivery failed is not **Success**; record every destination's failure (KI-033, KI-034). Report mail subject (KI-157). The renderer client has a fixed 30 s timeout shorter than a multi-object PDF (KI-167): derive it from the number of objects or make it a key. CSV and Text numbers formatted as in the table (KI-169). Items due in one tick run concurrently with a bound (KI-173). Needs the renderer and Mailpit: verify on the demo stack.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-032 | S2 | Alerts | **Run now** writes a run-log row but does not update the item's last state: the list **Last run** and the header badge stay **Never run**, while **Recent runs** on the... |
| KI-033 | S2 | Alerts | A run is **Success** (and starts the grace period) even when delivery failed; the failure is only in the run-log **Detail**. Nothing in the list marks an alert whose m... |
| KI-034 | S3 | Alerts | With several destinations only the first failure is recorded. An alert with an email and a webhook destination recorded only the webhook error; the missing-SMTP error... |
| KI-157 | S3 | Reports | A report's mail subject is always `[egret] <name> report`, so a report whose name ends in "report" mails "… report report".  |
| KI-167 | S2 | Reports | A multi-object PDF report fails with `render failed: error sending request for url (http://renderer:8080/render)` although the renderer is up. The render client has a... |
| KI-169 | S2 | Reports | CSV and Text reports write numbers unformatted, as `f64` debug text: `"F","1.0356810234900002e+9"` and `F \\| 1.0356810234900002e+9`, so spreadsheet users see scientif... |
| KI-173 | S3 | Alerts / Scheduler | Items due in the same tick run one after another, so a slow PNG/PDF render delays every other alert in the workspace: twelve items due at 12:10:00 started between 12:1... |

#### B5: Data API correctness

**Size** S · **Needs** nothing · **Rows** 4 (S1 0 · S2 2 · S3 2)

A new Path saved on a draft must not change the served URL before publish (KI-023): the router must match the published snapshot. The "Revoking will not remove access" note only where the inherited access exists (KI-180). Versions table width (KI-024); label the minimum and maximum boxes (KI-026).

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-023 | S2 | Data API | Saving a new **Path** on an endpoint's draft changes the served URL immediately, before publish. The live version's snapshot still has the old path, yet the router mat... |
| KI-024 | S3 | Data API | Versions tab: the **N breaking change** badge overlaps the note text because the Version column is too narrow.  |
| KI-026 | S3 | Data API | Number parameter constraints (**minimum**, **maximum**) are two unlabeled boxes under the type; once filled, the placeholder that names them disappears, so `0` and `10... |
| KI-180 | S2 | Data API | Access tab: the note **Revoking will not remove access** (*This account also reaches invoke through the workspace…*) is shown on every granted service account, includi... |

#### B6: Engines and monitor

**Size** L · **Needs** nothing · **Rows** 9 (S1 0 · S2 3 · S3 6)

Monitor must work on a Trino 483 provisioned engine (KI-194, the summary calls an endpoint that answers 502). Engines added with Add engine must be recreated after `demo-down` and `demo-up`, or **Restart** must do it (KI-213). Lineage reader needs access to the engine's catalogs (KI-201). Worker replicas `0` is stored as `0` (KI-197). Docker-provisioned engines: hide Logs and Exec, use container wording instead of Kubernetes (KI-195, KI-196), show an endpoint that resolves (KI-199), say when an ingress host is missing (KI-200), drop the unrelated `auth.passwordUsers` from Review changes and confirm Save (KI-198). Needs the Docker provisioner: verify on the demo stack.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-194 | S2 | Engines / Monitor | The **Monitor** tab of a running Trino 483 engine says *Engine not reachable — is a coordinator running?* and *Coordinator unreachable — no live queries.*: `/cluster`... |
| KI-195 | S3 | Engines | On a Docker-provisioned engine the **Logs** and **Exec** buttons stay visible although they cannot work: Logs shows the raw *bad request: not available with the Docker... |
| KI-196 | S3 | Engines | Engine dialogs speak Kubernetes on a Docker engine: Delete says *This destroys the engine's Kubernetes cluster*, Restart says it *rolls the coordinator and worker pods... |
| KI-197 | S3 | Engines | After **Add engine** with Worker → Replicas `0`, the stored spec says `workerReplicas: 2` and Overview shows *Coord 1 · Worker 2* while only the coordinator container... |
| KI-198 | S3 | Engines / Settings | **Review changes** lists an unrelated `auth.passwordUsers [] Added` next to the real change and has only **Close**; **Save changes** then applies without confirmation... |
| KI-199 | S3 | Engines | The engine header and list show the endpoint `docs-trino-coordinator.egret-engines:8080`, which does not resolve on the Docker network; the container answers as `docs-... |
| KI-200 | S3 | Engines / Add engine | **Expose via ingress** without a **Host** is silently dropped: the engine is created with no ingress and no message. With a host the Docker provisioner refuses (*not s... |
| KI-201 | S2 | Governance / Lineage | View lineage fails on a provisioned engine: *engine docs-trino: engine: Access Denied: Cannot access catalog demo_warehouse*, because the engine's own policy does not... |
| KI-213 | S2 | Engines / Demo stack | After `just demo-down` and `just demo-up`, engines added with **Add engine** stay `provisioning`: their containers are gone and nothing recreates them, and **Restart**... |

#### B7: Authentication, sessions and roles

**Size** M · **Needs** nothing · **Rows** 12 (S1 0 · S2 3 · S3 9)

Login response carries the roles (KI-145). The rate limit counts only failures and tells the user when to retry (KI-144, KI-002). `scope=openid` once (KI-142). `ranger.auth_token`: stop it from silently locking out stock plugins (KI-148). Let a workspace admin add an SSO user before the first sign-in, or give the user a better landing than "No workspaces yet" (KI-185, needs a decision). Role listings agree between Control Plane and Workspace settings and count inherited permissions (KI-019, KI-127). Disable asks for confirmation (KI-209). Audit log: default filter that hides routine reads (KI-017); name the actor (KI-018). KI-021 is unverified: reproduce first.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-002 | S3 | Auth | Local sign-in is limited to 5 attempts per 5 minutes and rejected (429) attempts keep the window open. There is no message telling the user when they can retry. |
| KI-017 | S2 | Audit log | Every page view writes `*:read allow` rows, so the audit log is dominated by routine reads and hides grant changes. Offer a default filter or separate reads from changes. |
| KI-018 | S3 | Access overview | *Recent changes* identifies the actor by an id prefix (`by 6d60b4c4`) instead of a name. |
| KI-019 | S3 | Roles | Control Plane → Roles lists four workspace roles (including `data_api_invoker`); Workspace settings → Roles & permissions lists three. Verify whether the fourth is hid... |
| KI-021 | S3 | Control Plane | verify: a group (`platform-admins`) mapped from the identity provider gives its member no platform role (Users shows no role for `admin-sso`). Check whether group-to-r... |
| KI-127 | S3 | Workspace settings / Roles | A custom role that inherits shows only its own permissions ("2 permissions") on the workspace Roles & permissions page, while the Control Plane role page counts 8 (inc... |
| KI-142 | S3 | Authentication | The authorization request carries `scope=openid+openid+email+profile`: the default scopes already contain `openid` and the client adds it again.  |
| KI-144 | S3 | Authentication | The sign-in rate limit counts successful attempts (1 success + 4 failures, then 429), which surprises people who sign in a few times in a row.  |
| KI-145 | S3 | Authentication | `POST /api/auth/login` returns `platform_roles: []` and `permissions: []` for the bootstrap `platform_admin`, while `GET /api/auth/me` returns the roles.  |
| KI-148 | S2 | Ranger | `ranger.auth_token` locks out stock Ranger plugins (they cannot send a bearer token; the rendered plugin config has no credential) while the setting reads like a harde... |
| KI-185 | S2 | Authentication | An SSO user lands on *No workspaces yet* after the first sign-in and can do nothing until a workspace admin adds them; Egret only knows the user after that first sign-... |
| KI-209 | S3 | Control plane / Users | **Disable** on a user page acts at once and signs the user out without confirmation, while **Delete** asks for a typed confirmation. |

### Wave 3: UX, with the UI foundation first

#### U1: UI foundation: page structure

**Size** L · **Needs** nothing · **Rows** 6 (S1 0 · S2 0 · S3 6)

Decide once (see Decisions) and put it in shared components: a page container with the agreed widths, one title scale, one tab component with two documented styles, one `StatusBadge`, one entity-link style, one detail header. Migrate every page that deviates; add the Playwright width and title-size assertion that `tests/e2e/data-api-detail-tab-widths.spec.ts` already models. Touches almost every FE route: merge before the other U packages and keep it to mechanical changes.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-214 | S3 | UI consistency | Page titles come in five sizes: 13 px (SQL Lab, the chart editor, an endpoint's detail page), 16 px (an app's viewer), 18 px (every Explore and Governance list), 20 px... |
| KI-215 | S3 | UI consistency | There is no shared page width. Lists and Control Plane pages run edge to edge (content from x=248 to x=1416); Workspace settings starts at x=256; the alert and report... |
| KI-216 | S3 | UI consistency | Five tab styles for the same job. Dataset detail and People & groups use a grey pill strip (40 px high, 6 px radius); the chart editor's **Data / Display** and an endp... |
| KI-222 | S3 | UI consistency | Status badges use three styles. Control Plane lists use a solid dark-green pill (*active* for workspaces and users, *allow* in the audit log); the Explore lists use a... |
| KI-223 | S3 | UI consistency | Names in Control Plane and Governance lists (workspaces, catalogs, users, groups, roles, profiles) are teal links; names in Explore lists (datasets, charts, dashboards... |
| KI-224 | S3 | UI consistency | Detail headers have four forms: back link + coloured icon tile + title + chips (alert, catalog, storage, secret); back link + title, no tile (dataset, endpoint, new en... |

#### U2: UI foundation: controls

**Size** M · **Needs** U1 · **Rows** 4 (S1 0 · S2 0 · S3 4)

Replace the 15 files that render a bare `<select>` with `ui/select` and add an eslint rule (FE-FORM-3 is only "Checked by: review" today) so it cannot come back. Replace the audit log's `datetime-local` inputs with the app's date picker. Make the Notifications page follow the other settings tabs (KI-012). Rebuild the dashboard Filter settings panel from the same parts as the card settings panel (KI-235).

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-012 | S3 | Workspace settings | The Notifications page does not follow the layout of the other settings tabs: the heading is offset, and Encryption is an unstyled native select. |
| KI-217 | S3 | UI consistency | 15 files render a bare browser `<select>` (grey OS chevron, a different height, border and radius from the shared `ui/select` used by 94 files). On screen: the alert a... |
| KI-218 | S3 | UI consistency | Audit log → **From** and **To** are the browser's own `datetime-local` inputs. The format follows the browser locale (here `dd/mm/yyyy, --:--`), the picker looks diffe... |
| KI-235 | S3 | Dashboards | The dashboard **Filter settings** side panel (340 px, see KI-068) is built from bare browser selects and a free **Default value** box, unlike the card settings panel t... |

#### U3: UI foundation: shell, states and small screens

**Size** M · **Needs** U1 · **Rows** 7 (S1 0 · S2 1 · S3 6)

One "no permission" component used by every page (inside the shell, with a way back; KI-230, KI-231) and hide the entries a user cannot open. Governance sidebar without one-item groups, with a label that does not wrap (KI-225). Workspace settings with one Collapse (KI-005). Remove the RisingWave tab and the "later phase" sentence, or mark it clearly (KI-227). Below 1024 px the app sidebar collapses to icons and tables scroll with a visible cue; tab strips scroll or wrap (KI-233, KI-234).

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-005 | S3 | Workspace settings | The Workspace settings page shows two **Collapse** buttons (the app sidebar and the settings sidebar). |
| KI-225 | S3 | Governance | The Governance sidebar is a group per page: **Data**, **People & groups**, **Classification**, **Lineage** and **Engine & policy** each hold exactly one item with the... |
| KI-227 | S3 | Control Plane | Two pages show unfinished surfaces as if they were features. Engines has a **RisingWave** tab next to **Trino** and **StarRocks** and says in its description *RisingWa... |
| KI-230 | S2 | Authorization | A 403 replaces the whole page with a bare card (*403 — Forbidden · You don't have access to this page.*) with no sidebar, no header, no back link and no link to a work... |
| KI-231 | S3 | Authorization | The same refusal is worded four ways. **Endpoints** and **Clients** show a raw red box with the API's word (*forbidden*) under a title bar that still offers **+ Endpoi... |
| KI-233 | S3 | Responsive | At 390 px, content is clipped at the right edge with no visible cue instead of scrolling or wrapping: the dashboards list loses its **Updated** column, the alert edito... |
| KI-234 | S3 | Responsive | At 768 px the app sidebar keeps its full 224 px, so tables lose their width: on **Datasets** the **Database** column wraps *trino-demo (trino)* onto three lines and it... |

#### U4: SQL Lab UX

**Size** M · **Needs** U1 · **Rows** 14 (S1 0 · S2 0 · S3 14)

Keep results and parameters when switching tabs (KI-082); name a saved-query tab after the query; separator when an Explorer click appends a name; a real zero-rows message; say what JSON and Copy export; confirm snippet delete; clear history; New folder; select the tab name on rename; sort the Explorer search results; singular counts; integer ticks in the quick chart; one status announcement; a taller default results panel.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-082 | S3 | SQL Lab | Switching to another tab and back discards the result grid, status and parameter values (the tab body is remounted).  |
| KI-083 | S3 | SQL Lab | Opening a saved query gives a tab titled "Saved" instead of the query's name.  |
| KI-084 | S3 | SQL Lab | Clicking a table or column in the Explorer appends the name with no separator: `SELECT` becomes `SELECTtpch.tiny.nation`.  |
| KI-085 | S3 | SQL Lab | A zero-row result shows the filter message `No rows match “”.`  |
| KI-089 | S3 | SQL Lab | Download JSON and Copy export only the rows loaded in the grid (first 1,000, after filter), while Download CSV exports the full result. Nothing says so except the smal... |
| KI-091 | S3 | SQL Lab | Deleting a snippet has no confirmation (saved queries do).  |
| KI-092 | S3 | SQL Lab | History cannot be cleared and is capped at 50 per user, so test runs push useful entries out.  |
| KI-093 | S3 | SQL Lab | Saved-query folders can only be created from a row's "Move to new folder…"; there is no New folder button as on the Datasets list.  |
| KI-094 | S3 | SQL Lab | The tab rename box does not select the existing text, so typing appends to it.  |
| KI-160 | S3 | SQL Lab | Explorer search lists same-named tables from different schemas in no fixed order (`tpch.sf1000`, `tpch.sf300`, `tpch.sf100000`, `tpch.sf1`, …, `tpch.tiny`), so finding... |
| KI-161 | S3 | SQL Lab | Singular is not handled: the status badge and counter read *Done in 1.1 s · 1 rows* and *1 rows*.  |
| KI-162 | S3 | SQL Lab | The quick **Chart** view of a result uses fractional ticks for counts (`1,833.3`, `3,666.5`, …) and rotates one-letter X labels by 45°.  |
| KI-228 | S3 | SQL Lab | After a run, the same result is announced three times: a green pill in the toolbar (*Done in 1.1 s · 40 rows*), a second pill in the Results tab strip, and a text line... |
| KI-229 | S3 | SQL Lab | The editor takes about two thirds of the page by default, so at 1440×900 the results grid shows 6 rows of a 40-row result and the footer line. |

#### U5: Dashboard UX

**Size** M · **Needs** U1 · **Rows** 10 (S1 0 · S2 1 · S3 9)

A half-configured chart says so on the dashboard (KI-003). A cleared card Title falls back to the chart name (KI-064). Show or let the author edit a filter's slug (KI-065). Search box widget behaves differently from Dropdown (KI-066). Labels for filter types (KI-067). Filter panel must not flip a fixed-width grid to the mobile layout (KI-068). Clamp the drill menu to the viewport (KI-069). No **+ Dashboard** in the Trash (KI-074). Confirm folder delete, trash and revert (KI-075). Print page: tall table cards clipped to their box (KI-168).

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-003 | S2 | Dashboards | A chart that is not fully configured (no X-axis) renders as an **empty card** on a dashboard, with no hint, while the chart editor says *Set up your Bar*. |
| KI-064 | S3 | dashboards / cards | The card **Title** box says "Blank = chart name", but after typing and clearing it the card stores `title: ""`, which hides the caption, the description icon and the t... |
| KI-065 | S3 | dashboards / filters | A filter's slug (used in URLs and `{{slug}}`) is assigned once as `filter`, `filter_2`, … and is neither shown nor editable; renaming the filter does not change it.  |
| KI-066 | S3 | dashboards / filters | The **Search box** widget option has no effect: `FilterControl` renders the same list with a search input for Dropdown and Search box.  |
| KI-067 | S3 | dashboards / filters | The Type select shows internal names (`text`, `temporal_unit`, …) rather than labels such as "Time grouping".  |
| KI-068 | S3 | dashboards / edit | Selecting a card, filter or "Add cards" opens a 340 px side panel. On a fixed-width (1048 px) dashboard the canvas then falls under 753 px, so the grid flips to the on... |
| KI-069 | S3 | dashboards / interactivity | The drill menu is positioned at the pointer without clamping to the viewport, so on a card at the right edge it is cut off.  |
| KI-074 | S3 | dashboards / list | The Trash view still shows the **+ Dashboard** button and the folder count in the summary line.  |
| KI-075 | S3 | dashboards / list | Deleting a folder, moving a dashboard to Trash and reverting a revision happen without confirmation. Deleting a folder moves its dashboards to Uncategorized.  |
| KI-168 | S3 | dashboards / print page | On the rendered print page (PNG/PDF reports) a table card is taller than its 384 px box and runs past the bottom border into the next card.  |

#### U6: Chart editor UX

**Size** M · **Needs** U1 · **Rows** 7 (S1 0 · S2 0 · S3 7)

One-step save for an existing chart (KI-008). Display names on axes, tooltips, sort and drill menus (KI-050). `color` spelling (KI-053). Same axis labels on waterfall and box plot as the other types (KI-054). Trash summary without folders (KI-055). Funnel steps hint (KI-056). Identifier columns not formatted as numbers (KI-058).

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-008 | S3 | Charts | Saving an existing chart takes two steps: **Update** opens a dialog and a second **Update** confirms. |
| KI-050 | S3 | Charts / Editor | Axis titles, tooltip series names, the **Sort** field menu and the drill menu show database names (`revenue`, `orderpriority`), not the dataset's display names (**Reve... |
| KI-053 | S3 | Charts / Editor | Spelling: **Colour** (Progress), **Conditional colours** (Big number), **Switch positive / negative colours** (Trend) against **color** on every other panel.  |
| KI-054 | S3 | Charts / Editor | Waterfall and box plot axes panels say **Show label** where the other types say **X label** / **Y label**, and have **Scale** instead of **Y scale**.  |
| KI-055 | S3 | Charts / Datasets list | The Trash view's summary reads "1 chart · 1 folder" although folders are not shown in the Trash.  |
| KI-056 | S3 | Charts / Funnel | The **Steps** list only appears after the first run, and stage order follows the database order unless a **Sort** or the Steps list is used; nothing in the empty state... |
| KI-058 | S3 | Charts / Formatting | Identifier columns (`orderkey`, `custkey`) are formatted as numbers with thousands separators by default (29,989) in tables and Object detail.  |

#### U7: Alerts and reports UX

**Size** M · **Needs** U1, U2 · **Rows** 6 (S1 0 · S2 0 · S3 6)

Schedule in words (KI-038). Put the "no rows" option where it applies (KI-039). Run log columns (KI-171). Short *Details* text in the editor, long text moved to help (KI-219). One idiom on the Notification tab (KI-220). Deep-link section keys equal the tab labels, unknown keys fall back with a message (KI-221).

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-038 | S3 | Alerts | The Schedule column shows a raw cron (`0 * * * *`) for an hourly schedule, while other schedules are shown in words.  |
| KI-039 | S3 | Alerts | The **Don't send when there are no rows** option sits under Notification, but reports and Data API alerts ignore it; the note says so only after you read it.  |
| KI-171 | S3 | Alerts / Run log | In a report's run log dialog the **Value** column is empty and too narrow: its heading runs into **Detail** ("ValueDetail") and **Started** wraps.  |
| KI-219 | S3 | UI consistency | Settings tables in the alert and report editor put paragraphs in the **Details** column: the Data API alert's **Endpoint**, **Metric** and **Window (seconds)** rows ru... |
| KI-220 | S3 | Alerts | One tab, two idioms. The **Notification** tab of an alert starts with a card list (**Deliver to** → *Email*, *Addresses*, **Add delivery method**) and then, below, a `... |
| KI-221 | S3 | Alerts | Deep links to an editor section depend on the type. `/alerts/new?type=dataapi_alert&section=what` opens the **Condition** tab (the key is `what`, the label is *Conditi... |

#### U8: Data API and Apps UX

**Size** S · **Needs** U1 · **Rows** 9 (S1 0 · S2 1 · S3 8)

Past expiry reads as past (KI-025). Show the publisher's email in Versions, in Data API and Apps (KI-027). Open **Database and parameters** by default (KI-031). Create-button placement (KI-183). Per-row action names in Apps (KI-182). Versions dialog names the restored version and refreshes after publish (KI-184, KI-176). `PUT .../draft` hint and publish-before-save (KI-042). Publish right after Ctrl+S no longer 409s (KI-177).

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-025 | S3 | Data API | Expired client shows `expires 7 years ago` next to the **Expired** badge; a past expiry reads as a future one. A revoked client row has no actions menu while an expire... |
| KI-027 | S3 | Data API / Apps | **Who** in the Versions history shows a shortened user id (`6d60b4c4`) instead of the publisher's email for a platform admin. The Apps version list does the same.  |
| KI-031 | S3 | Data API | **Database and parameters** is collapsed by default on the Query tab, so the parameters table and database picker are hidden until opened; the count in the summary is... |
| KI-042 | S3 | Apps | `PUT .../draft` without `If-Match` returns `If-Match with the draft revision is required`; an API client that only knows the draft body gets no hint of the revision. P... |
| KI-176 | S3 | Apps | The **Versions** dialog is stale after a publish (still *Nothing has been published yet.*) until the page is reloaded: `usePublishApp` (`frontend/src/api/apps.ts:119`)... |
| KI-177 | S2 | Apps | **Publish draft rev N** pressed right after Ctrl+S answers `409 {"error":"the draft changed since it was read","current_revision":2}` (toast *The draft changed while y... |
| KI-182 | S3 | Apps | The Apps list's row menu buttons are all named **Actions**; the Data API lists name them per row (*Actions for Top customers*), so screen-reader users cannot tell the... |
| KI-183 | S3 | Data API | **New service account** puts **Create service account** under the page description on the left; **New endpoint** and **New client** put their create button at the top... |
| KI-184 | S3 | Apps | The Apps **Versions** dialog says *restored from an earlier version* without naming it, although the API returns `restored_from`. Opening a draft-only app shows the ra... |

#### U9: Workflows validation and display

**Size** M · **Needs** U1 · **Rows** 11 (S1 0 · S2 1 · S3 10)

A deleted flow can be restored or purged and frees its id (KI-119). Schema for `DependsOn` matches the engine (KI-120). Warn on unknown keys, `warningOnRetry`, Pause `onResume` and on bad schedule inputs or cron at save (KI-121, KI-122). "Draft" for a never-published flow (KI-123). Next run shows a dash when off (KI-189). Triggers table fits at 1440 px and its header does not wrap (KI-190, KI-226). Not-started tasks are not "Not run" (KI-191). Execution action buttons aligned (KI-193). Version explained before the first one (KI-014).

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-014 | S3 | Workflows | A new flow shows *Version —* in its header, and *Run* says it runs "the saved draft"; it is unclear what a version is until **Create version** is pressed. |
| KI-119 | S2 | workflows | Deleting a flow keeps its namespace/id reserved ("a trashed flow still holds"), with no way to restore or purge.  |
| KI-120 | S3 | workflows | Served schema's `DependsOn` lists `condition`/`inputs`, but the engine reads namespace/flowId/states/labels/when.  |
| KI-121 | S3 | workflows | Unknown top-level keys, unknown task properties, `warningOnRetry` and Pause `onResume` save with no warning.  |
| KI-122 | S3 | workflows | Schedule `inputs` with `{{ }}` and bad cron only fail at the scheduler (Last error), not on save.  |
| KI-123 | S3 | workflows | Flow header shows "Active" for a never-published draft.  |
| KI-189 | S3 | workflows | A schedule that does not fire still shows a **Next run**: a switched-off one shows a time in the past ("6 minutes ago"), one with `disabled: true` in its YAML shows "i... |
| KI-190 | S3 | workflows | The triggers table is wider than the content area at 1440 px: on the **Triggers** page **Backfill** and **Details** are off-screen (sideways scroll, no hint), on a flo... |
| KI-191 | S3 | workflows | On a running execution, tasks that have not started yet are labelled *Not run* in the topology, the same words used for a skipped branch on a finished run.  |
| KI-193 | S3 | workflows | On a running or paused execution the action buttons (**Resume**, **Replay**, **Kill**) are indented about 75 px from the title, with an empty gap, and the *Replay is a... |
| KI-226 | S3 | Workflows | The Triggers table header wraps (**Last evaluated**, **Last execution**, **Next run** break onto two lines), row heights vary from one to three lines because the *Last... |

#### U10: Control Plane, Governance and settings UX

**Size** M · **Needs** U1 · **Rows** 9 (S1 0 · S2 1 · S3 8)

`/control-plane/config` redirect loop (KI-016). Grant and rule rows are links with labelled actions (KI-131). The New grant dialog fits a 900 px viewport (KI-132). User search list closes after a pick (KI-203). Register engine lands with a readable message, not raw API text (KI-204). No console 404s on Notifications (KI-212). A Trino without `process-forwarded` gets a message that names the setting (KI-001). Connection status after Test (KI-011). KI-007 (Engines vs databases): needs a vocabulary decision; apply it to labels only.

| Row | Sev | Area | Finding |
|---|---|---|---|
| KI-001 | S2 | Databases | Registering a dataset against a Trino without `http-server.process-forwarded=true` fails with a generic *502 Bad Gateway*. Egret always sends `X-Forwarded-Proto` and T... |
| KI-007 | S3 | Naming | The UI calls them *Engines* (Workspace settings → Engines), the API and parts of the code call them *databases*, and the Control Plane has its own *Engines*. Pick one... |
| KI-011 | S3 | Databases | A newly created connection shows status *Unknown* until **Test** is pressed, even though creation already validated reachability. *Docker demo stack (Oct 2026):* after... |
| KI-016 | S3 | Control Plane | `/control-plane/config` renders a completely blank page inside the shell. *Docker demo stack (Oct 2026):* no longer blank; it now redirects without end to `/control-pl... |
| KI-131 | S3 | Governance / Data | Grant and rule row actions are icon-only (pencil, bin) and the row text is not a link. The single grant/rule page (`/governance/grants/<id>`) has no visible link from... |
| KI-132 | S3 | Governance / Data | The New grant dialog taller than a 900 px viewport scrolls inside the dialog and the Preview result sits below the fold.  |
| KI-203 | S3 | Governance / Engine permissions | In **Grant engine permission** the user search list stays open after a pick and covers the **Grant** button until you click elsewhere. |
| KI-204 | S3 | Control plane / Engines | After **Register engine** without a profile you land on Plugin config with the raw API text *conflict: engine trino-demo-ext is bound to no policy profile: bind a prof... |
| KI-212 | S3 | Workspace settings / Notifications | Before settings exist the page loads `GET /notifications/smtp` and `/notifications/slack`, which answer 404 and show as console errors. |

## Deferred

Not in this programme, by the user's decision: the 53 open **feature** rows (Type *Gap*, *Missing*, *Limitation*): KI-015, KI-028, KI-029, KI-030, KI-035, KI-037, KI-041, KI-156, KI-170, KI-178, KI-179, KI-181, KI-186, KI-048, KI-052, KI-062, KI-070, KI-071, KI-073, KI-076, KI-077, KI-080, KI-090, KI-097, KI-104, KI-106, KI-107, KI-108, KI-113, KI-114, KI-115, KI-116, KI-117, KI-118, KI-124, KI-188, KI-125, KI-133, KI-134, KI-135, KI-202, KI-205, KI-208, KI-210, KI-137, KI-138, KI-139, KI-143, KI-147, KI-149, KI-150, KI-153, KI-232; and the 8 open **docs** rows: KI-009, KI-010, KI-172, KI-174, KI-109, KI-192, KI-151, KI-155. They start after Wave 3, from their own plan. Three of the feature rows are security hardening and worth moving forward if the user agrees: KI-147 (unauthenticated `/metrics`, `/healthz`, `/readyz`), KI-179 (session cookie without the `__Host-` prefix on plain http) and KI-205 (impersonated decisions are audited under the impersonated user).

## Risks

- **U1 touches every route.** Keep it mechanical, merge it alone, and rebase the other FE packages straight after. The Playwright width and title checks are what make the next package safe.
- **Behaviour changes need a decision, not a patch** (D6, D8, D9). A package that reaches one of them stops and asks; it does not pick silently.
- **Rows found on an older instance may be gone.** The re-verify step costs minutes and saves a fix for a problem that no longer exists.
- **The Docker provisioner packages (B6) change container lifecycle.** They are verified on the real stack and never against the shared `egret-dev-*` or `egret-s4-*` containers.
- **Two packages in parallel share the demo stack.** Prefix any data a package creates and never delete what it did not create.

## Coverage

Every one of the 165 rows is in exactly one package (checked by the script that generated this appendix). Row to package:

| Pkg | Rows |
|---|---|
| F1 | KI-159 |
| F2 | KI-129, KI-206, KI-207 |
| F3 | KI-013, KI-187 |
| F4 | KI-112, KI-146, KI-152 |
| F5 | KI-059, KI-060, KI-061, KI-063 |
| F6 | KI-136, KI-140, KI-141 |
| B1 | KI-004, KI-006, KI-078, KI-079, KI-081, KI-086, KI-087, KI-095, KI-154, KI-211 |
| B2 | KI-096, KI-098, KI-099, KI-100, KI-101, KI-102, KI-103, KI-105, KI-110, KI-111, KI-158, KI-166 |
| B3 | KI-043, KI-044, KI-045, KI-046, KI-047, KI-049, KI-051, KI-057, KI-072, KI-163, KI-164, KI-165 |
| B4 | KI-032, KI-033, KI-034, KI-157, KI-167, KI-169, KI-173 |
| B5 | KI-023, KI-024, KI-026, KI-180 |
| B6 | KI-194, KI-195, KI-196, KI-197, KI-198, KI-199, KI-200, KI-201, KI-213 |
| B7 | KI-002, KI-017, KI-018, KI-019, KI-021, KI-127, KI-142, KI-144, KI-145, KI-148, KI-185, KI-209 |
| U1 | KI-214, KI-215, KI-216, KI-222, KI-223, KI-224 |
| U2 | KI-012, KI-217, KI-218, KI-235 |
| U3 | KI-005, KI-225, KI-227, KI-230, KI-231, KI-233, KI-234 |
| U4 | KI-082, KI-083, KI-084, KI-085, KI-089, KI-091, KI-092, KI-093, KI-094, KI-160, KI-161, KI-162, KI-228, KI-229 |
| U5 | KI-003, KI-064, KI-065, KI-066, KI-067, KI-068, KI-069, KI-074, KI-075, KI-168 |
| U6 | KI-008, KI-050, KI-053, KI-054, KI-055, KI-056, KI-058 |
| U7 | KI-038, KI-039, KI-171, KI-219, KI-220, KI-221 |
| U8 | KI-025, KI-027, KI-031, KI-042, KI-176, KI-177, KI-182, KI-183, KI-184 |
| U9 | KI-014, KI-119, KI-120, KI-121, KI-122, KI-123, KI-189, KI-190, KI-191, KI-193, KI-226 |
| U10 | KI-001, KI-007, KI-011, KI-016, KI-131, KI-132, KI-203, KI-204, KI-212 |
