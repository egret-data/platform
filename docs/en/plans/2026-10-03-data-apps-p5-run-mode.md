# Data Apps P5 — Run Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A viewer opens a published app from the Apps list and sees it run under their own permissions, in a pod that holds no other viewer's kernel. An author sees and restores past versions.

**Architecture:** One marimo **run** pod per (viewer, published version) — not per version. `marimo run`'s own thread-mode kernel is used as-is inside that pod (no forced-IPC patch), because with exactly one viewer's identity in the pod, different sessions sharing a thread no longer mixes different people's data. The proxy, launcher trait and both launchers (subprocess, Kubernetes) gain a `Run` counterpart to their existing `Edit` methods, keyed by `(user, version)` instead of `(user, workspace)`.

**Tech Stack:** Rust (axum, kube, reqwest), Python (`apps-image/egret_marimo`), React/TypeScript, Helm, kind, Playwright.

**Spec:** `docs/en/specs/2026-09-30-marimo-data-apps-design.md` §5.2, §5.3, §6.3, §9 (P5), D7, D15, D16. Plan: `docs/en/plans/2026-10-02-data-apps-p4-origin-proxy-edit.md` (P4a–P4e, merged — #617–#621). P4e live result: `docs/en/plans/2026-10-03-data-apps-p4e-live-result.md`.

## D24 — decided with the user, 2026-10-03, before this plan was written

**Finding.** marimo's IPC kernel channels (`marimo/_ipc/connection.py`) are ZeroMQ PUSH/PULL sockets bound to `tcp://127.0.0.1:<random port>`, unauthenticated and unencrypted. The spec's run-mode design (one pod per **version**, many viewers' kernels inside it via the forced-IPC patch, D7) would let a published app's own cell code — running as one viewer's kernel — scan localhost and read another viewer's control channel (their `x-egret-app-token` is visible in request headers the kernel reads) or inject forged output into their session. This is a cross-viewer leak inside a shared pod, worse than D3's "the author is untrusted with respect to the viewer": it would let the author reach a *different* viewer's session.

**Decided:** **one run pod per (app version, viewer)**, not per version. This is the fallback D7 already named ("one pod per (app × viewer)") for when the forced-IPC patch breaks — adopted here as the default instead, because it is the only one of the three options considered (accept the risk; this one; patch marimo to Unix-domain sockets) that is both safe by construction and needs no further private-API patching. Measured and spiked in Task 5a-1 before anything else in this plan commits to it.

**Consequence, worked out below and confirmed in Task 5a-1:** with exactly one viewer per pod, different browser tabs of that *same* person sharing marimo's default thread-mode kernel is no longer a cross-person isolation problem, so **the forced-IPC patch built in P3 is not used by default** for run pods. It is kept in `apps-image/egret_marimo/patches.py`, wired behind a kill switch (`apps.pod.run_kernel_mode`), undocumented as a supported path, for a future revival if a Unix-socket fix is ever built. D15's numeric start-up targets were written against a shared warm pool; they no longer describe this architecture (there is nothing to pre-warm for a person who has not been identified yet) — Task 5a-1 measures the real number (a cold pod for a first-time viewer) and the spec is updated with it rather than a target nothing can meet.

**Cost accepted:** one full pod per concurrent viewer of a published app, not one pod shared by many. Noted in §6.3 and in the resource-budget task.

## Global Constraints

- A run pod serves exactly one `(version_id, user_id)` pair for its whole life; a new published version never reuses an old pod, and a pod opened before a publish keeps serving the version it was opened for until its last session closes and it idles out.
- A viewer's run pod holds no credential to an engine; all queries go through the gateway under that viewer's identity (unchanged from P1).
- `include_code=False` in run mode (spec §6.3): a viewer never sees cell source.
- Rust: `cargo fmt`, clippy clean on touched files (main still carries pre-existing clippy debt from a toolchain change; do not add to it, do not try to fix it in this plan). Migrations, if any, edited in place only before merge.
- Commits are Conventional Commits, no `Closes #` in commit bodies (PR descriptions only).
- Every security- or isolation-relevant test is shown to fail when the code it guards is broken on purpose (same discipline as P4).
- `cargo test --workspace` is not run locally in this environment (disk headroom); rely on per-crate runs plus CI.

## File structure

New:
- `crates/egret-apps/src/proxy/pod_ref.rs` — the two pod-reference functions (edit, run), moved out of `mod.rs` so both are defined once, next to each other, with one set of tests.
- `crates/egret-apps/tests/run_launcher.rs` — subprocess run-launcher tests with a real process.
- `apps-image/tests/test_thread_mode_isolation.py` — the measurement behind D24's consequence: two sessions in one thread-mode pod, each session's query carries only its own `x-egret-app-token`.
- `frontend/src/routes/explore/AppViewPage.tsx`, `frontend/src/components/apps/VersionsDialog.tsx`.
- `frontend/src/routes/explore/__tests__/AppViewPage.test.tsx`, `frontend/src/components/apps/__tests__/VersionsDialog.test.tsx`.
- `docs/en/plans/2026-10-0X-data-apps-p5-startup-result.md` — the cold-start measurement from Task 5a-1, filled in by that task.
- `docs/en/plans/2026-10-0X-data-apps-p5-live-result.md` — the live two-viewer kind run, filled in by Part P5e.

Modified: `crates/egret-apps/src/proxy/{mod,relay}.rs`, `crates/egret-apps/src/launcher.rs`, `crates/egret-apps/src/subprocess_pod.rs`, `crates/egret-apps/tests/proxy.rs` (pod-ref format changes), `crates/egret-bin/src/{main,apps_pod_k8s}.rs`, `crates/egret-config/src/lib.rs`, `apps-image/egret_marimo/launcher.py`, `deploy/helm/egret/{values.yaml,templates/*}`, `frontend/src/routes/explore/{Apps,AppEditPage}.tsx`, `frontend/src/App.tsx`, `docs/en/specs/2026-09-30-marimo-data-apps-design.md` (+ vi).

---

# Part P5a — pod identity, proxy plumbing, config (backend only, fakes)

Branch `feat/data-apps-p5a-run-identity` off `main`. Worktree `egret-wt-apps-p5a` (or reuse `egret-wt-apps-p4a`, switching branch — it already holds the built `egret-apps:p4e` image and a warm `target/`).

### Task 5a-1: Measure the thread-mode consequence of D24 before building on it

This is the spike S0/S0b style measurement that D24 promised. It must show two things, or the rest of this plan is built on a guess.

**Files:**
- Create: `apps-image/tests/test_thread_mode_isolation.py`
- Create (result): `docs/en/plans/2026-10-0X-data-apps-p5-startup-result.md` (date it on the day you run it)

- [ ] **Step 1: Write the isolation test.** Two concurrent marimo sessions inside one `marimo run` server (thread mode, no IPC patch), simulating two tabs of the **same** viewer: each session's cell calls `egret_marimo._session_token()` (already exported for tests as `EGRET_TEST_TOKEN`/the real header path) and asserts it reads back the token that request's `X-Egret-App-Token` carried, never the other session's. Use the pattern already in `apps-image/tests/e2e/test_edit_and_run.py` (two `viewer_page` contexts, `app_proxy.make_app`) but seed **one** viewer identity for both contexts (same `TOKENS` entry), and assert via the fixture's `egret.queries` log that every query recorded the one token, matching the kernel pid the session landed in (confirms thread-mode still tags requests correctly even without separate processes).

```python
def test_two_tabs_of_one_viewer_never_cross_tokens_in_thread_mode(egret, launch, proxy, browser):
    """D24: with one pod per viewer, thread mode is safe — both tabs are the SAME person."""
    egret.session_tokens["tok-solo"] = {"user": "solo"}
    egret.versions[(APP, VERSION)] = {"source": (APPS / "viewer.py").read_text(), "version_no": 1}
    server = launch("run", f"/r/{VERSION}", EGRET_APP_ID=APP, EGRET_VERSION_ID=VERSION)
    base = proxy(server.port, {"solo": "tok-solo"})
    with viewer_page(browser, base, "solo") as a, viewer_page(browser, base, "solo") as b:
        for page in (a, b):
            page.goto(f"{base}/r/{VERSION}/")
            _wait_text(page, PID_LINE)
    queries = list(egret.queries)
    assert queries, "no queries recorded"
    assert {q["token"] for q in queries} == {"tok-solo"}
```

- [ ] **Step 2:** Run it against the **current** `install_run_mode_ipc_patch` path first (to confirm the harness works) — `pytest -q -m e2e apps-image/tests/e2e/test_edit_and_run.py -k two_tabs` — expect it to fail to collect until Task 5c-1 adds the `run_kernel_mode` toggle; for now run it manually with the patch commented out of `build()` to prove thread mode alone passes. Record the result.
- [ ] **Step 3: Measure cold-start.** Using `apps-image/tests/e2e/harness.py`'s `launch()` against a **freshly started** `egret-apps run` process (no pre-warming possible — there is no pool), time navigation → first render for one viewer, five interleaved runs, on the same machine used for S0b (note the load average, same discipline as S0b: "compare numbers within one run only"). This is the number that replaces D15's pool-vs-thread comparison, because there is no pool to compare against anymore.
- [ ] **Step 4: Write the result doc.** State plainly: the measured cold-start number, that it is a **pod** cold start (scheduling + container start + marimo start), not a kernel-launch number, and that D15's "≤ 0.5 s slower than thread mode" framing no longer applies because this design *is* thread mode with no warm alternative to compare against. Recommend to the user, in the doc, whether the number is acceptable as-is for v1 or whether pre-warming a small number of pods for an app's **most recently published** version (a bounded, cheap idea — not one pod per possible future viewer) is worth a follow-up. Do not build that follow-up in this plan; it is out of scope unless the user asks after reading the number.
- [ ] **Step 5:** Update the spec's D15 row (§1.1) to point at this result and state the architecture change, mirroring how D20/D21 were updated after P4e's measurement.

### Task 5a-2: `pod_ref` for run pods, and the trait extension

**Files:**
- Create: `crates/egret-apps/src/proxy/pod_ref.rs`
- Modify: `crates/egret-apps/src/proxy/mod.rs`, `crates/egret-apps/src/launcher.rs`
- Test: `crates/egret-apps/src/proxy/pod_ref.rs` (inline `#[cfg(test)]`), extend `crates/egret-apps/src/launcher.rs` tests

**Interfaces:**
- Produces:

```rust
// proxy/pod_ref.rs
pub fn edit_pod_ref(user: UserId, ws: WorkspaceId) -> String;            // unchanged behaviour, moved
pub fn run_pod_ref(user: UserId, version: Uuid) -> String;               // new, same shape (16 hex chars)

// launcher.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RunPodKey { pub user_id: UserId, pub version_id: Uuid }
impl RunPodKey { pub fn pod_ref(&self) -> String; }

#[async_trait]
pub trait AppPodLauncher: Send + Sync {
    async fn ensure_edit(&self, key: EditPodKey, timeout: Duration) -> Result<PodEndpoint>;
    async fn stop_edit(&self, key: EditPodKey) -> Result<()>;
    async fn idle_edit_pods(&self, idle: Duration) -> Result<Vec<EditPodKey>>;
    /// The pod serving one viewer's own copy of one published version.
    async fn ensure_run(&self, key: RunPodKey, timeout: Duration) -> Result<PodEndpoint>;
    async fn stop_run(&self, key: RunPodKey) -> Result<()>;
    async fn idle_run_pods(&self, idle: Duration) -> Result<Vec<RunPodKey>>;
    fn mode(&self) -> &'static str;
}
```

`pod_ref()` on `EditPodKey` moves to call `edit_pod_ref` (behaviour and output unchanged — existing tests in `launcher.rs` and `proxy.rs` must keep passing without editing their expected values).

- [ ] **Step 1: Write the failing tests.** In `pod_ref.rs`: `run_pod_ref` is stable per `(user, version)`, distinct for a different user or a different version, 16 lowercase hex chars, and — the property that matters — **distinct from `edit_pod_ref` of the same user against any workspace/version id that happens to share bytes** (mix the hash input so edit and run refs can never collide for the same raw UUID pair; e.g. a one-byte domain separator prefixed into the hash). In `launcher.rs`: `FakeAppPodLauncher` needs `ensure_run`/`stop_run`/`idle_run_pods` with the same reuse/stop/idle semantics as the edit methods, tested the same way as `one_pod_per_key_and_a_second_request_reuses_it` but for `RunPodKey`.

```rust
#[test]
fn run_and_edit_refs_never_collide_for_the_same_raw_uuid_pair() {
    let u = UserId(Uuid::new_v4());
    let shared = Uuid::new_v4(); // used as BOTH a workspace id and a version id
    assert_ne!(edit_pod_ref(u, WorkspaceId(shared)), run_pod_ref(u, shared));
}
```

- [ ] **Step 2:** `cargo test -p egret-apps pod_ref` — FAIL (module does not exist).
- [ ] **Step 3: Implement.** `run_pod_ref` hashes a domain tag byte (`b'R'`) + user + version; `edit_pod_ref` is the existing function with an `b'E'` tag added (this **changes edit pod names** — acceptable because nothing has shipped to production yet and P4e's verification release is disposable; note it in the PR). Extend `FakeAppPodLauncher`'s internal `FakeState` with a second `live`/`started`/`stopped` tracking pair for `RunPodKey`, or key both maps by an enum `{Edit(EditPodKey), Run(RunPodKey)}` if that reads cleaner — implementer's choice, tests pin behaviour not internal shape.
- [ ] **Step 4:** Run — PASS. Run the **existing** `launcher.rs` and `proxy.rs` test suites now — the edit-ref output changed (domain tag added), so any test asserting a literal hash value (none currently do; they all call `pod_ref`/`edit_ref` and compare to itself) should still pass unchanged. Confirm with `cargo test -p egret-apps launcher:: proxy::`.
- [ ] **Step 5: Break it.** Remove the domain tag from one of the two functions; the collision test must fail. Restore.
- [ ] **Step 6:** Commit `feat(apps): run_pod_ref and the AppPodLauncher run methods`.

### Task 5a-3: `PodResolver::run_pod` takes the viewer, and `LauncherPods` wires it

**Files:**
- Modify: `crates/egret-apps/src/proxy/mod.rs`

**Interfaces:**
- Produces (breaking change to an existing trait, all four call sites below updated in this task):

```rust
#[async_trait]
pub trait PodResolver: Send + Sync {
    fn edit_ref(&self, user: UserId, ws: WorkspaceId) -> String;
    async fn edit_pod(&self, user: UserId, ws: WorkspaceId) -> Result<PodEndpoint>;
    /// The URL segment of a viewer's own run pod for one version.
    fn run_ref(&self, user: UserId, version: Uuid) -> String;
    /// That viewer's own pod, serving that version, started if need be.
    async fn run_pod(&self, user: UserId, app: Uuid, version: Uuid) -> Result<PodEndpoint>;
}
```

- [ ] **Step 1: Update in lockstep** (one commit, since the trait is the seam): `UnavailablePods::{run_ref, run_pod}` (ref via `run_pod_ref`, pod via `Err(NotFound)` as today); `LauncherPods<L>::run_ref` (→ `run_pod_ref`) and `::run_pod` (→ `self.launcher.ensure_run(RunPodKey { user_id: user, version_id: version }, self.ready_timeout)`, `app` unused — keep the parameter for symmetry with the pod-API's `(app, version)` pair and because `admit()` in relay.rs already has `app_id` in scope there); the two test doubles in `crates/egret-apps/tests/proxy.rs` (`FakePods`, `SwitchPods`) gain matching methods.
- [ ] **Step 2:** `cargo check -p egret-apps --tests` — the compiler finds every call site that needs the new parameter; fix each (this is the mechanical half of the task — let the type system drive it, do not hunt by grep).
- [ ] **Step 3:** No new test here (the trait's own behaviour has no logic yet beyond delegation); the existing `launcher.rs` test `the_resolver_starts_the_callers_own_pod_and_names_it_with_the_shared_ref` is extended with a run-mode counterpart:

```rust
#[tokio::test]
async fn the_resolver_also_starts_the_viewers_own_run_pod_by_version() {
    let (u, v) = (UserId(Uuid::new_v4()), Uuid::new_v4());
    let pods = LauncherPods { launcher: fake(), ready_timeout: Duration::from_secs(3) };
    assert_eq!(pods.run_ref(u, v), RunPodKey { user_id: u, version_id: v }.pod_ref());
    let ep = pods.run_pod(u, Uuid::new_v4(), v).await.unwrap();
    assert_eq!(ep.base.as_str(), "http://10.0.0.1:2718/");
}
```

- [ ] **Step 4:** PASS. Commit `feat(apps): PodResolver run_pod takes the viewer; LauncherPods wires it to AppPodLauncher::ensure_run`.

### Task 5a-4: The proxy's run-mode admission is per viewer, not per version alone

This is the task that closes D24's actual hole in the merged P4c code: today `admit()` computes the run surface's "entitled" pod reference from `grant.version_id` alone, so every viewer of one version is entitled to the same `{ref}` — exactly the shared-pod design D24 rejected. After this task, two different viewers' grants for the same version are entitled to two different `{ref}`s.

**Files:**
- Modify: `crates/egret-apps/src/proxy/relay.rs`
- Test: `crates/egret-apps/tests/proxy.rs`

- [ ] **Step 1: Write the failing tests**, extending the existing `proxy.rs` fixture (it already has `editor`/`viewer` `Person`s, a `version`, and `FakePods`):

```rust
#[tokio::test]
async fn two_viewers_of_the_same_version_get_two_different_run_pod_references() {
    let fx = Fixture::new().await;
    let other = fx.new_person("second_viewer", "ws_viewer").await; // add this fixture helper
    let (c1, _) = fx.cookie_for(&fx.viewer, AppMode::Run).await;
    let (c2, _) = fx.cookie_for(&other, AppMode::Run).await;
    let ticket1 = fx.ticket_for(&fx.viewer, AppMode::Run).await;
    let r1 = fx.get(&format!("/_ticket?t={ticket1}"), None).await;
    let ticket2 = fx.ticket_for(&other, AppMode::Run).await;
    let r2 = fx.get(&format!("/_ticket?t={ticket2}"), None).await;
    let loc1 = r1.header("location")[0].clone();
    let loc2 = r2.header("location")[0].clone();
    assert_ne!(loc1, loc2, "two viewers redirected to the same run pod reference");
    // and each viewer's grant is refused on the OTHER's pod reference
    let ref1 = loc1.split('/').nth(2).unwrap().to_string();
    assert_eq!(fx.get(&format!("/r/{ref1}/api/version"), Some(&c2)).await.status, 401);
    assert_eq!(fx.get(&format!("/r/{ref1}/api/version"), Some(&c1)).await.status, 200);
    let _ = c1;
}
```

Also extend `the_pod_gets_exactly_our_token_...` style coverage: a run grant's cookie on the OTHER viewer's run pod path must 403 (wrong pod, same surface) — add this as a second assertion in the test above (`403`, not `401`, once past the cookie check but failing the "not your app" pod-reference match — adjust the expected status to whatever `admit()` actually returns for "right surface, wrong ref owned by someone else", which is `StatusCode::FORBIDDEN` per the existing code path).

- [ ] **Step 2:** `cargo test -p egret-apps --test proxy two_viewers_of_the_same_version` — FAIL (both viewers currently land on the same `{ref}`, so `loc1 == loc2` and the pod-ownership check does not distinguish them).
- [ ] **Step 3: Implement.** In `relay.rs`:
  - `redeem()`'s `AppMode::Run` arm: replace the bare `grant.version_id` scope/location with `st.inner.pods.run_ref(grant.user_id, v)` (mirrors the `Edit` arm's `edit_ref` call), building `/r/{r}/` for both scope and location (a run pod has no `?file=` equivalent — the pod serves exactly one file, so there is nothing to redirect to beyond the root).
  - `admit()`'s step 5 `entitled` match: `Surface::Run => grant.version_id.map(|v| st.inner.pods.run_ref(grant.user_id, v)).unwrap_or_default()`.
  - `handle()`'s pod-fetch match: `Surface::Run => match admitted.grant.version_id { Some(v) => st.inner.pods.run_pod(admitted.grant.user_id, admitted.grant.app_id, v).await, None => Err(...) }`.
  - `PodResolver` is accessed through `st.inner.pods: Arc<dyn PodResolver>` already — no new field needed.
- [ ] **Step 4:** PASS. Run the **full** `proxy.rs` suite (`cargo test -p egret-apps --test proxy -- --include-ignored` with `EGRET_APPS_BIN` set, per the P4c PR) to catch every place the old shared-ref assumption was baked into an existing assertion (e.g. `another_users_pod_reference_is_refused` already tests edit; check whether any run-mode assertion elsewhere assumed the bare version string — fix forward, do not weaken).
- [ ] **Step 5: Break it.** Revert `admit()`'s `entitled` computation to the bare version string; the new test must fail (both viewers again entitled to the same ref). Restore.
- [ ] **Step 6:** Commit `fix(apps): a run grant is entitled to its OWN viewer's pod, not every viewer's (D24)`.

### Task 5a-5: `apps.pod` configuration for run pods

**Files:**
- Modify: `crates/egret-config/src/lib.rs`

**Interfaces:**
- Produces (added to `AppPodConfig`):

```rust
/// A run pod nobody has used for this long is stopped. Separate from the edit default
/// (30 min) because an abandoned viewer tab is cheaper to let go of than an author's work.
pub run_idle_secs: u64,           // default 300
/// Sessions (browser tabs) of the ONE viewer a run pod serves, open at once, before a new
/// one gets 503 "app busy". Not a multi-tenant cap (D24: one pod is one person) — it bounds
/// a reconnect loop, not concurrent strangers.
pub run_session_cap: u32,         // default 8
/// "thread" (default, safe because D24 makes one pod one viewer) or "forced_ipc" (the P3
/// patch, kept for a future shared-pod revival; NOT a supported production path today).
pub run_kernel_mode: String,      // default "thread"
```

- [ ] **Step 1: Write the failing test** in the existing `validate_checks_the_pod_launcher_choice_and_what_each_needs` style: `run_kernel_mode` must be `"thread"` or `"forced_ipc"`, nothing else; `run_session_cap >= 1`; defaults round-trip through `AppsConfig::default()` and the existing `apps_config_defaults` figment test (extend its expected struct literal).
- [ ] **Step 2:** Run — FAIL (fields absent).
- [ ] **Step 3:** Implement: add fields with `Default` values above, extend `validate()`'s `match pod.launcher.as_str()` arm for `"subprocess" | "kubernetes"` with the two new checks.
- [ ] **Step 4:** PASS. Commit `feat(config): apps.pod run-pod settings — idle, session cap, kernel mode`.

### Task 5a-6: PR P5a

- [ ] `cargo fmt --all`, clippy on touched crates, full `egret-apps`/`egret-config` suites plus the ignored real-marimo proxy test. Open the PR against `main`; stop for the user's merge approval.

---

# Part P5b — the run-pod launchers

Branch `feat/data-apps-p5b-run-launchers` off `main` after P5a merges.

### Task 5b-1: Subprocess run launcher (dev)

**Files:**
- Modify: `crates/egret-apps/src/subprocess_pod.rs`
- Test: `crates/egret-apps/tests/run_launcher.rs`

**Interfaces:**
- Produces (added to `SubprocessPodLauncher`'s `impl AppPodLauncher`): `ensure_run`, `stop_run`, `idle_run_pods`, mirroring the edit methods. Internal `Running` tracking is keyed by an enum or two maps (implementer's choice; the `Fixture` in the new test only observes behaviour). `start()` for a run pod sets `EGRET_APP_ID`/`EGRET_VERSION_ID` and uses `/r/{ref}` as `EGRET_BASE_URL` instead of `/e/{ref}`, mirroring the real launcher's two modes (`egret-apps run` instead of `egret-apps edit` as the appended argument).

- [ ] **Step 1: Write the failing tests** in `run_launcher.rs`, copying `subprocess_pod.rs`'s existing test shape against `fake_marimo.py` (it already honours `EGRET_BASE_URL`/`EGRET_MARIMO_PORT`/`EGRET_MARIMO_TOKEN` generically, so it works for "run" without changes — pass `mode="run"` plumbing if the launcher's `start()` hardcodes `"edit"` as the appended arg today; check and parameterise):
  - two different viewers of the same version get two different ports/pids (`ensure_run` with two different `RunPodKey`s);
  - the SAME viewer re-requesting the same version reuses the pod;
  - the same viewer, two different versions, gets two pods;
  - `stop_run` kills the process and revokes nothing extra (run pod tokens are version-scoped via `PodTokenRepo::mint_run`, not per-viewer — confirm `stop_run` does not try to call `revoke_edit_for`; there is no `revoke_run_for` yet — see Step 1a below);
  - idle reporting mirrors `idle_edit_pods`.
- [ ] **Step 1a:** `PodTokenRepo` has no `revoke_run_for` today (P4 only built `revoke_edit_for`). Add it, same shape, scoped by `(ws, app, version)` rather than `(user, ws)` — a run pod's token identifies "may read this version", not "is this viewer", so revoking it on stop is about not leaving a live token for a pod that is gone, not about per-viewer isolation (the version's content is not secret from its own viewers). Write this as a small preceding sub-step with its own failing test in `crates/egret-apps/tests/podtoken_repo.rs`, same pattern as `revoke_edit_for_kills_every_live_edit_token_of_that_user_in_that_workspace_only`.
- [ ] **Step 2:** Run — FAIL for each in turn as written.
- [ ] **Step 3: Implement.** Factor the shared parts of `start()` (process spawn, health-wait, kill-group, log-tail) into a private helper parameterised by mode (`"edit"` or `"run"`) and the extra env (`EGRET_APP_ID`/`EGRET_VERSION_ID` for run); keep `ensure_edit`/`ensure_run` as thin public wrappers so the trait's method names stay the call sites' intent.
- [ ] **Step 4:** PASS.
- [ ] **Step 5: Break it.** Make `ensure_run` ignore `version_id` in the key (always use the first version it sees); the "same viewer, two versions, two pods" test must fail. Restore.
- [ ] **Step 6:** Commit `feat(apps): subprocess run-pod launcher; PodTokenRepo::revoke_run_for`.

### Task 5b-2: Kubernetes run launcher

**Files:**
- Modify: `crates/egret-bin/src/apps_pod_k8s.rs`

**Interfaces:**
- Produces: `pub fn run_pod_name(key: &RunPodKey) -> String` (`"app-run-{ref}"`, mirrors `pod_name`/`"app-edit-{ref}"`); `build_run_pod`/`build_run_secret` (same hardening as edit's `build_pod`/`build_secret`, `args: ["run"]`, env adds `EGRET_APP_ID`, `EGRET_VERSION_ID` — both already in the pod API's version-fetch path, the pod needs them to call `GET /api/apps/pod/apps/{app_id}/versions/{version_id}` itself exactly as `apps-image/egret_marimo/launcher.py` already expects); `ensure_run`/`stop_run`/`idle_run_pods` on `KubernetesAppPodLauncher`, reusing `ensure_once`'s generic retry/exit-code logic (generalise `ensure_once` to take a `PodKind::{Edit, Run}` and dispatch pod name / spec builder / token-mint call, rather than duplicating the loop).

- [ ] **Step 1: Write the failing tests**, mirroring every existing `apps_pod_k8s.rs` unit test for the edit pod builders: `run_pod_name` is DNS-safe and distinct per key; the run pod's `EGRET_APP_ID`/`EGRET_VERSION_ID` are plain env (not secret — they are not sensitive, unlike the two tokens); the run pod carries the same hardening (`run_as_non_root`, `read_only_root_filesystem`, `drop: ["ALL"]`, `restartPolicy: Never`); `key_of`-equivalent annotation round-trip for the reaper (reuse the same `egret.io/user`/`egret.io/last-used` annotations plus a new `egret.io/version`, and `key_of` becomes `key_of(pod) -> Option<EitherKey>` or two functions `edit_key_of`/`run_key_of` filtered by a `egret.io/kind` label — implementer's choice, pin behaviour not shape).
- [ ] **Step 2:** Run — FAIL.
- [ ] **Step 3: Implement.** Add an `egret.io/kind: edit|run` label so `idle_edit_pods`'/`idle_run_pods`'s `list()` calls can each select only their own kind (`labels().selector("app=egret-apps,managed-by=egret,egret.io/kind=run")`), avoiding a run pod ever being reaped by the edit path or vice versa.
- [ ] **Step 4:** PASS.
- [ ] **Step 5: Break it.** Drop the `egret.io/kind` label from the selector in `idle_run_pods`; a test that seeds one edit pod and one run pod and asserts `idle_run_pods` returns only the run one must fail. Restore.
- [ ] **Step 6:** Commit `feat(apps): Kubernetes run-pod launcher, sharing the edit launcher's hardening and retry logic`.

### Task 5b-3: Wire both launchers' run methods into `main.rs`; reap both kinds

**Files:**
- Modify: `crates/egret-apps/src/launcher.rs` (generalise `spawn_reaper`), `crates/egret-bin/src/main.rs`

**Interfaces:**
- Produces: `spawn_reaper` renamed `spawn_pod_reaper`, taking the same `Arc<dyn AppPodLauncher>` and reaping **both** edit and run pods each tick (two idle-list calls, two stop loops, same per-key error handling as today — a failure on one kind must not skip the other). Update its existing test (`the_reaper_stops_idle_pods_and_survives_a_failing_one`) to also seed a `RunPodKey` and assert it is reaped on its own tick independent of the edit key's failure.

- [ ] **Step 1:** Extend the reaper test first (failing: the renamed/widened function does not exist yet).
- [ ] **Step 2:** Run — FAIL.
- [ ] **Step 3:** Implement the widened reaper; update `main.rs`'s single call site (`egret_apps::spawn_reaper(launcher.clone(), ...)` → `spawn_pod_reaper`, with the run idle duration read from `config.apps.pod.run_idle_secs` — the edit and run idle durations differ, so the function takes both: `spawn_pod_reaper(launcher, edit_idle, run_idle, every)`).
- [ ] **Step 4:** PASS. `cargo check -p egret-bin` to confirm the call site compiles.
- [ ] **Step 5:** Commit `feat(apps): one reaper ticks both edit and run pods, on their own idle windows`.

### Task 5b-4: Helm — run-pod resources, idle, session cap

**Files:**
- Modify: `deploy/helm/egret/values.yaml`, `deploy/helm/egret/templates/backend-deployment.yaml`

- [ ] **Step 1:** Add `apps.pod.run: { idleSecs: 300, sessionCap: 8, kernelMode: thread }` to `values.yaml`, documented inline the same way `apps.pod.idleSecs` already is.
- [ ] **Step 2:** Add the three `EGRET_APPS__POD__RUN_*` env entries to the existing `{{- if eq .Values.apps.pod.launcher "kubernetes" }}` block in `backend-deployment.yaml`, next to the edit ones.
- [ ] **Step 3:** `helm template` with and without `apps.pod.launcher=kubernetes`, grep for the three new env names present/absent as expected (same check style used in P4d's PR description). Extend `crates/egret-server/tests/deployment_probes.rs` with one assertion that the three run env names appear in the backend Deployment template when `apps.pod.launcher` is rendered as kubernetes (string-search the template, same technique already used there for the apps Ingress).
- [ ] **Step 4:** Commit `feat(deploy): apps.pod.run — idle, session cap, kernel mode`.

### Task 5b-5: PR P5b

- [ ] fmt, clippy, suites. PR against `main`; stop for approval.

---

# Part P5c — apps-image: run pods default to thread mode, session cap enforced

Branch `feat/data-apps-p5c-thread-mode` off `main` after P5b merges.

### Task 5c-1: `run_kernel_mode` toggle in the launcher

**Files:**
- Modify: `apps-image/egret_marimo/launcher.py`
- Test: `apps-image/tests/test_launcher.py`

- [ ] **Step 1: Write the failing test:** `build("run", secrets)` with `EGRET_RUN_KERNEL_MODE` unset or `"thread"` does **not** call `install_run_mode_ipc_patch` (patch a spy over it, assert not called); with `EGRET_RUN_KERNEL_MODE=forced_ipc` it does. An unknown value is `LaunchError(EX_USAGE, ...)`.
- [ ] **Step 2:** Run — FAIL (the call is unconditional today).
- [ ] **Step 3: Implement.** Read `EGRET_RUN_KERNEL_MODE` (default `"thread"`) in `build()`'s run branch; only call `install_run_mode_ipc_patch(shim)` when it is `"forced_ipc"`. The env var name matches `apps.pod.run_kernel_mode`'s Helm wiring from Task 5b-4 (`EGRET_RUN_KERNEL_MODE`, plain env, not a `pod.toml`-style key — add it as a fourth required-adjacent var, validated like `EGRET_APP_ID`).
- [ ] **Step 4:** PASS.
- [ ] **Step 5: Break it.** Hardcode thread mode (remove the `forced_ipc` branch entirely); the `forced_ipc` test must fail. Restore.
- [ ] **Step 6:** Commit `feat(apps-image): run_kernel_mode toggle — thread by default (D24), forced_ipc kept for a future shared-pod revival`.

### Task 5c-2: The isolation measurement from Task 5a-1 becomes a permanent contract test

**Files:**
- Create: `apps-image/tests/test_thread_mode_isolation.py` (promote the spike from Task 5a-1 into the suite, marked `e2e` like its neighbours)

- [ ] **Step 1:** Move the test written in Task 5a-1 here, running against the **real** default (`run_kernel_mode` unset → thread). It is now exercising production code, not a manual patch-removal.
- [ ] **Step 2:** `pytest -q -m e2e apps-image/tests/test_thread_mode_isolation.py` — PASS (it already passed as a spike; this confirms it still passes wired through the real toggle).
- [ ] **Step 3:** Commit `test(apps-image): pin D24's thread-mode isolation property as a permanent e2e test`.

### Task 5c-3: Session cap

**Files:**
- Modify: `apps-image/egret_marimo/launcher.py` or the proxy (decide by where marimo exposes a session count — check `marimo/_session/session_manager.py` for a way to read live session count before accepting a new `/ws` upgrade)
- Test: `crates/egret-apps/tests/proxy.rs` (if enforced at the proxy) or `apps-image/tests/test_launcher.py` (if enforced in the pod)

This task's exact seam depends on a fact not yet confirmed: **can the proxy count a run pod's live WebSocket sessions the same way `tokens.rs` already counts a grant's open WebSockets (D19's `ws_opened`/`ws_closed`)?** If yes (likely — the proxy already owns the WS lifecycle for every relayed connection), enforce the cap there, reusing the exact counting mechanism already built for token revocation, keyed by `RunPodKey` instead of grant id, and keep it simple: no new marimo-side code.

- [ ] **Step 1: Confirm the seam** by reading `crates/egret-apps/src/proxy/relay.rs`'s `relay_ws`/`WsGuard` and `tokens.rs`'s `ws_opened`/`ws_closed` once more; if a per-pod (not per-grant) counter fits naturally alongside the existing per-grant one, proceed at the proxy. If it does not (e.g. because the proxy does not know which **pod** a WebSocket belongs to independent of the grant — it does, via `admitted.grant` → `run_ref` — so it should fit), document why and fall back to a marimo-side count instead, before writing any test.
- [ ] **Step 2: Write the failing test** in `proxy.rs`: open `run_session_cap` (configured low, e.g. 2, for the test) WebSockets against one `RunPodKey`'s pod through the proxy, then attempt a `(cap + 1)`th; expect the relay to refuse the upgrade with `503` before it ever reaches the (fake) pod. Closing one of the open sessions must free a slot for the next.
- [ ] **Step 3:** Run — FAIL.
- [ ] **Step 4:** Implement a small per-run-pod open-count map alongside `TokenBook` (or inside it, keyed by `RunPodKey` rather than grant id — a `DashMap<RunPodKey, usize>`), checked in `relay_ws` before dialing upstream, incremented/decremented symmetrically with the existing `ws_opened`/`ws_closed` calls (same `WsGuard` drop point).
- [ ] **Step 5:** PASS. **Break it:** skip the check; the over-cap test must fail. Restore.
- [ ] **Step 6:** Commit `feat(apps): run_session_cap — a pod's own viewer is capped at N concurrent tabs, 503 beyond it`.

### Task 5c-4: PR P5c

- [ ] `pytest -q apps-image/tests` (unit) and `pytest -q -m e2e apps-image/tests` (Chromium), `cargo test -p egret-apps`. PR against `main`; stop for approval.

---

# Part P5d — frontend: the viewer page, Open enabled, Versions/restore

Branch `feat/data-apps-p5d-viewer-ui` off `main` after P5c merges.

### Task 5d-1: Viewer page

**Files:**
- Create: `frontend/src/routes/explore/AppViewPage.tsx`
- Create: `frontend/src/routes/explore/__tests__/AppViewPage.test.tsx`
- Modify: `frontend/src/App.tsx` (route `apps/:id` or `apps/:id/run` — use `apps/:id` as the **viewing** route since Edit already owns `apps/:id/edit`, matching the "Explorer" vs editor split pattern elsewhere in the app), `frontend/src/api/apps.ts` (`requestAppTicket` already takes a mode; add a thin `useAppVersions`/`useRestoreVersion` pair reusing the generated `appsVersions`/`appsVersionRestore` the same way `usePublishApp` wraps `appsPublish`)

**Interfaces:**
- Produces in `api/apps.ts`:

```typescript
export function useAppVersions(ws: string, id: string) { /* wraps useAppsVersions, select r.data as AppVersionSummary[] */ }
export function useRestoreVersion(ws: string, id: string) {
  // wraps appsVersionRestore; invalidates useAppVersions' and useApp's query keys
}
```

- [ ] **Step 1: Write the failing tests** for `AppViewPage`, copied and trimmed from `AppEditPage.test.tsx`'s shape (same `requestAppTicket` mock, same StrictMode/stale-ticket-race tests reused verbatim — the ticket-lifecycle logic is identical, only the mode and the chrome differ):
  - requests exactly one **run** ticket (`requestAppTicket(ws, id, "run")`, no `versionId` — "open the latest" is the default the backend already resolves);
  - the iframe has the same sandbox/referrer attributes as the editor's;
  - no Publish/Reload-editor/Versions controls — a viewer only sees the app, full height, with a thin header naming the app and a link back to the Apps list;
  - 403/404/409/503 failures reuse `describeFailure` (export it from `AppEditPage.tsx` or move it to a shared `apps/failures.ts` module both pages import — do the latter, since duplicating the switch would drift);
  - **one more case `AppEditPage` does not have:** a 409 whose body text is specifically "no published version" (spec: `AppsHttpError::NoPublishedVersion`) renders a distinct message from the generic trashed-app 409 ("Nobody has published this app yet" vs "This app can't be opened right now") — `describeFailure` needs the response body, not just the status, to tell them apart; thread the parsed `ApiError.body` through.
- [ ] **Step 2:** Run — FAIL.
- [ ] **Step 3: Implement**, extracting `describeFailure` to `frontend/src/lib/apps/failures.ts`, updating `AppEditPage.tsx` to import it (no behaviour change there — its own tests must keep passing unmodified), and writing `AppViewPage.tsx` as a slimmer sibling of `AppEditPage.tsx` (same ticket-effect-with-ref pattern, no publish/reload chrome, `mode: "run"`).
- [ ] **Step 4:** PASS. Run `AppEditPage.test.tsx` too — unchanged, still green.
- [ ] **Step 5:** Commit `feat(frontend): the app viewer page (run mode)`.

### Task 5d-2: Open is enabled; Edit page gets a Versions panel

**Files:**
- Modify: `frontend/src/routes/explore/Apps.tsx`, `frontend/src/routes/explore/AppEditPage.tsx`
- Create: `frontend/src/components/apps/VersionsDialog.tsx`, `frontend/src/components/apps/__tests__/VersionsDialog.test.tsx`

**Interfaces:**
- `VersionsDialog` props: `{ ws: string; appId: string; open: boolean; onOpenChange: (v: boolean) => void }`; internally `useAppVersions` + `useRestoreVersion`, a list sorted newest-first (the generated `appsVersions` already returns them that way per its doc comment — confirm, do not assume), each row showing version number, published-at, published-by, and a **Restore** button with a confirmation (restoring publishes a NEW version from the old source — the spec is explicit history stays linear — word the confirm dialog that way so nobody expects a pointer move).

- [ ] **Step 1: Write the failing tests.** `Apps.test.tsx`: the `Open` button is a real link to `/explore/:ws/apps/:id` (no longer `aria-disabled`) when `latest_version_no != null`, and stays disabled with the existing tooltip when it is `null` (draft only — nothing to open). `VersionsDialog.test.tsx`: lists versions, Restore calls the mutation with the right version id, shows a toast on success, and the confirm dialog's body contains the word "new" (pinning the "restore creates a new version" framing so a future edit cannot silently soften it to sound like a rollback).
- [ ] **Step 2:** Run — FAIL.
- [ ] **Step 3:** Implement. Add a **Versions** button next to Publish in `AppEditPage.tsx`'s header, opening `VersionsDialog`.
- [ ] **Step 4:** PASS.
- [ ] **Step 5:** Commit `feat(frontend): enable Open for published apps; a Versions panel with restore`.

### Task 5d-3: PR P5d

- [ ] `npx vitest run`, `npx tsc --noEmit`, `npx eslint .`. PR against `main`; stop for approval.

---

# Part P5e — live verification on kind

Branch `feat/data-apps-p5e-live` off `main` after P5d merges. Reuses the `egret-verify` release pattern from P4e (`just deploy-kind-apps`, now exercising `apps.pod.launcher=kubernetes` for BOTH edit and run — already the case since P4d/P4e set `apps.pod.launcher=kubernetes` globally, not edit-only).

### Task 5e-1: Two viewers, one published app, two pods

**Files:**
- Create: `frontend/tests/e2e/apps-run-kind.spec.ts`
- Create (result): `docs/en/plans/2026-10-0X-data-apps-p5-live-result.md`

- [ ] **Step 1:** Write a Playwright spec, reusing `apps-edit-kind.spec.ts`'s shared-login fixture pattern (two separate logins this time — a run grant needs `app:read`, so seed a second local user with `ws_viewer`, not reuse the admin for both viewers, or the "two different pods" assertion could be confused by the admin's own edit-mode cookie path):
  1. an admin publishes an app whose source is `apps-image/tests/e2e/apps/viewer.py`'s pattern (prints `pid={os.getpid()} viewer={egret.sql(...)}`);
  2. two different logged-in viewers each open it via the Apps list's **Open**;
  3. each page shows its own identity in the rendered text, and (read from the live `egret-verify-apps` namespace via `kubectl get pods -o jsonpath`, run from the test through `child_process` the same way `apps-edit-kind.spec.ts`'s raw-socket test shells out, or — cleaner — assert indirectly through the rendered `pid=` values being different) the two sessions never show the same pid;
  4. closing one viewer's tab and waiting past `run_idle_secs` (configure a short one, e.g. 30 s, via the release's `--set apps.pod.run.idleSecs=30` for this run only) makes that pod disappear (`kubectl get pods` from the test, or accept this as a manual-only check noted in the result doc if driving `kubectl` from Playwright proves awkward — do not fake it with a sleep-and-hope assertion that cannot fail).
  5. a THIRD viewer, opening the SAME app, does not affect either of the first two (their pods are untouched) — the structural proof that D24 holds in production, not just in the fake-pod proxy tests.
