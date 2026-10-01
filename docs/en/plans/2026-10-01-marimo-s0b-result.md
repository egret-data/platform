# Data Apps S0b — start-up latency result

**Run:** 2026-09-30 22:06 to 2026-10-01 00:38 (+07), across two calendar days, in one sitting. marimo 0.25.0,
Playwright 1.52 / HeadlessChrome 136, Python 3.12.11, macOS dev laptop (x86_64). Branch `spike/marimo-s0`
@ `14cc33f1` (never merged). Every artefact path below is relative to `spikes/marimo-s0/` on that branch.
**Spec:** [`docs/en/specs/2026-09-30-marimo-data-apps-design.md`](../specs/2026-09-30-marimo-data-apps-design.md) D7, §6.3, §9
**Plan:** [`2026-09-30-marimo-data-apps-s0b-startup.md`](2026-09-30-marimo-data-apps-s0b-startup.md)
**Previous result:** [`2026-09-30-marimo-s0-result.md`](2026-09-30-marimo-s0-result.md)

**How things were checked.** Every timing and identity result comes from an automated Playwright script
(`probes/s0b_breakdown.py`, `probes/s0b_startup.py`, `probes/s0b_fork_safety.py`, `probes/s0b_reload.py`,
`probes/q1_token.py`) that drives HeadlessChrome against the spike's fake egret, its apps-origin proxy and
a marimo run server. Nothing was checked by hand in a browser. Process counts, `rss` and thread counts
come from `ps` snapshots saved under `results/`. The kill-on-disconnect behaviour comes from pytest
(`tests/test_kernel_pool.py`, `tests/test_kernel_forkserver.py`). Each of those tests was shown to fail
with the guarded thread disabled.

**Conditions — read before any number.** Each variant ran **once** per round, on a dev laptop that other
sessions were loading heavily (rustc builds and a VM). The 1-minute load average was ~4.5 for the T1
breakdown, 11.5 to 25.8 in round 3, and 5.4 to 22.4 in round 4 (`results/s0b-r4-uptime.txt`), even
though round 4 waited 14 minutes for the load to fall below 6. Compare numbers **within one round only**.
Differences under about 1 s between variants are within the noise of a single run. None of this is a pod
or Linux figure.

**Target** (from the plan): a new viewer on a warm pod renders in < 2 s at sequential p50, and three
concurrent new viewers render in < 3 s. **No variant met either target on this laptop, and neither did
thread mode:** round 4 thread gave 2.12 s sequential p50 and 11.97 s concurrent max.

## Where the start-up time goes

### S0's 13.2 s was not reproduced

S0 measured 13.2 s to first render for the first IPC session. S0b's T1 breakdown measured 4.82 s for
the same case (`results/s0b-breakdown-ipc.json`, session b0, load ~4.5). A rerun with an empty `.pyc`
cache gave 7.35 s (`results/s0b-breakdown-ipc-cold.json`, load 5.1 to 6.7), so a cold bytecode cache
explains ~2.5 s of the difference, not all of it. **Why S0 saw 13.2 s is unmeasured.** Thread mode's
first session was comparable: 4.44 s here against 5.24 s in S0.

### Per hop, IPC ("direct") against thread (T1: 3 sequential sessions per mode, load ~4.5, single run)

Seconds are counted from navigation start. Each row is one session. Sources:
`results/s0b-breakdown-thread.json` and `results/s0b-breakdown-ipc.json`.

| session | mode | ticket redeemed | ws_open | kernel_spawn_begin | kernel_ready | first gateway request | total (page shows the viewer) |
|---|---|---|---|---|---|---|---|
| b0 | thread | 1.10 | 3.35 | n/a (no spawn in thread mode) | n/a | 3.99 | 4.44 |
| b1 | thread | 0.12 | 1.78 | n/a | n/a | 1.83 | 2.00 |
| b2 | thread | 0.12 | 1.74 | n/a | n/a | 1.79 | 1.98 |
| b0 | ipc | 0.30 | 2.04 | 2.12 | 3.88 | 4.34 | 4.82 |
| b1 | ipc | 0.11 | 1.98 | 1.99 | 3.79 | 4.10 | 4.25 |
| b2 | ipc | 0.12 | 2.02 | 2.07 | 3.76 | 4.07 | 4.27 |

