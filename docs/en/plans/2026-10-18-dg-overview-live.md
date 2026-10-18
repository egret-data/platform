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
