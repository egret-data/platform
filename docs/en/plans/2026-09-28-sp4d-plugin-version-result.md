# SP4d-1 Task 1 result: which Ranger actually enforces

Date: 2026-09-28
Cluster: `kind-egret`, namespace `egret-engines`.

This plan's premise was already corrected twice by measurement before this
task started: an earlier survey read the Ranger plugin version as 2.8.0 from
`trinodb/trino:latest` (Trino 483), which egret does not provision. The
existing test harness (`tests/ranger/pom.xml`) separately pins 2.4.0. Neither
number was re-verified against what egret actually runs. This doc records
that verification.

## Step 1: Trino Ranger plugin version

Two Trino coordinators were running in `egret-engines`:
`demo-trino-coordinator-c54f46974-xwgbt` (must not be disturbed — holds 26
endpoints/alerts/datasets used elsewhere) and
`sp3c2-trino-live-coordinator-855f5cc7bb-lzpg9`. Both run the same image, so
all read/write-sensitive exec was done against `sp3c2-trino-live` instead of
`demo-trino`, and only read-only `exec` was used against `demo-trino` (config
`cat`, no restart/modify).

Command:

```bash
command kubectl --context kind-egret get pod -n egret-engines sp3c2-trino-live-coordinator-855f5cc7bb-lzpg9 -o jsonpath='{.spec.containers[*].image}'
```

Output:

```text
trinodb/trino:470
```

Command:

```bash
command kubectl --context kind-egret get pod -n egret-engines demo-trino-coordinator-c54f46974-xwgbt -o jsonpath='{.spec.containers[*].image}'
```

Output:

```text
trinodb/trino:470
```

Both coordinators run `trinodb/trino:470`. Plugin directory listing (against
`sp3c2-trino-live-coordinator-855f5cc7bb-lzpg9`, the pod has no `find`, so
`ls` via `sh -c` was used):

```bash
command kubectl --context kind-egret exec -n egret-engines sp3c2-trino-live-coordinator-855f5cc7bb-lzpg9 -- \
  sh -c 'ls /usr/lib/trino/plugin/apache-ranger/'
```

Output (elided to the Ranger-relevant entries; full listing captured during
the session, ~150 jars total, all other jars are unrelated Trino/plugin
dependencies):

```text
org.apache.ranger_ranger-plugin-classloader-2.5.0.jar
org.apache.ranger_ranger-plugins-audit-2.5.0.jar
org.apache.ranger_ranger-plugins-common-2.5.0.jar
org.apache.ranger_ranger-plugins-cred-2.5.0.jar
io.trino_trino-ranger-470.jar
trino-ranger-470-services.jar
```

Exit code of the `exec`: `0`.

**Trino's Ranger plugin version is confirmed as 2.5.0.** This matches the
brief's prior measurement (also on `trinodb/trino:470`) and refutes the
2.8.0 figure read earlier from `trinodb/trino:latest` (Trino 483), which
egret does not provision.

## Step 2: StarRocks Ranger plugin version

Disk check before considering any image pull:

```bash
df -h /
```

Output:

```text
Filesystem        Size    Used   Avail Capacity iused ifree %iused  Mounted on
/dev/disk1s5s1   466Gi    12Gi    39Gi    24%    459k  413M    0%   /
```

39Gi available. `docker images` showed `starrocks/allin1-ubuntu:latest`
(5.22GB) already local, but **not** `starrocks/fe-ubuntu:4.1-latest`, which is
the image egret actually provisions
(`crates/egret-engine-mgmt/src/engines.rs:59`, `DEFAULT_SR_FE_IMAGE`). Rather
than pull that image (several GB, avoidable), two live pods already running
that exact image were found and used instead — no pull needed, no risk to
disk:

```bash
command kubectl --context kind-egret get pod -n egret-engines sp3c2-sr-live-fe-0 -o jsonpath='{.spec.containers[*].image}'
```

Output: `starrocks/fe-ubuntu:4.1-latest`

```bash
command kubectl --context kind-egret get pod -n egret-engines femeta-live-fe-0 -o jsonpath='{.spec.containers[*].image}'
```

Output: `starrocks/fe-ubuntu:4.1-latest`

Read-only exec into the running FE container (both pods; the container is
named `fe`, `kubectl exec` prints "Defaulted container \"fe\" out of: fe,
build-keystore (init)" as a notice on stderr, folded into stdout below):

```bash
command kubectl --context kind-egret exec -n egret-engines sp3c2-sr-live-fe-0 -- \
  sh -c 'ls /opt/starrocks/fe/lib/ 2>/dev/null | grep -i ranger || ls /data/deploy/starrocks/fe/lib/ 2>/dev/null | grep -i ranger'
```

Output:

```text
Defaulted container "fe" out of: fe, build-keystore (init)
ranger-audit-core-2.8.0.jar
ranger-authz-api-2.8.0.jar
ranger-plugin-classloader-2.8.0.jar
ranger-plugins-common-2.8.0.jar
ranger-plugins-cred-2.8.0.jar
```

Exit code: `0`.

Repeated against `femeta-live-fe-0` for corroboration — identical output
(same five jars, all `2.8.0`), exit code `0`.

**StarRocks's Ranger plugin version is confirmed as 2.8.0** (not inferred
from `allin1-ubuntu`, which was never queried for this). This differs from
Trino's 2.5.0 — the two engines bundle different Ranger plugin versions.

## Step 3: super-users short-circuit