- **The page-load floor is ≈ 1.75 s.** In every mode, navigation → `ws_open` takes 1.74 to 2.02 s in the
  warm sessions (b1, b2; b0 was 2.04 s in ipc and 3.35 s in thread). This is the browser loading egret's host page and marimo's frontend. The spike
  did not split it further. No launcher touches it. Warm thread mode renders at 1.98 to 2.00 s, so the
  plan's "< 2 s" target lies within ~0.25 s of this floor.
- **The IPC delta.** In the warm sessions, IPC minus thread is ~2.3 s: `kernel_spawn_begin` →
  `kernel_ready` takes 1.69 to 1.80 s (wrapped `IPCKernelManagerImpl.start_kernel`,
  `marimo/_session/managers/ipc.py:311`), `kernel_ready` → first gateway request takes a further 0.25 to
  0.45 s (the first cells' imports plus the IPC round trip), and ~0.15 s is not attributed to any mark.
- **What is inside `kernel_spawn_begin` → `kernel_ready`.** Task 3 found that before **every** launch from
  a configured, non-writable venv, marimo runs `<venv>/bin/python -c "import marimo, msgspec, zmq;
  print(marimo.__version__)"` (`has_marimo_installed`, `marimo/_session/_venv.py:128`, called from
  `ipc.py:375`). That is a fresh interpreter importing marimo. In the round-3 pool run, spawn → worker
  claimed took **1.0 to 1.8 s** per launch, mostly this check (the span also includes the shim's start).
  The kernel child then imports `marimo._ipc.launch_kernel`, ~0.7 s (next table), before it prints
  `KERNEL_READY`.

### Imports (`python -X importtime`, cumulative, `results/s0b-importtime.txt`)

| # | module | cumulative (ms) |
|---|---|---|
| 1 | `marimo._ipc.launch_kernel` (almost all of it is `marimo` itself: 721 ms) | 725 |
| 2 | `marimo._ai.llm` (pulled in by `import marimo`) | 561 |
| 3 | `marimo._plugins.ui` (via `_ai.llm._impl` → chat) | 357 |
| 4 | `anywidget` | 236 |
| 5 | `ipywidgets` (inside anywidget) | 204 |
| 6 | `marimo._plugins.ui._impl.altair_chart` | 202 |
| 7 | `marimo._plugins.ui._core.ui_element` | 166 |
| 8 | `marimo._ai._pydantic_ai_utils` | 163 |
| 9 | `polars` (tied with `marimo._messaging.msgspec_encoder`, 121 ms) | 121 |
| 10 | `marimo._plugins.core.web_component` | 112 |

These figures are nested, so they overlap and do not add up. A bare `uv run python -c pass` takes 0.09 s
(median of 5, same file). Interpreter start-up is cheap. **Importing marimo is the cost, and marimo's
per-launch check pays it a second time.**

## Launchers compared

Both new launchers sit behind a shim at the venv interpreter path that marimo is configured to use
(`shim/bin/python`). The shim hands the kernel launch to a pre-started process that runs marimo's own
`marimo._ipc.launch_kernel.main()`.

- **Pool (variant A).** `kernel_pool.py` holds size 3 warm workers plus one "armed" worker, 4 in all
  (4 `worker_warm` marks before the first viewer). Each worker has already imported marimo, polars, httpx
  and `egret_marimo`, and serves exactly one kernel.
- **Fork server (variant B).** `kernel_forkserver.py --preload full` keeps a single-threaded parent that
  has imported the same modules and forks one child per kernel.
- **`-skipcheck` variants.** These add the opt-in `S0_SHIM_SKIP_CHECK`: the shim answers marimo's exact
  `has_marimo_installed` command with the version string and does not start an interpreter.

### Round 4, end to end (all six variants back-to-back 00:32–00:37, single run each, load 5.4 → 22.4)

Timer: from opening a fresh browser context until the page shows `PY pid=… viewer=<user>`. The first
viewer is followed by 5 sequential and then 3 concurrent viewers, 9 sessions per variant. Source:
`results/s0b-r4-<variant>.json`, with load taken before and after each variant from
`results/s0b-r4-uptime.txt`.

| variant | load before → after | first (s) | seq p50 (s) | seq p50 − thread (s) | 3 concurrent max (s) | distinct pids / sessions | identity ok | idle cost (MB rss) |
|---|---|---|---|---|---|---|---|---|
| thread (marimo default, not D7) | 5.66 → 12.05 | 3.46 | 2.12 | 0 | 11.97 | 1 / 9 | 9/9 `every_viewer_ok` | 0 (no per-viewer process) |
| direct (S0 patch) | 14.29 → 10.16 | 7.47 | 6.09 | +3.97 | 10.89 | 9 / 9 | 9/9; `results/q1-ipc.json` (S0) | 0 (no pre-started process) |
| pool, size 3 | 10.15 → 7.98 | 4.48 | 3.54 | +1.42 | 7.95 | 9 / 9 | 9/9; `results/q1-pool.json` | 4 workers × 64.5–70.3 + pool 9.2–13.2 (`results/s0b-pool-rss.txt`) |
| pool-skipcheck, size 3 | 7.28 → 5.79 | 2.79 | 2.43 | +0.31 | 10.74 | 9 / 9 | 9/9 `every_viewer_ok` | same as pool (`results/s0b-pool-rss.txt`, snapshot B) |
| forkserver, preload full | 5.41 → 8.12 | 5.90 | 3.03 | +0.91 | 9.84 | 9 / 9 | 9/9; `results/q1-forkserver.json` | parent 48.5 (`results/s0b-forkserver-rss.txt`, Step 3 run) |
| forkserver-skipcheck, full | 7.87 → 22.42 | 4.98 | 2.95 | +0.83 | 12.91 | 9 / 9 | 9/9 `every_viewer_ok` | parent 48.5 (same file; not re-measured for this variant) |

- "`every_viewer_ok`" means that each page showed the user it was opened as. That name came back
  through the fake egret gateway from the page's own kernel (`probes/s0b_startup.py`). Only the `q1-*`
  files also check the result after a reactive re-run (slider).
- **Targets:** none met. Sequential p50 < 2 s: no variant, and thread itself was at 2.12 s. Three
  concurrent < 3 s: no variant, and thread was at 11.97 s.
- **The concurrent column ranks nothing.** It tracks host load: thread mode, which has no kernel to
  spawn, took the longest but one. The uncommitted local phase logs show why. For the three concurrent
  viewers, ticket → `ws_open` alone took 5.72 to 11.76 s in every variant, including 10.31 to 10.62 s in
  thread mode (`.work/logs/timing-r4-<variant>.jsonl`, not committed, present on the spike worktree).
  With three browser contexts on this laptop, the page load dominates, not the kernel launch.
- **Round 3** (four variants, 23:48–23:54, load 11.5 to 25.8, `results/s0b-{thread,direct,pool,pool-skipcheck}.json`)
  gave the same order: seq p50 thread 1.96, pool-skipcheck 2.95, pool 4.13, direct 7.48 s. Its direct
  sessions drifted from 4.61 to 9.33 s as load rose. The fork server was built after round 3, so it is not
  in it.
- **What holds across rounds 3 and 4:** direct is clearly the slowest process-per-viewer launcher, and
  the pool cut its sequential-p50 excess over thread by more than half in both rounds (round 4: +3.97 →
  +1.42 s; round 3: +5.52 → +2.17 s), and so did the fork server in round 4, its only round (+3.97 →
  +0.91 s). The ordering inside
  {pool, pool-skipcheck, forkserver, forkserver-skipcheck} is **not** separated by a single run at this
  load.

### Round 4, kernel-side phases (uncommitted local logs `.work/logs/timing-r4-<variant>.jsonl`)

| variant | `kernel_spawn_begin` → `kernel_ready`, all 9 sessions (s) | ticket → `ws_open`, 5 sequential (s) | ticket → first gateway response, 5 sequential (s) |
|---|---|---|---|
| thread | n/a (no spawn) | 1.53–2.45 | 1.59–2.52 |
| direct | 1.97–3.70 | 1.57–2.76 | 3.93–6.84 |
| pool | 1.03–1.27 | 1.64–1.80 | 2.89–3.21 |
| pool-skipcheck | 0.11–0.23, one at 1.48 | 1.48–1.66 | 1.81–1.95 |
| forkserver | 0.90–2.99 (first fork 2.99, slowest; unexplained) | 1.46–1.66 | 2.63–2.95 |
| forkserver-skipcheck | 0.16–0.61 | 1.60–2.56 | 2.13–3.08 |

The concurrent sessions' `spawn_begin`/`kernel_ready` pairs are matched by order, because the marks carry
the server's pid and not the viewer's. Claimed → first cell took 0.08 to 0.24 s for both pre-started
launchers.

- **The per-launch check shows up in the phase marks, for both launchers.** Skipping it moved pool
  spawn → ready from 1.03–1.27 s to 0.11–0.23 s, and fork server from 0.90–2.99 s to 0.16–0.61 s.
  **End to end, the fork server did not show it:** seq p50 went from 3.03 to 2.95 s, a difference within
  single-run noise. The pool's end-to-end seq p50 did move, from 3.54 to 2.43 s.
- **Fork server and pool are not equal in these marks.** Without the skip, fork spawn → ready spanned
  0.90–2.99 s against the pool's 1.03–1.27 s, and the first fork was the slowest, for a reason not
  measured. With the skip, the pool's 0.11–0.23 s is tighter than the fork server's 0.16–0.61 s. One
  run at this load cannot say whether either difference is real.
- In pool-skipcheck, ticket → first gateway response for the sequential viewers (1.81–1.95 s) is close
  to thread's (1.59–2.52 s). This is one run: the observation is that the remaining gap to thread lies
  mostly outside the kernel launch, not that it is closed.

## Fork safety (`results/s0b-fork-safety.json`, automated Playwright, one viewer per preload)

| preload | polars imported in parent | parent OS threads before / after fork | rendered (s) | polars path and SQL cell correct | fork warning lines |
|---|---|---|---|---|---|
| full | yes (`polars_loaded=True`) | 1 / 1 | 3.27 | yes (`PY pid=48035 viewer=fsfull`) | 0 |
| light (without polars and `egret_marimo`) | no | 1 / 1 | 3.75 | yes (`PY pid=48096 viewer=fslight`) | 0 |

`import polars` does not start polars' thread pool: the process still has 1 thread afterwards. So `full`
is safe **only while nothing in the parent uses polars before forking**. That holds because of how
`kernel_forkserver.py` is written today, and nothing enforces it.

Two gaps were not tested:

- A code-review note from Task 4, not measured: an `os.fork` failure (EAGAIN) in the fork-server loop is
  not caught and would take the fork server down.
- One run with one viewer per preload does not prove fork safety under concurrency or for other native
  libraries.

## Reload reuse (`results/s0b-reload.json`, automated Playwright, fork server, one run)

**No reload reuses a kernel.**

| step | kernel pid | seconds | what egret saw |
|---|---|---|---|
| open | 50514 | 7.64 | ticket, 2 queries as alice |
| host-page reload (new ticket) | 50535 (new) | 3.82 (an earlier run: 1.82) | ticket, old token revoked, 2 queries |
| iframe-only reload (same `apps_sid` cookie, no new ticket) | 50550 started, never rendered | 60.41 (timed out) | token revoked, 2 × `query_rejected` |

- The iframe-reload dead end is **the spike proxy's doing**. The old WebSocket's close revoked the session
  token, but the proxy's `SESSIONS` entry still maps `apps_sid` to it, so the new kernel's queries were
  rejected and the page stayed blank.
- The host reload starting a new pid is observed, but the spike does not show **why**. The spike did not
  establish whether the proxy (new ticket), marimo, or both cause it, so this result does not attribute
  it to the proxy.
- Consequence: marimo run's `--session-ttl` (default 120 s) keeps a closed session's kernel alive, but
  through this proxy no reload reached it.

## Isolation

It held in every variant measured:

- **Distinct pids.** Every process-per-viewer variant gave 9 distinct pids for 9 sessions, in round 3 and
  in round 4.
- **Single use.**
  - Pool: 9 `worker_claimed` pids, each claimed once, equal to the page pids; none alive 150 s after
    close.
  - Fork server: 11 `forked` marks = 11 distinct claimed pids, none alive ~2.5 min after close, and the
    fork server's own pid never served a query (`results/s0b-forkserver-claims.json`).
- **Kill on disconnect.** A SIGKILLed shim client takes its kernel down within 5 s, for both pool and fork
  server (pytest). With the guard thread disabled, the same tests fail.
- **Identity.** `results/q1-pool.json` and `results/q1-forkserver.json` show alice and bob on different
  pids, with the header correct on every read including slider re-runs, and no rejected queries.
- **What pre-starting gives up.** Environment variables that a process reads only at interpreter start
  (`PYTHONPATH`, `PYTHONHASHSEED`, and anything marimo reads at import) belong to the pool or fork
  server, not to each launch. The working directory is passed per launch and matched (asserted with
  `lsof -d cwd`). In the spike the pool/fork server and the marimo server share their environment, so
  both are equal.

## Findings

1. **Page and frontend load (≈ 1.75 s warm) is a floor that no launcher changes.** A < 2 s target leaves
   ~0.25 s for everything else, on a laptop.
2. **marimo's `has_marimo_installed` check is the single biggest removable cost.** It runs before every
   IPC kernel launch; spawn → claimed was 1.0 to 1.8 s in the round-3 pool marks, mostly this check
   (the span includes the shim start). It was measured in the pool and fork-server phase marks; by the code
   (`ipc.py:375`) it also runs on the direct path, where the spike did not time it separately.
   - It runs because egret points `_get_venv_config` at a configured, non-writable interpreter (S0 patch).
   - Options for P3/P5:
     - (a) The shim short-circuit measured here. It is a hack against private behaviour (the exact
       command string), and it needs a contract test that fails when marimo changes the command.
     - (b) An upstream change, for example caching the result per interpreter and version.
     - (c) Doing the check once, at image build or pod start, and having the entrypoint's patch return
       that result. This is another private patch, with its own contract test.
   - None of the three was chosen here.
3. **A pre-started kernel (pool or fork server) removes the child boot** (claimed → `kernel_ready`
   0.04–0.07 s in the round-3 pool marks) and cuts direct's sequential-p50 excess over thread by more
   than half: the pool in rounds 3 and 4, the fork server in round 4 (its only round).
   - The evidence **cannot choose between pool and fork server.** In round 4 their sequential-p50 gap
     was ~0.5 s and flipped sign with the check skipped (3.54 vs 3.03 s; skipped, 2.43 vs 2.95 s); the
     first viewer was faster on the pool both times (4.48 vs 5.90 s; skipped, 2.79 vs 4.98 s), and the
     first fork was the slowest spawn in the fork-server log (2.99 s), for an unmeasured reason. That is
     one run per variant at load 5.4 to 22.4: a lead for P5 to test, not a result.
   - They differ on things that are not timing:

     | | pool | fork server |
     |---|---|---|
     | idle memory | (size+1) workers × 64.5–70.3 MB rss, pool 9.2–13.2 MB (`results/s0b-pool-rss.txt`) | parent 48.5 MB; children 35–50 MB rss, which on macOS counts shared copy-on-write pages, so unique memory per child is unmeasured (`results/s0b-forkserver-rss.txt`) |
     | fork-safety condition | none: workers are fresh interpreters (`kernel_pool.py`) | parent must never *use* polars; 1 OS thread before/after fork, 2 viewers (`results/s0b-fork-safety.json`) |
     | socket hand-off | fd-passing lost 1/5, 1/5, 1/5, 4/12 viewers, replaced; 0 lost in 46 after (README "S0b T3 (pool hand-off)") | none needed: child inherits the accepted socket by fork (`kernel_forkserver.py`) |
     | shutdown | hung > 20 s when kernels were killed first, open (README "S0b T3 OPEN QUESTION") | exited in 2–3 s with kill-after-exit order; kernels-killed-first untested (README "S0b T4 shutdown") |

4. **The pool's first hand-off design lost sessions** on macOS under marimo. The design passed the
   accepted socket to a worker with `send_fds`, and the worker's `recv_fds` returned 0 bytes and 0 fds:
   1 of 5, 1 of 5, 1 of 5 and 4 of 12 viewers were lost. The cause is not proven. The design was replaced
   by workers that inherit the listener, with one armed at a time. It then had 0 failures in 46 marimo
   sessions. The price is that the pool holds size+1 warm workers. The fd-passing design is untested on
   Linux.
5. **Open question: pool-mode shutdown hangs.** When a pool-mode server's kernels are killed before it,
   `entrypoint.py` stays in `zmq::signaler_t::wait` for over 20 s. Direct mode does not hang. The marimo
   side was not investigated, and the harness works around it by stopping the server first.
6. **Isolation held in every variant** (above).
7. **The launcher depends on one public name and two private patches.** The launch entry that both
   pre-started launchers run, `marimo._ipc.launch_kernel`, is declared public in its docstring: "The
   module path "marimo._ipc.launch_kernel" is a public API used by external consumers (e.g.,
   marimo-lsp). Changing this path is a BREAKING CHANGE" (`marimo/_ipc/launch_kernel.py`, `main()`
   docstring).
   - That docstring covers the **module path**. It does not explicitly cover the stdin `KernelArgs`
     payload or the `KERNEL_READY` / `KERNEL_INFO <pid> <executable>` lines. The `KERNEL_INFO` line is
     documented only in a code comment, as meant for "a launcher such as uv" sitting between the manager
     and the kernel, and `ipc.py:198` parses it tolerantly.
   - Unlike the S0 patches, then, the kernel end of the launcher rests on a declared public name. The
     server end still needs both private patches: `SessionImpl.create` and `_get_venv_config`.
8. **Reload never reuses a kernel.** An iframe-only reload through the spike proxy leads to a blank page
   (above). P4 must re-ticket or reject it clearly.

## What P5 must still measure (on a quiet Linux node on kind, not this laptop)

- All variants **interleaved**, ≥ 5 runs each: seq p50, first, and concurrent max. Report per-phase marks
  (ticket → `ws_open`, spawn → ready) alongside the totals, so that page load and kernel launch stay
  apart.
- Pool against fork server with the `has_marimo_installed` fix that P3/P5 adopts, not the spike's shim
  hack.
- Unique memory per idle worker and per forked child: PSS or USS, not rss.
- Whether pool-mode shutdown hangs on Linux, and whether fork-server mode hangs when its kernels are
  killed first.
- The fd-passing hand-off on Linux, only if P5 wants that design back.
- Why the first fork is slow, if the fork server is chosen.
- Page and frontend load time from the pod through the real proxy. The floor measured here is a laptop
  figure.

## What the spec must now say

- **D7 / §6.3:**
  - Process per viewer stays.
  - A pre-started launcher (pool or fork server) behind the configured-venv interpreter is the P5
    direction. The choice between the two is left to P5's Linux measurement.
  - The launcher's kernel end rests on the public `marimo._ipc.launch_kernel` path. The two private S0
    patches remain.
  - The `has_marimo_installed` per-launch check is a named P3/P5 decision.
- **§6.3 cap and cold-start text:**
  - Idle cost is ~64–70 MB rss per warm pool worker, with size+1 held; the fork-server parent is 48.5 MB.
  - An active kernel is ~81 MB.
  - Page load (≈ 1.75 s) is a floor.
  - A reload is always a new kernel.
  - The S0 13.2 s figure is replaced with what S0b measured.
- **§9:** an S0b row, done.
