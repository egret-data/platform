# P5e live result — run mode on kind

Task 5e-1 of `docs/en/plans/2026-10-03-data-apps-p5-run-mode.md`: the structural proof that D24
holds in production, not just in the fake-pod proxy tests. Run against the `egret-verify` release
on the shared `kind-egret` cluster (the same pattern P4e used), rebuilt from this branch's code.

**Spec:** `docs/en/specs/2026-09-30-marimo-data-apps-design.md` §10/§11 (updated by this result),
D24 (§1.2).

## What the spec asked for vs. what this test does

The plan's five sub-claims, and the one live spec (`frontend/tests/e2e/apps-run-kind.spec.ts`)
that covers them:

1. An admin publishes an app; two different logged-in viewers each open it via the Apps list's
   **Open** — done exactly as written.
2. Each page shows its own identity; the two sessions never show the same pid — done, but NOT with
   pid (see "A defect in the test itself" below). Each page renders its pod's own hostname instead.
3. Closing one viewer's tab and waiting past `run_idle_secs` makes that pod disappear — done, with
   a twist: rather than a fixed sleep, the test keeps the *other* viewer actively reloading and
   polls until the pod count drops by exactly one, which also proves the complementary half of the
   claim (an **active** session's pod is *not* reaped) that a bare timer wait would not have shown.
4. A third viewer of the same app does not affect either of the first two — done.
5. Pod names, counts: read from the live `egret-verify-apps` namespace via `kubectl`, shelled out
   from the spec the same way other kind specs shell raw sockets or `curl`.

## Result: PASS, on the 4th attempt — 3 real defects found and fixed along the way

Every attempt before the last failed for a *reason this plan exists to surface*, not for a reason
to shrug off. Each is recorded here in full, because the finding is as much the deliverable as the
green run.

### Defect 1 — a Kubernetes run pod was bound to the EDIT port, not the run port

`crates/egret-bin/src/apps_pod_k8s.rs`'s `marimo_container()`/`endpoint()` (built in P5b, Task
5b-2) used one hardcoded `MARIMO_PORT = 2718` constant for both `build_pod` (edit) and
`build_run_pod` (run). `egret_marimo.launcher`'s own `PORTS` map says run listens on **2719**, not
2718. Every run pod's container port, readiness probe, and the URL the proxy dialed were all
wrong for run pods — the first live attempt showed the exact symptom: `kubectl get events` recorded
`Readiness probe failed: dial tcp <ip>:2718: connect: connection refused`, and the pod was killed
by egret's own `ensure_once_run` timeout before ever being reachable.

Fixed: separate `EDIT_MARIMO_PORT`/`RUN_MARIMO_PORT` constants, threaded through
`marimo_container`, `build_pod`/`build_run_pod`, and `endpoint()`. Pinned with a new unit test
(`the_run_pod_carries_its_app_and_version_as_plain_env_and_its_tokens_only_by_reference`'s
extension) asserting the run pod's container port and readiness-probe port are both 2719 —
break-it verified (reverting to the shared constant made the new assertion fail).

None of P5b's own unit tests caught this because they each built one pod kind and checked only
that kind's own fields in isolation; nothing compared the two kinds' ports against each other or
exercised a real cluster. This is exactly the class of defect P4e and now P5e exist to catch.

### Defect 2 — the apps NetworkPolicy's ingress rule only opened the edit port

With defect 1 fixed, the run pod's readiness probe passed (kubelet-sourced, same-node) but every
real request from egret's proxy to the pod **timed out** (not refused — a silent drop, the
fingerprint of a policy DROP rather than a missing listener). Direct reproduction: manually
redeeming a ticket and GETting the pod's `/r/{ref}/` path through the real proxy returned a 502;
`kubectl -n egret-verify-apps exec` into a disposable debug pod and curling the run pod's IP
directly on 2719 confirmed a connection timeout, while curling 2718 against an edit pod from the
same vantage point worked. `deploy/helm/egret/templates/networkpolicy-apps.yaml`'s ingress rule
allowed only `{ protocol: TCP, port: 2718 }` from egret's own pods — never updated for run pods in
P5b.

This also falsifies this file's own comment, written during P4d/P4e, that "kind's default CNI
(kindnet) does NOT enforce NetworkPolicy." On this cluster, at this kindnet version, it demonstrably
does — at least for the ingress port list (the readiness probe passing is consistent with kubelet
probes being sourced in a way this particular policy shape does not catch, not with policy being
inert overall). The comment is corrected in place rather than just patched around.

Fixed: added `{ protocol: TCP, port: 2719 }` to the same ingress rule. Pinned with a new test in
`crates/egret-server/tests/deployment_probes.rs`
(`the_apps_network_policy_opens_both_the_edit_and_the_run_port`) — break-it verified.

### Defect 3 — the live spec's own identity check was invalid

After fixing both of the above, the pods existed, were reachable, and the app rendered — but the
test still failed: both viewers' pages showed `pid=11`. This is not a third production bug: a
Kubernetes Pod gets its own PID namespace by default (no `hostPID`, no `shareProcessNamespace` —
both correctly absent, since sharing either would be a real isolation hole), so "the first process
in the container" is a small number alike in every pod, by design. `os.getpid()` can never
distinguish two pods from inside them; it was the wrong oracle from the start, not a flaky one.

Confirmed via `kubectl get pods -l egret.io/kind=run` at the moment of the failure: two distinct
pod names existed (`app-run-6178687723d60e75`, `app-run-d04f194fa115584d`) even though both
printed `pid=11`. D24's pod separation was already correct; the test's assertion was not.

Fixed: the published app now prints `socket.gethostname()` instead — Kubernetes sets a pod's
hostname to the pod's own name, so this *is* a real cross-pod fingerprint. No SQL/engine dependency
either way (deliberate: D24's claim is about pod separation, not query permissions, which is
already covered live by `apps-edit-kind.spec.ts`'s "real pod API" test).

## Final run: PASS

```
Running 1 test using 1 worker
[1/1] … two viewers get two pods; an abandoned one is reaped; a third viewer does not disturb an active one
  1 passed (1.5m)
```

Verified live, in order, in one test: two viewers of one published app land in two different pods
(by hostname and by `kubectl`-observed pod count); closing one viewer's context while the other
keeps reloading reaps exactly the abandoned pod's; the still-active viewer's pod and its rendered
identity are unchanged afterward; a third viewer gets a new pod of their own without touching the
second viewer's.

## Honest gaps

- The idle-reap step proves "abandoned is reaped, active is not" — it does not independently prove
  the exact `run_idle_secs` *timing* (the poll loop's 5 s granularity and the test's own
  reload-driven activity make a tight timing assertion impossible to do honestly; the plan already
  anticipated this and allowed accepting a looser check over a sleep-and-hope one).
- This run used `apps.pod.launcher=kubernetes` with the production NetworkPolicy template; it was
  NOT run with `apps.pod.networkPolicy.enabled=false` to confirm the policy is actually the thing
  kindnet is enforcing here (as opposed to some other coincidental cause) — the direct in-cluster
  curl reproduction (connection timeout on 2719 only, from a pod not matching the policy's "from"
  selector; success once the port was added to the one real caller's rule) is strong but not a
  controlled A/B. Worth a follow-up only if another kindnet-based environment disagrees.
- `egret-verify`'s debug app (`debug-pid-app` / `e2e-run-*`) and the manually-created local users
  from earlier failed attempts were not all individually cleaned up on the live cluster; they are
  disposable verification-release artifacts, not production data.
