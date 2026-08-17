# Data API P6 — closing the authoring loop

Amends `2026-08-16-data-api-p5-design.md` §2.4 (the detail route's sections) and adds one serving-adjacent route. Where the two disagree, this document governs.

## 1. Why this exists

Writing a Data API endpoint today costs a round trip through the UI for every iteration. The SQL lives in `Definition`, which also holds cache, limits and rate limit, so the editor is squeezed between three configuration groups. To see whether the query returns the right rows the author leaves for `Try it`, reads the result, and comes back — and `Try it` runs the **published** version, so a draft edit is not even visible there.

So the loop an author actually runs — write SQL, look at rows, adjust — is broken in two places at once: the editor has no room, and nothing on the page can show what the query returns.

Two references settle the shape. **pydbapi** (`data-portal/pydbapi`, the same product built independently) splits its endpoint editor into `API Configuration` and `API Execution Content` — the query gets a surface of its own. And egret's own **ChartExplore** is a studio: controls on one side, a live preview of the result on the other, updating as you work. The second pattern is the one this codebase already proved; this spec applies it to endpoint authoring.

**This is a UX change with one new backend route.** It adds no capability to what a published endpoint can do: no script engine, no multi-statement SQL, no new parameter locations. Those are separate projects and are deliberately out of scope.

## 2. The preview route

```text
POST /api/ws/{workspace}/data-api/endpoints/{id}/preview
```

Gated on **`data_api_endpoint:manage`** — the same gate `validate` carries, and for the same stated reason: this runs a query against the engine under the caller's identity, which is an authoring action rather than a read of metadata.

### 2.1 It runs as the author, not as a service account

`validate` already does exactly this. `validated_draft` (`crates/egret-dataapi/src/routes.rs:1096`) passes `ctx.principal.user_id` into `validate`, whose comment records why: `describe_as` resolves under the caller's own identity, so a delegated connection applies **their** policy and an author cannot publish a contract over a table they cannot themselves see.

Preview keeps that identity and changes only the verb — `execute_as_with_params` (`crates/egret-query-api/src/port.rs:196`) instead of `describe_as`.

**The consequence must reach the UI, not just this document.** A green preview does **not** prove a consumer can call the endpoint: the service account is a different principal and Ranger may refuse it. That is the question `Try it` answers, and it is why both exist rather than one being redundant.

### 2.2 The body carries the on-screen draft, not just test values

```jsonc
{
  "sql": "SELECT …",                        // what is in the editor now
  "params": [ /* ParamSpec[], as declared on screen */ ],
  "values": { "region": ["EU"] },           // the author's test input
  "database_id": "…"                        // the picker's current value
}
```

**Reading the saved draft from the database instead would defeat the feature.** The author would have to Save before every preview, and the loop would still be cut — by a button rather than by navigation. The point is to see what is on screen.

This opens no new attack surface. SQL Lab already executes arbitrary author-supplied SQL at an equivalent permission, and the payload here passes through the same gates publish does (§2.3).

### 2.3 It must reuse serving's render path, not a second one

Preview renders through **`check_render_rules` and the behavioural parameter gate** — render twice, once with typed opaque values and once with the same values as plain strings, and refuse on divergence — then binds exactly as serving binds.

This is not defensive boilerplate. P4.5 spent five independent review rounds on a byte scanner that disagreed with minijinja's lexer about the same bytes, and the fix was to stop scanning and start comparing rendered output. A second render path is where that class of bug comes back, so preview does not get one.

Note the difference from `validate`, and why it matters: `validate` renders with `representative(&draft.sql, &draft.params, &[])` — synthetic stand-in values, because it has no real input. Preview has the author's real values and renders with those. Both go through the same gate.

### 2.4 Four things preview does not do

Preview is not a consumer call, and every one of these would make it lie if it were included:

- **No `access_log` row.** Activity measures real traffic; a preview click is not traffic. Writing one would inflate call counts and skew every percentile on that page.
- **No rate-limit consumption.** An author iterating must not throttle a live consumer.
- **No cache read or write.** Reading would show a stale answer for SQL that was just edited; writing would poison what consumers read with a result produced under a different principal.
- **`limits.max_rows` does not apply.** Preview caps at **100 rows**, fixed and not configurable, because the author wants the shape of the data rather than a million rows in a browser. The number is named here so it is not left to the implementer, and the UI must state when it truncated — a capped preview must never read as a small result set.

### 2.4.1 Preview shows the engine's own column names and the engine's own errors

Two deliberate differences from what a consumer sees, both because preview serves a different reader.

**Column names come back as the engine returns them — `output.naming` is not applied.** An author debugging SQL needs the names they wrote; `Response` is where the published contract, naming transform included, is inspected. Say which is which in the UI or the two will be confused.

**Engine errors surface verbatim.** Serving deliberately maps an engine permission failure to `data_access_denied` and never echoes the engine's text, because a consumer must not learn the shape of the tables. Preview is the opposite case: the author owns the SQL, is running as themselves, and cannot fix a syntax error from a sanitised message. So preview returns the engine's message as-is — and that is precisely why it must never be reachable below `data_api_endpoint:manage`.

### 2.5 Not audited

`validate` emits no audit event, and preview matches it. Both run a read-only query as the caller themselves — the same thing SQL Lab does, which is also unaudited. This is deliberate and stated here so its absence does not read as an oversight: the audited actions are the ones that change what consumers receive (publish, rollback, disable, enable, delete) plus impersonation (`try`).

## 3. The detail route's sections

Six sections, divided by the question each answers:

```text
Query · Config · Response · Access · Try it · Versions
```

### 3.1 `Query` — everything that decides what the query returns

The index route. Holds the database picker, the SQL editor, the parameter **declarations**, and the preview.

**Layout is vertical — editor above, results below — following SQL Lab, not ChartExplore's left/right split.** ChartExplore owns the full page width; a section inside the detail route does not, and a right-hand results column would squeeze both halves.

**Parameters carry two roles here, and merging them is the trap.** A parameter has a *declaration* (name, type, required, default, allowed) and, for preview only, a *test value*. Putting the test value in the same row as the declaration invites the reader to think it is saved with the endpoint. So `ParamEditor` keeps declarations, and the preview block has its own input per declared parameter, the way `Try it` does. That duplicates `Try it`'s input row, which is acceptable: the two sections are never on screen together, and they run as different principals.

### 3.2 `Config` — everything about the endpoint as an HTTP object

Four groups, each with its own heading:

1. **Endpoint** — name, description, method, path. The URL contract.
2. **Cache** · 3. **Limits** · 4. **Rate limit** — the `Setting | Details | Value` tables from #513, unchanged.

### 3.3 Unchanged

`Response`, `Access`, `Try it` and `Versions` keep their current content and behaviour.

### 3.4 Three buttons, three different relationships with `dirty`

The detail route shares one form across all sections, and after this change the same page carries three actions that disagree about unsaved changes:

| Action | With unsaved changes | Runs |
|---|---|---|
| **Preview** | Enabled — this is the point | What is on screen |
| **Validate** | Disabled | The saved draft |
| **Publish** | Disabled | The saved draft |

Because they differ, **each button must say which it runs.** "Preview" without that label invites an author to preview, see green, publish, and get something else — the failure this whole spec exists to prevent.

## 4. What this does not change

Stated positively so it cannot be read as an omission:

- The serving path is untouched. No new capability reaches a published endpoint.
- **Single-statement SQL stays enforced.** pydbapi allows multi-statement; egret refuses a second statement at publish, and P3 had a SEV-0 because a `;` slipped through a scanner. Relaxing that is a separate project with its own threat model.
- **Parameters stay query-only.** `location: body` is still refused and there is still no `location` control.
- No script engine.

## 5. Testing

Every backend test here is DB-gated: missing `DATABASE_URL` makes it report `ok` while asserting nothing, so run against `egret_test` and read the duration.

- **Preview runs as the author, not the service account.** With a connection whose engine reports the session user, assert the returned rows name the *caller*. A preview that ran as a service account would be `Try it` wearing the wrong label.
- **Preview reflects unsaved SQL.** Send a body whose `sql` differs from the stored draft and assert the result matches the body. This is the property §2.2 exists for, and it fails if anyone later "simplifies" the handler to read the draft.
- **Preview writes no `access_log` row.** Count rows before and after; the count must not move. Then mutate: write a row deliberately and confirm the test fails.
- **Preview does not populate the cache.** Preview, then call the endpoint as a service account, and assert the serving response is `cached: false`.
- **Preview refuses what publish refuses.** A parameter payload the behavioural gate rejects must be rejected here too, with the same error — proving preview did not get its own render path.
- **Preview surfaces the engine's message.** Feed the port a syntax error and assert the author sees the engine's own text, not `data_access_denied`. This is the one place the sanitising rule is deliberately inverted, so it needs a test saying so.
- **Frontend: Preview stays enabled while dirty, Validate and Publish do not.** One test, three assertions; it pins §3.4 as a set rather than one button at a time.

Assertions get mutation-checked before being believed. And a mutation that fails to compile proves nothing — build first, confirm zero errors, then read the result.

## 6. Decomposition

1. **The preview route** — backend, with the tests in §5.
2. **`Query` section** — split SQL, database and parameter declarations out of `Definition`; wire the preview block.
3. **`Config` section** — move name/description/method/path in beside the three existing config tables; retire `Definition`.
4. **The three-button labelling** (§3.4) — small, but kept separate so it is not lost in a large diff.

Step 1 ships independently and is testable on its own. Steps 2 and 3 together are what retires `Definition`, so they should land close to each other to avoid a release where the same field is editable in two places.

## 7. Deferred, with the reason

These came out of the pydbapi comparison and are **not** in this spec:

- **Python script engine** (RestrictedPython sandbox, `db`/`http`/`cache`/`tx` context objects) — the largest capability gap, and a new attack surface inside a product that already runs privileged engine queries. Needs its own threat model.
- **Multi-statement SQL** — see §4.
- **Parameter locations** beyond query — header, body, path.
- **Concurrency limit** (max in-flight per client). egret rate-limits but does not cap concurrency, so one slow caller can still occupy the pool.
- **Result transform**, **macro definitions**, **regex/script parameter validation**, and the richer Jinja filter set (`sql_like`, `in_list`, `compare`, `{% where %}`).
- **Public (unauthenticated) endpoints.**
