# SP4c — classification clearance must reach the data plane

**Status:** design, approved in principle 2026-09-28.
**Follows:** SP4a (the permission model, #596) and SP4b (its UI, #600/#601).

## The defect this exists to close

A classification's clearance is enforced when someone **clicks the object** and not enforced when they **query the column**.

Cedar compiles a tag's `required_roles` / `required_groups` into a `forbid` policy (`crates/egret-auth/src/pdp/mod.rs:159-206`, template at `pdp/templates.rs:77-98`) which runs on **control-plane object actions** — opening a dashboard, a dataset. The permission model gates the **data-plane query** through Ranger. SP4a's Rules select by tag but read only the tag's `key`: a grep finds **zero** references to `required_roles` or `required_groups` anywhere in `egret-perm` or `egret-perm-compile`.

So the two mechanisms can disagree about the same column, and nothing reconciles them. A user whom Cedar forbids from opening a tagged dataset can read the same data through SQL Lab or a notebook if they hold a read grant. **Effective access is the union of whichever path the user takes, not the intersection.**

**User decision (2026-09-28): clearance must reach the data plane.** A tag requiring role X means that column is refused at the engine to anyone without X. Effective access becomes the intersection.

## D0 — What must be measured before anything is designed

SP4a's D7 measured that **groups never reach an engine**: Trino has no group provider, egret writes none, there is no usersync, so a Ranger policy item naming a group matches nobody. That measurement is why the workspace-assignment grant compiles to an enumerated member list instead of a group.

**Nobody has measured the same question for roles.** Ranger has a `roles` bucket and the perm model already has `PrincipalKind::Role`, so the obvious implementation names the role and stops. But Ranger roles are defined **in Ranger**, and egret pushes nothing to Ranger. If a Ranger policy item naming a role matches nobody for the same reason a group does, then naming the role is a control that silently does nothing — and this whole design collapses into "enumerate the role's members and recompile when membership changes", exactly as SP4a had to do for workspace members.

**This is task zero, and it decides the rest.** Measure it the way D7 was measured — by mechanism, not by a failed query, because a denied query has many possible causes. Establish whether the Trino plugin resolves a role name at all, and where it would get the membership from. Write the result into this spec before any code is written.

## D1 — Clearance compiles to a tag-based deny, which amends SP4a's D2

SP4a's D2 says **tags drive rules only, never access**. Clearance is the exception, and this spec amends D2 rather than quietly contradicting it: a classification whose whole purpose is to withhold data cannot be expressed as a mask or a row filter, because both still answer the query.

Clearance compiles to a **deny on the tag service** for the tagged resource, with the cleared principals in `denyExceptions`. Ranger's four-way allow / deny / allowExceptions / denyExceptions shape exists precisely because there is no principal set-difference, and "everyone except the cleared" is the case it was built for.

Two properties of Ranger make this the right layer, and both were established during SP4a:
- **Tag policies are evaluated first and short-circuit**, so a clearance denial wins over any resource grant. That is what clearance must do.
- **The tag service is the compiler's**, since SP4a's Important 5 claimed it — a hand-written tag ALLOW there could otherwise short-circuit ahead of a compiled deny.

## D2 — `required_groups` cannot be enforced at the engine, and the UI must say so

D7's measurement already settles half of this: a group named in a Ranger policy matches nobody. So a tag gated **only** by `required_groups` cannot be enforced at the data plane today, whatever D0 concludes about roles.

That is not a reason to silently skip it. The classification UI must say, where the clearance is set, that a group-only gate is enforced on object access and **not** in the engine — the same shape as SP4b's group warning, and for the same measured reason. A control that appears to withhold data while withholding nothing is the failure this programme keeps refusing to ship.

## D3 — Three new recompile triggers, because clearance has three inputs

A compiled clearance deny depends on the tag's gate, on which columns carry the tag, and on who holds the clearing role. So it goes stale from three directions:
- a tag's `required_roles` / `required_groups` changing — **new**
- a tag assignment changing — **exists** (SP4a Task 7)
- membership of a clearing role changing — **new**, and only if D0 concludes we must enumerate members

Each must recompile in the same transaction as its write, using the seam SP4a established: affected services expanded through `services_and_tag_services_via`, locked ascending, with the `debug_assert!(service_ids.is_sorted())` gate. SP4a shipped the "a write without its recompile" defect on four separate paths, so the enumeration of writers comes before the fix, not after.

## D4 — `explain` must name a clearance denial as its own reason

`explain` today would answer `no_grant_matched` for a subject blocked by clearance, which is the wrong administrative answer: nobody needs to add a grant, somebody needs clearance. It gains a reason class of its own, naming the tag.

This also narrows an existing limit. `explain` is blind to hand-written policy and will stay so in this spec — but it must not be blind to a mechanism this spec itself adds.

## D5 — The intersection property is the acceptance test

The point of the whole spec is one sentence: **a user without clearance cannot read a tagged column, by any path.** The test is therefore not that a policy was written, but that the read fails — through SQL Lab and through a notebook, since the notebook path is what made the union visible in the first place.

Live verification is required and is not satisfied by unit tests. SP4a shipped three Criticals that ~200 green unit tests passed over, each found only by running a real engine, and each of the same shape: a policy that compiled, deployed, and enforced nothing.

## Deliberately not in SP4c

- **Migrating hand-written policies** — now SP4d. The survey found structural blockers, not just volume: the delegator policies (`impersonate` on `trinouser`, `alter` on `systemproperty`) are inexpressible in a model scoped to catalog/schema/table/column, and `delegateAdmin`, override priority and `additionalResources` are all hardcoded off in the compiler. It also needs `explain` to stop being blind to hand-written policy first, or the migration cannot be previewed.
- **The per-workspace notebook role** — now SP4e, and it needs a correction first: the F2 design says notebook authz is keyed on Keycloak groups with Ranger deferred, and **that is not what runs**. Ranger is live, Trino connects with `X-Trino-User` keyed on the bare username, and workspace membership compiles to enumerated per-user grants. Building a notebook role on the group-keyed premise would build on a model that does not exist.
- Reconciling Cedar's object-level forbid with the data plane in the **other** direction — making a data-plane deny visible to Cedar. Out of scope; clearance flows one way here.