`ranger.plugin.super.users`, if set, makes Ranger grant access unconditionally
for listed principals before any policy is evaluated — egret's evaluator
cannot see or reproduce this from policy data alone, so its absence/presence
matters for what the evaluator can promise.

```bash
command kubectl --context kind-egret exec -n egret-engines sp3c2-trino-live-coordinator-855f5cc7bb-lzpg9 -- \
  sh -c 'cat /etc/trino/ranger-trino-security.xml'
```

Output:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<configuration>
<property><name>ranger.plugin.trino.service.name</name><value>sp3c2-trino-live</value></property>
<property><name>ranger.plugin.trino.policy.rest.url</name><value>http://egret.egret-system.svc.cluster.local:8080</value></property>
<property><name>ranger.plugin.trino.policy.source.impl</name><value>org.apache.ranger.admin.client.RangerAdminRESTClient</value></property>
<property><name>ranger.plugin.trino.policy.cache.dir</name><value>/tmp/ranger/sp3c2-trino-live/policycache</value></property>
<property><name>ranger.plugin.trino.policy.pollIntervalMs</name><value>30000</value></property>
<property><name>ranger.plugin.trino.policy.rest.client.connection.timeoutMs</name><value>120000</value></property>
<property><name>ranger.plugin.trino.policy.rest.client.read.timeoutMs</name><value>30000</value></property>
</configuration>
```

Exit code: `0`. `ranger.plugin.super.users` (and `ranger.plugin.super.groups`)
is **not present** in this file.

Repeated read-only (no write/restart) against `demo-trino-coordinator` for
corroboration:

```bash
command kubectl --context kind-egret exec -n egret-engines demo-trino-coordinator-c54f46974-xwgbt -- \
  sh -c 'cat /etc/trino/ranger-trino-security.xml'
```

Output: identical structure, only `ranger.plugin.trino.service.name` (=
`demo-trino`) and the two `demo-trino`-named path values differ.
`ranger.plugin.super.users` is likewise **not present**. Exit code: `0`.

**Confirmed: neither provisioned Trino instance sets
`ranger.plugin.super.users` (or `...super.groups`).** No super-user
short-circuit is configured on the measured instances. This is a property of
egret's own Ranger config templating (both instances share it), not
something that can vary per-deployment without a code change — but it was
only checked on these two live instances, not read from the templating
source itself, so treat "no other deployment sets it" as unconfirmed beyond
what was measured here.

## Offline availability of the target version

```bash
ls ~/.m2/repository/org/apache/ranger/ranger-plugins-common/
```

Output:

```text
2.4.0/
2.5.0/
2.7.0/
2.8.0/
```

**2.5.0 is present in the local Maven repository.** Task 2 can pin against it
without a network fetch.

## Existing harness pin (for contrast)

```bash
grep -n "ranger-plugins-common" tests/ranger/pom.xml
```

Output:

```text
<dependency><groupId>org.apache.ranger</groupId><artifactId>ranger-plugins-common</artifactId><version>2.4.0</version></dependency>
```

The existing harness pins **2.4.0**, which matches neither engine actually
provisioned by egret.

## Conclusion — what Task 2 pins to, and why

**Task 2 must pin the harness's `ranger-plugins-common` dependency to
`2.5.0`, replacing the current 2.4.0 pin in `tests/ranger/pom.xml`.**

Reasoning: SP4d-1's evaluator exists to reproduce the access decision of the
Ranger plugin that egret's provisioned Trino coordinator actually loads, and
Trino is the primary/only engine this package's later tasks target per the
brief (StarRocks was measured here for completeness and to close off a
second possible guess, not because the evaluator is scoped to it in this
task). The provisioned Trino image is `trinodb/trino:470` (confirmed
identical on both `demo-trino` and `sp3c2-trino-live` coordinators), and it
loads `ranger-plugins-common-2.5.0.jar`, verified directly from the plugin
directory on a live pod — not inferred from any image tag, sibling image, or
changelog. 2.5.0 is also already present in `~/.m2`, so Task 2 needs no
network fetch. The prior 2.8.0 figure (from `trinodb/trino:latest`/Trino 483)
and the harness's existing 2.4.0 pin are both refuted by this measurement and
must not be used.

StarRocks, measured separately, bundles `ranger-plugins-common-2.8.0` — a
different version from Trino's. If the evaluator package is later extended to
cover StarRocks enforcement, that extension must pin 2.8.0 for StarRocks
specifically, not reuse the Trino-derived 2.5.0 pin. That is out of scope for
this task and is recorded here only so the discrepancy isn't rediscovered
from scratch.

No `ranger.plugin.super.users` short-circuit is configured on either measured
Trino instance, so the evaluator's scope (policy-only decisions, no
super-user bypass) matches what is actually enforced on the instances
measured.

## Summary table

| Measurement | Value | Status |
|---|---|---|
| Trino image (both coordinators) | `trinodb/trino:470` | Measured |
| Trino Ranger plugin (`ranger-plugins-common`) | `2.5.0` | Measured |
| StarRocks image (both FE pods) | `starrocks/fe-ubuntu:4.1-latest` | Measured |
| StarRocks Ranger plugin (`ranger-plugins-common`) | `2.8.0` | Measured |
| `ranger.plugin.super.users` set (Trino, both instances) | Not set | Measured |
| 2.5.0 available in `~/.m2` | Yes | Measured |
| Existing harness pin (`tests/ranger/pom.xml`) | `2.4.0` (stale) | Measured |
| StarRocks `allin1-ubuntu` image version | — | NOT MEASURED (not the provisioned image; irrelevant, not queried) |
