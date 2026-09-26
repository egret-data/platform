# D7 result — the principal form, and whether a group grant reaches an engine

**Measured:** 2026-09-24, on the kind cluster (`kind-egret`), helm revision 61.
**Engine:** `demo-trino`, Trino coordinator `demo-trino-coordinator-c54f46974-xwgbt`.
**Why this exists:** the programme spec (`2026-09-15-control-plane-dg-review-programme.md:44`)
*inferred* — and marked as unverified — that "no group provider or usersync exists, so group rules
may never match inside engines". SP4a's D7 refuses to build on an inference, because the answer
decides how the implicit workspace grant compiles (Task 6).

---

## 1. The principal an engine actually sees: a bare username

```sql
SELECT current_user AS who
```

run through egret's query API as `admin@egret.local`, against `demo-trino`:

```json
{ "columns": [{ "name": "who", "type": "varchar" }],
  "rows": [["admin"]] }
```

**`admin`, not `admin@egret.local`.** Corroborated by the engine's own error message from an earlier
run, before policies were reachable: `Access Denied: Principal admin cannot become user admin`.

**Consequence.** A policy whose principal is written as an email matches nobody. egret writes both
forms today — the auto-seeded admin policies use the bare username
(`egret-policy/tests/default_policies.rs:45` seeds with `"admin"`, and the live
`egret-admin - trinouser` policy on this cluster carries `"users": ["admin"]`), while the policy
picker writes emails. **SP4a's canonical form is the bare username**, and the compiler emits that and
nothing else.

## 2. Groups do not reach the engine — by mechanism, not by symptom

Three independent pieces of evidence, none of which is a failed query:

1. **Trino has no group provider.** `/etc/trino/` on the running coordinator contains
   `access-control.properties`, `catalog/`, `config.properties`, `jvm.config`, `log.properties`,
   `node.properties`, `ranger-trino-security.xml`, `tls` — and **no `group-provider.properties`**.
   Trino's default is no group provider, so `getGroups(user)` is empty for every user.
2. **egret never writes one.** `grep -rn "group-provider|group_provider|GroupProvider"` across
   `crates/egret-provisioner/src/` and `deploy/` returns only a line in
   `deploy/trino/etc/README.md` describing the file as something an operator could add. Nothing in
   the provisioning path creates it.
3. **egret has no usersync.** `grep -rn "usersync|xusers|/service/xusers"` across
   `crates/egret-policy/src/` returns nothing. egret serves policies and tags to the plugin; it
   never pushes user or group membership.

Ranger's plugin asks the host engine for the requesting user's groups. Trino answers "none". So a
policy item naming a group matches no one, on any engine egret provisions today.

**This is stronger evidence than the query experiment the plan called for.** A denied query has many
possible causes — a wrong policy, a stale poll, an unrelated deny — and would have needed the
user-policy/group-policy control pair to be conclusive. The absent group provider is the mechanism
itself, and it cannot produce a false negative.

## 3. What Task 6 must therefore do

Per D7's second half: `workspace-members` **cannot** compile to a Ranger group. It compiles to **the
member list enumerated at compile time**, and adding or removing a workspace member must recompile
the profile — the same staleness shape as D5's StarRocks tag expansion, with the same answer, since
egret owns the compile.

And per D1's rule that a control which silently does nothing is worse than an absent one: SP4b must
not offer "group" as a principal kind while this holds. If it does, it must say, where the group is
chosen, that group grants are not enforced inside the engine.

## 4. A larger finding, recorded here because this measurement surfaced it

**The cluster was enforcing nothing.** `EGRET_RANGER__ENABLED=false`, so egret never mounted the
`/service` router, and the Trino plugin received `404 — service not found` on every poll with
`lastKnownVersion=-1`. `demo-trino` had **never downloaded a policy** and was denying every query.

Not introduced by this work: helm revision 58 — before any deploy in this programme — carried the
same five values, and the chart defaults `ranger.enabled` to `false`
(`deploy/helm/egret/values.yaml:97-98`). Enabled with the user's approval at revision 61; the plugin
then logged `Switched policy engine to [29]` and the query above succeeded.

**A trap worth fixing separately:** `helm upgrade --wait` reported "successfully rolled out" while
the pods stayed eight hours old, because the ranger setting reaches the container through a ConfigMap
and the chart carries no checksum annotation to force a restart. `kubectl rollout restart` was
required. Verify the pod's environment, not helm's own report. This is the second instance of the
shape this session: SP3c-2's init-job hook migrates with the *new* image while the Deployment keeps
serving the *old* one.
