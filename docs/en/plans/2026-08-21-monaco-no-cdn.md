# Stop Loading the Code Editor From a Public CDN

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Every code editor in egret loads from egret's own origin, at the version the lockfile pins, with no third-party runtime fetch.

**Architecture:** Configure `@monaco-editor/react`'s loader to use the bundled `monaco-editor` package instead of its built-in CDN default, wired once at the app entry so component tests are unaffected.

**Tech Stack:** Vite 7, React 18, `@monaco-editor/react`, `monaco-editor`.

## The defect, measured live on kind

Loading an endpoint's Query page fetched **13 files from `cdn.jsdelivr.net`**, including the editor core and the SQL language:

```
cdn.jsdelivr.net/npm/monaco-editor@0.55.1/min/vs/loader.js
cdn.jsdelivr.net/npm/monaco-editor@0.55.1/min/vs/editor/editor.main.js
cdn.jsdelivr.net/npm/monaco-editor@0.55.1/min/vs/sql-32GpJSV2.js
… 10 more
```

Cause: `@monaco-editor/loader/lib/cjs/config/index.js:7` hardcodes `https://cdn.jsdelivr.net/npm/monaco-editor@0.55.1/min/vs`, and nothing in `frontend/src/` calls `loader.config` to override it.

Three consequences, in order of severity:

1. **An egress-restricted deployment has no editor at all.** A data platform running Trino and StarRocks in a private cluster is exactly the deployment where this breaks, and it breaks silently.
2. **The app executes third-party JavaScript fetched at runtime.** `nginx-prod.conf:218` sets only `frame-src`, so no `script-src` policy stands in the way.
3. **The running version is not the pinned version.** The CDN serves **0.55.1**; `package.json` pins **0.56.0**. For Monaco the lockfile is decorative.

**This is not new, but it just got wider.** SQL Lab has had it all along; PR #529 put Monaco on a second page, and notebooks are a third.

## Measured facts to build on

- **Three consumers**, all through `@monaco-editor/react`: `components/sqllab/SqlEditor.tsx`, `components/notebook/CellEditor.tsx`, `components/notebook/monaco/notebookCompletion.ts`.
- **Two languages are needed**: `sql` (`SqlEditor.tsx:141`) and `python` (`CellEditor.tsx:139` — `cell.cell_type === "sql" ? "sql" : "python"`). Completions are registered for both (`notebookCompletion.ts:82`, `:115`).
- The local `monaco-editor` package is **complete** (99MB on disk, including `min/vs/sql-32GpJSV2.js` — the same file the CDN served). It is simply unused.
- `src/main.tsx` is a 229-byte entry importing React, `App` and `index.css`.
- **Tests mock `@monaco-editor/react` per file** (`CellEditor.test.tsx:30`), and `vitest.config.ts` has no monaco alias.

## Global Constraints

- **Behaviour parity is the bar.** Everything that works in the editors today must still work: SQL and Python highlighting, the completion providers, the custom themes (`SqlEditor.tsx`'s `egret-sql-light`/`egret-sql-dark`), find/replace, bracket matching, and the ⌘↵ run binding added in #529. **Do not assume which ESM entry point gives parity — measure it in a browser.** `editor.api` is leaner than `editor.main`; picking the lean one and losing folding or find is a silent regression.
- **Wire it at `src/main.tsx`, not inside the editor components.** Component tests mock `@monaco-editor/react` but not `monaco-editor`; importing the real ESM package from a component would drag it into jsdom for every test that renders an editor. The app entry is never imported by a component test.
- **The worker must not break the test run.** Vite's `?worker` import syntax is a build-time transform; verify what vitest does with it before assuming it is inert.
- eslint baseline **0 errors / 177 warnings**; `tsc --noEmit` clean.
- Run the frontend suite unpiped. NEVER pipe into `head`/`tail` — SIGPIPE kills a run mid-suite while exiting 0.
- **A shell hook rewrites `... | grep` into `rtk grep`, which swallows piped stdin** — redirect to a file and grep the file.
- **Commit before cutting any mutation.**
- `ChartExplore.mount.test.tsx`'s pie test is a known pre-existing flake (2 of 5 runs, ~5.1s against a 5s limit). Do not chase it; do not use it to excuse a real failure.

---

### Task 1: Load Monaco from our own bundle

**Files:**
- Create: `frontend/src/lib/monaco.ts`
- Modify: `frontend/src/main.tsx`
- Test: a new test asserting the loader is configured

- [x] **Step 1: Write the setup module**

It must do three things: register a worker factory on `self.MonacoEnvironment`, import the language contributions for **sql** and **python**, and hand the resulting namespace to `loader.config({ monaco })`.

**Choose the entry point by measurement, not by recall.** Import it, build, then open both editors in a browser and check the parity list above. Report which entry you chose and what you verified — "it rendered" is not parity.

- [x] **Step 2: Import it for its side effect in `main.tsx`**

One line, above `App`. Say in a comment why it is here and not in the editor components — a future reader will otherwise "tidy" it into `SqlEditor.tsx` and pull Monaco into every component test.

- [x] **Step 3: Prove the CDN is gone**

A unit test cannot see a network fetch. Assert what a unit test *can*: that the module calls `loader.config` with a `monaco` instance. **Then verify the real property in a browser** — reload with the network panel and confirm **zero requests to any host other than egret's own origin**.

State honestly in your report which half each check covers. A test asserting `loader.config` was called does not prove the CDN is unused; only the network measurement does.

- [x] **Step 4: Run the suite and measure the bundle**

Report `dist` size before and after, and the largest new chunk. The estimate that justified this approach was ~3MB versus 24MB for self-hosting; if the real number is far off, say so rather than letting the estimate stand.

- [x] **Step 5: Commit**

---

### Task 2: Keep it from coming back

**Files:** `frontend/nginx-prod.conf`, and wherever the CSP lives

- [x] **Step 1: Add a `script-src` policy**

The current CSP (`nginx-prod.conf:218`) sets only `frame-src`, so nothing prevents a future dependency from fetching executable code from anywhere. Add a `script-src` that permits egret's own origin and whatever the app genuinely needs.

**Measure what the app actually loads before writing the policy** — a CSP written from assumption either blocks something real or permits everything. Load every major page with the network panel open and enumerate the script origins.

- [x] **Step 2: Verify nothing broke**

CSP failures are silent in the sense that the page still renders while one feature stops working. Check the editors, the notebook, charts, and the report renderer specifically.

- [x] **Step 3: Commit**

---

## After the plan: check it in a browser

1. **Zero third-party requests** on the Query page, SQL Lab, and a notebook — the whole point.
2. The SQL editor highlights SQL, the notebook highlights Python, and completions still fire in both.
3. The custom light/dark themes still apply, and switching theme still works.
4. ⌘↵ still runs the preview from the Query editor (#529's binding).
5. No CSP violations in the console on any major page.

## Self-Review

**Spec coverage:** the defect's three consequences map to Task 1 (offline + version) and Task 2 (executing third-party code). **Placeholder scan:** Task 1 Step 1 deliberately does not name the ESM entry point, because picking it correctly requires measuring parity rather than recalling an API. **Type consistency:** one setup module, imported once, used by all three existing consumers unchanged.
