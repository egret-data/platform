# Governance Overview: live measurements

> Canonical (English). Plan: `docs/en/plans/2026-10-18-dg-overview.md`. Spec:
> `docs/en/specs/2026-10-18-dg-overview-design.md` (§4.3, decision O9). Branch `feat/dg-overview`, worktree
> `egret-dgov`.

## Measurements (Task 1)

The question: does the Ranger plugin download ledger (`policy.ranger_plugin_downloads`) say what the Overview's
drift rules (`plugin_silent`, `plugin_behind`, `plugin_quiet`) assume? Four premises of spec §4.3 were never
measured: which rows each engine writes and under which service, whether a plugin id is stable, how long a version
takes to be reported as held, and what moves `authz_services.updated_at`.

### 1.0 Predictions (committed before anything was observed)

Written from the code at `e7116931` (main `4a67321b` plus the spec and plan commits, which change no code), before
any container of this run was started. A wrong prediction is a finding. Nothing in this subsection is edited after
observing.

**Setup the predictions are about.**
- The stack of `docs/en/plans/2026-10-17-dg-people-groups-live.md` §2.2, reduced to what the ledger needs:
  - an egret image built from this branch;
  - throwaway DB `egret_dgov_live` in the private `dgov-pg`;
  - own containers `vg-*` on network `vg-net` (`vg-egret`, `vg-rustfs`, `vg-trino-coord`, `vg-sr`);
  - Trino 483 with JWT and TLS (profile `vgtr`, `tagService = vgtr-tag`, host `vgtr-coordinator-6d5f7c9b8-ab1cd`);
  - StarRocks 4.1.6 allin1 with `access_control = ranger` (profile `vgsr`);
  - Ranger poll 5000 ms on both (rendered: 30000);
  - engine rows inserted as `provisioned`, `running` (SQL, recorded as DEV-n). No catalog is needed.
- The plugin id is `<appId>@<hostname>-<serviceName>` (view-governance live result, the `pluginId` cell).

**Cells.**

| id | check | prediction |
|---|---|---|
| L1a | rows Trino writes | exactly 2: `(trino@vgtr-coordinator-6d5f7c9b8-ab1cd-vgtr, policies)` and `(…, tags)`, both under `service_id` of `vgtr` (the resource service). None under `vgtr-tag` |
| L1b | rows StarRocks writes | 1: `(starrocks@<container id>-vgsr, policies)` under `vgsr`. No `tags` row (not confident: the StarRocks plugin may still start a tag retriever) |
| L1c | `last_known_version` once settled | equals `policy_version` (and `tag_version` for Trino's `tags` row) of the polled service |
| L2a | `docker restart` of each engine container | the plugin id does not change (the container keeps its hostname): no new row, `at` moves on |
| L2b | the same engine run under a new hostname (a rescheduled pod) | a new row with a new id; the old row stays with its last `at` |
| L3 | lag from `POST /perm/compile/{id}` that writes something (a grant create) to `last_known_version = policy_version` | ≤ 10 s on both engines at the 5000 ms poll (one poll plus the apply); so ≤ ~35 s at the default 30 s poll, and 90 s is enough |
| L4 | statements that set `authz_services.updated_at` without a version bump | some exist (profile config/description update); one confirmed live moves `updated_at` and leaves `policy_version` alone |
| L5 | StarRocks stopped for ≥ 2 min | its rows keep their last `at`; no row is deleted (no poll of that service means no prune of it) |

**Stop rule (plan Task 1).** If L1 shows Trino's `policies` rows under a service other than the bound profile, or
L2 shows ids that change on every poll: stop and report BLOCKED. Any other wrong prediction is a finding for the
drift rules' wording or thresholds.

### 1.1 Result (read this first)

**Not blocked.** Neither stop condition fired: Trino's `policies` (and `tags`) rows land under its bound resource
profile, and plugin ids are stable across a container restart. Three findings change how Task 7 must code the drift
rules:

1. **Confirmation takes two polls, not one.** A plugin reports `lastKnownVersion` = the version it applied on the
   previous poll. After a bump, the first poll downloads the new version and still reports the old one; the next
   poll reports the new one. Measured at the 5000 ms poll: 7.1 s (StarRocks) and 8.7 s (Trino). At the rendered
   30 000 ms poll that is up to ~60 s plus apply time: **90 s holds, with ~30 s of margin, not 60 s.**
2. **Trino's `tags` row regresses to `-1` after every policy download.** The tag retriever restarts when the
   policy engine is rebuilt, so the first tag poll after a `policies` download reports `lastKnownVersion = -1`,
   and the next one (30 s later; the tag poll ran every 30 s although the policy poll was 5 s) reports the real tag
   version again. A `plugin_behind` check on the `tags` kind that reads one row's current value can therefore see
   `-1 < V` for up to one tag interval after any *policy* bump. With the 90 s threshold measured from `updated_at`
   this lands just inside the window at the default 30 s poll (download ≤ U+30 s, `-1` poll ≤ U+60 s, recovery
   ≤ U+90 s). Task 7 should not judge the `tags` kind from a `-1` alone (treat `-1` as "not yet reported", or give
   the tags kind its own threshold of ≥ 120 s).
3. **An old plugin id's rows stay forever (in practice).** A container recreated under a new hostname (a
   rescheduled pod) writes new rows; the old id's rows keep their last `at` and are pruned only after 24 h, and only
   when that service is polled. So `plugin_quiet`/`plugin_silent` must use the **newest** `at` per profile and kind,
   never "any row older than N" — otherwise every pod reschedule raises a false `plugin_quiet`.

