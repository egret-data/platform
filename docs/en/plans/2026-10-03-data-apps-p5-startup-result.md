# Data Apps P5 — start-up result (Task 5a-1)

2026-10-03. Spike-only measurement (not committed code): `apps-image/egret_marimo/launcher.py`'s
`install_run_mode_ipc_patch(shim)` call was commented out by hand for this run and restored
immediately after (`git diff` shows no change to that file). Reproduced by anyone with the P3 image
checked out the same way, until Task 5c-1 lands the real `run_kernel_mode` toggle.

## 1. The isolation property (trustworthy regardless of load)

`apps-image/tests/e2e/test_thread_mode_isolation.py` — **passed**. Two browser tabs of ONE viewer,
against one `marimo run` process in its default thread mode (no forced-IPC patch): both sessions'
queries carried that one viewer's token and nothing else, and both sessions' rendered `pid=` matched
`server.proc.pid` exactly — confirming thread mode was really in effect (an IPC-patched run would show
a different, non-server pid per session, as the existing `test_run_mode_one_kernel_process_and_token_per_viewer`
test already pins for the two-different-viewers case). **This is the property D24's consequence rests
on, and it held.** It does not depend on wall-clock conditions, unlike the number below.

## 2. Cold-start (NOT trustworthy as measured — re-run on a quiet machine before trusting it)

**Conditions at the time of the run:** `uptime` reported **load average 116.70 / 63.35 / 30.37** on an
8-core machine — another process on the same machine was compiling a large Rust workspace
(`rustc` processes for `egret-workspace`, `egret-perm-compile`, and others visible in `ps`) at the
same time, alongside a running virtualization process (a kind cluster VM). This is far outside even
S0b's worst recorded condition (load 25.8, also flagged there as noise). **The numbers below are an
upper bound under severe contention, not a measurement of the design.** Methodology (`/tmp/p5_coldstart.py`,
not committed — a disposable spike script, same shape as `apps-image/tests/e2e/harness.py`'s `launch()`):
start `egret-apps run` fresh (no pre-warming — none is possible, D24), wait for its own `/health`,
then time a fresh Chromium page's navigation to first render, 5 runs back to back, same process
never reused between runs.

| | pod ready (process start → health check passes) | first render after navigation |
|---|---|---|
| median | 12.65 s | 7.76 s |
| min | 10.95 s | 6.51 s |
| max | 17.11 s | 9.04 s |

**What this does and does not say.** The isolation property above is unaffected by load and is the
thing this task needed to prove before anything else in the plan builds on it. The timing number is
not re-measured here under quiet conditions because none were available in this session; re-run
`/tmp/p5_coldstart.py`'s approach (or fold it into the e2e suite properly) on an idle machine, or on
kind in Part P5e where the live result already measures pod scheduling realistically, before using
any number from this section to judge whether pre-warming is worth building. **Recommendation to the
user:** do not treat 12.65 s as the real cold-start cost; treat "a cold run pod takes noticeably longer
than a cold edit pod's ~4 s (measured in the P4e live result under normal load)" as the only safe
takeaway — `marimo run` additionally fetches the version source over HTTP before it can start serving,
which an edit pod does not need to prove responsive (its health check can answer sooner).

## What this means for D15

D15's targets ("≤ 0.5 s slower than same-round thread mode at the median") were written to compare a
pre-started kernel pool against marimo's own thread mode, inside one SHARED pod. D24 removed the
shared pod, and with it the pool — there is nothing to pre-warm for a viewer who has not been
identified yet, since the pod's identity IS the viewer. The targets as written do not describe this
architecture and are not re-stated as a pass/fail gate for P5. The real, still-open question is
whether a first-time viewer's cold-pod wait (whatever it measures as, on a quiet machine or on kind)
is acceptable UX for v1, or whether a narrow, bounded form of pre-warming — e.g. one spare pod kept
warm for an app's most recently published version, refreshed on each publish — is worth a follow-up.
That follow-up is not built in this plan; it is a decision for the user once a trustworthy number
exists (Part P5e's live kind run is the next chance to get one under realistic, if not quiet,
conditions).
