# B5: Data API correctness (KI-023, KI-180, KI-024, KI-026)

Package B5 of the known-issues fix programme
(`docs/superpowers/plans/2026-10-21-known-issues-fix-programme.md`). All four rows reproduced on the demo stack
(main `f3dad9f6`) before any change.

## KI-023: a draft's new path must not change the served URL

**Premise.** `PATCH …/endpoints/{id} {"path":"b5/moved"}` on a live endpoint at `/b5/probe`: the old URL answered
`404 not_found`, the new one `200` with `meta.version` 1 (the live version's SQL). The router matched the draft's
`dataapi.endpoints (method, path)`.

**Decision.** The router matches the route of the LIVE version. Migration 0204 adds `live_method` / `live_path` to
`dataapi.endpoints`, written only by the two writers of `live_version_id` (publish, from the snapshot it freezes;
rollback, from the snapshot it makes live), with a partial unique index on `(workspace_id, live_method, live_path)`.
A column pair rather than a join through `endpoint_versions.snapshot`, because the URL needs a unique constraint:
without it, an endpoint whose draft moved away from `/x` plus a second endpoint published at `/x` would be two live
answers to one URL. That second publish is now a 409 naming the URL. The draft's own `UNIQUE (workspace_id, method,
path)` is unchanged.

Backfill: from the live snapshot, except where another endpoint already holds that URL (draft row or snapshot), in
which case the row keeps the URL it is served on today. That makes the index satisfiable on any existing data.

Readers that follow: the serving router, the generated OpenAPI document, Try it (the trial calls the live URL). The
endpoint DTO carries `live_method` / `live_path`; the endpoint list shows the live URL with *Pending publish: METHOD
/path* under it when the draft differs; Config says *The new path goes live when you publish. Callers still use
METHOD /path.* under the Path (and Method) field. Old URL after publish: 404, no redirect.

## KI-180: "Revoking will not remove access" on every granted account

**Premise.** Granted accounts `b5-norole` (no workspace role) and `b5-wsrole` (member as `data_api_invoker`) both
carried the note. The row read it from the per-endpoint OpenAPI probe, which answers yes for any granted account.

**Decision.** The server answers it: `GET …/grants` returns `inherits_invoke` per grant, asked of the PDP at
WORKSPACE scope (`invoke::inherits_invoke`), where an object-scoped grant cannot count and a workspace or platform
role does. The row shows the note iff `inherits_invoke`, on any endpoint state (the old "Revoking may not remove
access" fallback for a failed or impossible probe is gone: the answer no longer depends on the probe). The probe
still runs, only for accounts without a grant.

## KI-024: breaking-change badge overlaps the note

The Version cell is `whitespace-nowrap` in a `w-32` column; `v2` + `Live` + `1 breaking change` is about 200 px.
The cell now wraps its chips (`flex-wrap`, `whitespace-normal`) and the column is `w-40`, `w-56` at `xl`.
Checked in the browser at 1440 and 1024 px.

## KI-026: unlabeled constraint boxes

Each constraint box (number: Minimum, Maximum; string: Min length, Max length, Pattern (regex); date/timestamp: Not
before, Not after) gets a visible `Label` tied by `htmlFor`/`id` (FE-FORM-2). The row-specific `aria-label` stays.