L4: every statement that sets `authz_services.updated_at` also bumps a counter, but the two counters share the one
`updated_at`: a `policy_version` bump moves it with `tag_version` unchanged (seen live), and a `tag_version` bump
moves it with `policy_version` unchanged (`tag_version.rs:116`). For the other kind that is an `updated_at` move
without a version bump: it can only delay `plugin_behind`, as the spec says. L5 holds (rows of a stopped engine keep
their `at`; nothing deletes them).

### 1.2 Stack and deviations

- **Image.** `egret-backend:vg-dgov1`, built from `df94fbb3` (the predictions commit; no code change since
  `4a67321b`). Warm release build, 15 min. Free disk at its lowest: 128 GiB (floor 30 GiB).
- **DB and containers.** DB `egret_dgov_live` in the private `dgov-pg` (attached to `vg-net` for the run):
  `migrate`, `init`, `serve`. Log `kubernetes unavailable; engine provisioning disabled`; no `ERROR` line in the
  egret log. Containers on `vg-net`: `vg-egret` (`catalog.public_url = http://vg-egret:8096`), `vg-rustfs`
  (`rustfs/rustfs:1.0.1`), `vg-trino-coord` (Trino `db58cc93…` = 483, host `vgtr-coordinator-6d5f7c9b8-ab1cd`, the
  JWT/TLS rendering of the #1 live doc with a fresh self-signed leaf), `vg-sr` (allin1 `5cbf09c4…` = 4.1.6,
  `access_control = ranger` appended to `fe.conf`, the two XMLs copied to `fe/conf`, restarted).
- **Profiles through the API:** `vgtr-tag` (tag, id 1), `vgtr` (trino, `tagService = vgtr-tag`, id 2), `vgsr`
  (starrocks, id 3).
- **DEV-1 (SQL): engine rows** `vgtr` → 2 and `vgsr` → 3, `provisioned`, `running`, `spec = {}`. The insert itself
  bumped both profiles' `policy_version` 1 → 2 (the `0187` lineage trigger on `engine_mgmt.engines`).
- **DEV-2 (SQL): catalog.** A storage connection, warehouse `vgice`, registry catalog `ice` and its attachment to
  both engines, inserted directly (no data is needed for the ledger). The attachment insert bumped nothing.
- **Username through the API:** `POST /perm/people/users/{admin}/username` → `admin`, so a grant can name it.
- **DEV-3 (L3 trigger).** L3 used `POST /perm/grants` (allow `read` on `ice.s.t` to `user admin`), whose
  transaction recompiles both profiles (`created: 2`), not a bare `POST /perm/compile/{id}`: a manual compile of
  an unchanged model writes nothing and bumps nothing, so it has no lag to measure. Both run the same funnel.
- **DEV-4 (config).** Ranger policy poll 5000 ms on both engines (rendered: 30000). The Trino tag poll was not
  configured; it ran every 30 s.
- **L2b** recreated `vg-trino-coord` under host `vgtr-coordinator-6d5f7c9b8-zz9yy` to model a rescheduled pod.

### 1.3 Every cell

| id | prediction | observed | verdict |
|---|---|---|---|
| L1a | 2 Trino rows under `vgtr`, none under `vgtr-tag` | `trino@vgtr-coordinator-6d5f7c9b8-ab1cd-vgtr` × (`policies`, `tags`), both `service_id = 2`; nothing under 1 | as predicted |
| L1b | 1 StarRocks `policies` row, no `tags` | `starrocks@5c44f099b016-vgsr` / `policies` under 3; no `tags` row in the whole run | as predicted |
| L1c | `last_known_version` = the polled counter | `policies` = `policy_version` (3 after L3); Trino `tags` = `tag_version` (1), **except** `-1` for one tag poll after each policy download (15:46:15, 15:49:55) | finding 2 |
| L2a | restart keeps the id | after `docker restart` of both: the same three ids, `at` moved on, no new row | as predicted |
| L2b | a new hostname writes a new row; old row stays | `…-zz9yy-vgtr` × 2 new rows; the `…-ab1cd-vgtr` rows kept `at` 15:53:51 / 15:53:46 (329 s old at the last read) | as predicted; finding 3 |
| L3 | ≤ 10 s at 5000 ms | bump at 15:49:51.906; StarRocks downloaded at 15:49:53.961 (reported 2), confirmed 3 at 15:49:58.976 (**7.1 s**); Trino downloaded 15:49:55.649, confirmed 15:50:00.649 (**8.7 s**) | as predicted; finding 1 (two polls) |
| L4 | some statements move `updated_at` without a bump | none in the tree: every `UPDATE policy.authz_services … updated_at` also bumps `policy_version` (`store.rs:2051/2114/2213/2265/2385/2436`, `trigger.rs:1239`, `0182`, `0187`, `0190`, `0191`) or `tag_version` (`tag_version.rs:116`); `trigger.rs:1119` and `0176` change other columns without touching `updated_at`. Live: the L3 grant moved `updated_at` to 15:49:51.906 with `tag_version` still 1 | partly wrong; only the shared-counter case exists |
| L5 | stopped StarRocks keeps its row | stopped 15:56:09; at 15:59:20 the row still had `at` 15:56:04.092 (197 s old), not deleted | as predicted |

### 1.4 Left behind

Nothing. Removed: containers `vg-egret`, `vg-trino-coord`, `vg-sr`, `vg-rustfs`; network `vg-net` (`dgov-pg`
disconnected first); DB `egret_dgov_live`; image `egret-backend:vg-dgov1`; scratch env file, TLS key and keystore,
cookie jar and engine config copies. `dgov-pg` and `dgov-valkey` stay for Tasks 2–5.