- [ ] **Step 2:** Run it against `egret-verify` (rebuild images, `just deploy-kind-apps`). Record the result, including the actual pod names seen (`app-run-<ref>` per Task 5b-2), honestly reporting anything that did not work as designed — same discipline as the P4e result doc.
- [ ] **Step 3:** Update the spec's "Done when" (§11) line about viewers seeing their own permissions to note it now covers run mode, with a link to this result.

### Task 5e-2: PR P5e

- [ ] Commit the spec/result updates and the new live spec. PR against `main`; stop for approval.

---

## Self-review against the spec and D24

| Requirement | Task |
|---|---|
| D24: one pod per (viewer, version), not per version | 5a-2, 5a-3, 5a-4 |
| D24 consequence: thread mode safe, forced-IPC kept as an off-by-default fallback | 5a-1 (measured first), 5c-1, 5c-2 |
| §5.2: a run pod serves exactly one version; new version → new pod; old sessions keep the old pod until idle | structural from 5a-2's keying (version is part of the key) + 5b-1/5b-2's idle reaping; no separate "detect republish" logic needed because a NEW published version simply has a different `version_id` and therefore a different `RunPodKey` by construction |
| §6.3: per-pod session cap, memory limit, session-start UX from measurements | 5a-1 (start-up number), 5c-3 (cap), 5b-4 (limits — reusing edit's, documented as shared) |
| D15: start-up targets | 5a-1 rewrites this against the new architecture rather than pretending the old pool-vs-thread framing still applies — flagged to the user in the result doc, not silently reinterpreted |
| D16: shutdown acceptance for whichever launcher | Not separately tasked: with no pool/fork-server process to out-live the pod (Task 5a-1's decision removes that), the shutdown case S0b found (pool hung when kernels died first) does not arise — `marimo run` in thread mode exits when its one process is killed, same as `marimo edit` already does. If Task 5a-1's measurement surprises this assumption, add a task before 5b before trusting it. |
| §7: Versions panel with restore | 5d-2 |
| §11 "done when": two viewers see results under their own permissions | 5e-1 |
| P1 carry-over items already closed in P4 | none remaining for P5 |

Gaps and honest limits, stated rather than hidden:
- **No pre-warming.** A first-time viewer of a version always pays a cold pod start. Task 5a-1 measures it and hands the number to the user rather than assuming it is fine.
- **Resource cost of pod-per-viewer** is not capacity-planned in this plan beyond reusing the edit pod's existing CPU/memory request/limit numbers; a popular published app with many concurrent viewers creates that many pods. Worth a follow-up once real usage exists, not guessed at here.
- **The Unix-socket fix for marimo's IPC channels was not built** — D24 chose the safe-by-construction option over it. If a future need to share pods across viewers returns (e.g. for density), that patch is the remaining path and is not spiked here.
- **Facts to verify when the implementer reaches them, not assumed while writing this plan:** whether `appsVersions`' generated client really returns newest-first (Task 5d-2 says "confirm, do not assume"); the exact session-count seam in Task 5c-3 (the task itself is written to confirm before building); marimo's `session_manager.py` API for a live session count, only needed if the proxy-side seam turns out not to fit.
