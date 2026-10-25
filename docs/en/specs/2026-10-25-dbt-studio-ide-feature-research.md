# dbt Studio IDE: feature research

> **Status:** research only, canonical (English). No decision has been made on whether or how egret builds this. It records what the product does so a later spec can choose scope.
> **Date:** 2026-10-25.

Sources (all fetched 2026-10-25; the docs describe the dbt platform product, not dbt Core):

- <https://docs.getdbt.com/docs/platform/studio-ide/develop-in-studio?version=2>
- <https://docs.getdbt.com/docs/platform/studio-ide/ide-user-interface>
- <https://docs.getdbt.com/docs/platform/studio-ide/lint-format>
- <https://docs.getdbt.com/docs/platform/studio-ide/keyboard-shortcuts>
- <https://docs.getdbt.com/docs/platform/studio-ide/develop-studio-ai>
- <https://docs.getdbt.com/docs/platform/studio-ide/git-commit-signing>
- <https://docs.getdbt.com/docs/platform/git/merge-conflicts>
- <https://docs.getdbt.com/docs/platform/about-defer>

Not fetched: Lineage tab internals, Semantic Layer / MetricFlow commands, "Build and view docs", `dbt-jsonschema` repo, environment variables page, dbt AI FAQs, Manage dbt AI.

## 1. What it is

A web IDE that edits, compiles, runs, tests and version-controls a dbt project. It compiles dbt code to SQL and executes it on the user's warehouse using the user's own development credentials.

Prerequisites on the dbt platform: a Developer seat licence, a git repository with write access, a project connected to a data platform, a development environment with user credentials, dbt 1.0 or higher.

## 2. Layout

| Area | Contents |
|---|---|
| Left side menu | Repo link (repo name + branch), documentation-site button, Version Control menu, file explorer, file-tree / search toggle |
| Centre | Tabbed file editor, minimap, git diff view |
| Console (below editor) | Buttons: Preview, Compile, Build, Lint, dbt Wizard. Tabs: Commands, Problems, Results, Code quality, Compiled code, Lineage, plus Markdown and CSV preview tabs |
| Bottom bar | Command bar, Defer menu, Status button, dbt version, project status (error and warning count), options menu |
| Overlay | Invocation History Drawer (opens with Ctrl+backtick) |

Dark mode toggle in the status bar.

## 3. File explorer

- Click to open, drag and drop to move between directories, double-click to rename.
- Indicators: `•` unsaved, `M` modified, `A` added, `D` deleted.
- Right-click menu: copy name, copy relative path, **copy as `ref`**, download, duplicate, rename, delete.
- Breadcrumb at the top of the editor; right-click a breadcrumb to open adjacent files in that folder.

## 4. Editor

- Tabs: reorderable, right-click menu (for example duplicate), unsaved tabs show a blue dot, closing several unsaved tabs saves them together (batch save), unsaved text is held in browser local storage.
- SQL syntax highlighting.
- Find and replace in the file (Cmd/Ctrl-F). Multiple cursors. Go to line.
- Minimap.
- Block comment shortcut inserts Jinja comments `{# #}` in SQL files.
- Typing `__` lists dbt functions.
- Markdown preview tab (renders `.md` live) and CSV preview tab (renders seed CSVs as a table live).
- Git diff view: previous version on the left, inline changes on the right.

### dbt-aware autocomplete

| Trigger | Completes |
|---|---|
| `ref` | model names |
| `source` | source name, then table name |
| `macro` | macro arguments |
| `env_var` | environment variable names |
| `-` in YAML | inline YAML completion |

### YAML validation

Uses JSON Schema from the open-source `dbt-jsonschema` project for completion, structure checks and inline diagnostics. The schemas follow the dbt v2 spec and apply to all release tracks. The docs say to trust the dbt commands if a warning looks wrong.

### CodeGenCodeLens

A **Generate model** link next to a table in a source YAML file creates a basic staging model.

## 5. Search and command palette

- Project-wide search with match case and whole word, replace field, replace-all, per-result replace, replacement preview.
- Command palette (Ctrl/Cmd+P): Go to File, Show and Run Commands (IDE actions only, not dbt commands), Search for Text, Go to Symbol in Editor, Go to Line/Column, Go to Symbol in Workspace, search open files only. Git actions, compile and preview are also reachable from it.

## 6. Running dbt

### Command bar

Accepts dbt commands with node selection syntax. Output goes to the Invocation History Drawer.

### Console buttons

| Button | Behaviour |
|---|---|
| **Preview** | Runs the active SQL (saved, unsaved or a highlighted selection) and shows rows in Results. Does not require saving. Default 500 rows; add `limit N` in the SQL to change it; from dbt 1.6 a "Change row display" control raises the default up to 10,000 until refresh. `SELECT TOP #` is accepted. The limit must be literal, not from a macro. |
| **Compile** | Compiles saved or unsaved SQL into the Compiled code tab. From dbt 1.6 saved models compile with context, so `{{ this }}` and `{{ is_incremental() }}` resolve. |
| **Build** | Dropdown for `dbt build`, `dbt test` or `dbt run` on the active model, scoped to: only this resource, plus upstream, plus downstream, or all dependencies. Available for all executable nodes. dbt invocations require the file to be saved. |
| **Lint** | Runs the linter on the active file (see section 9). |
| **dbt Wizard** | AI agent (see section 11). |

Other run features: build only modified models, resume a build from the last failure, MetricFlow commands, build and view docs for in-development changes, special environment variables (for example branch name) to build dynamic schema names, real-time refresh of the IDE as models, tests, seeds and operations run.

### Invocation History Drawer

- List of past invocations: command, branch, status, elapsed time.
- Summary for the selected one; a System Logs toggle shows full stdout and debug logs.
- Command Control button: cancel or rerun.
- Node results filtered by status: Pass, Warn, Error, Skip, Queued; per-node summary and debug logs.
- Failed models and tests expose downloadable run logs.

### Problems tab (dbt v2 only)

Project issues that would block runs, updated as the user edits. The bottom bar shows the count.

## 7. Lineage

- DAG of the active model. Default selector `2+model+2`; switchable to `+model+` for the full DAG.
- Double-click a node to open its file. Node selection syntax expands or shrinks the graph.
- `--exclude` is not supported.
- Known issue: the IDE can freeze with two or more models open, the Lineage tab active and a large DAG.

## 8. Defer

Builds and tests only what changed; unchanged upstream `ref()` calls resolve to existing staging or production objects using that environment's manifest.

- Needs one environment marked Production and a previously successful production deploy job. CI jobs are excluded. If a Staging environment exists, its artifacts are used, and defer fails if no job ever ran there.
- Resolution order per ref: development relation if it exists, otherwise staging, otherwise production. One environment per invocation. `--favor-state` skips the development step.
- Toggled from the Defer menu in the bottom bar: Development environment (defer off), dbt default behaviour (staging else production, recommended), Custom environment.
- `defer-env-id` in `dbt_project.yml` selects a specific environment.
- In dbt Core the same effect needs `--defer` plus `--state` pointing at a `manifest.json` that the user manages. The IDE fetches and supplies it automatically. Typical use: new branch, empty dev schema, edit one model, `dbt build -s state:modified`.
- Self-managed setups can use `defer_to_target` in `profiles.yml` (dbt State) so `state:*` selectors compare each node with its own last execution.

## 9. Lint and format

| File type | Lint | Format |
|---|---|---|
| `.sql` | SQLFluff (lint and fix) | sqlfmt, or SQLFluff when selected |
| YAML, Markdown, JSON | none | Prettier |
| `.py` | none | Black |

- The Code quality tab has a Config dialog to choose SQLFluff (lint and format) or sqlfmt (format only). A `.sqlfluff` file in the project root switches the default to SQLFluff.
- **Lint file** shows wavy underlines with hover details and Quick Fix. It does not re-run on save. **Lint and Fix file** auto-fixes. Linting runs on all modified `.sql` files on the branch by default.
- SQLFluff configuration: `.sqlfluff` at the project root, optional nested `.sqlfluff` in subfolders, `.sqlfluffignore` (snapshots must be excluded by hand). Restart the IDE after changing config. The dbt Labs example uses `templater = dbt`, which is slower because it compiles first.
- sqlfmt: no custom configuration. Prettier: standard config precedence; `.prettierrc.json5`, `.js` and `.toml` are not supported.
- Limits: 60 seconds on the backend, then "Unable to complete linting this file"; fixing is harder than linting; no terminal access to SQLFluff; ephemeral models unsupported on dbt 1.5 and lower; on the v2 stable track linting still uses the v1 engine.
- Formatting and linting work on the protected branch; commits there are blocked and the IDE asks for a new branch.

## 10. Version control

- Git actions button whose label and action change with state: pull, commit and sync, create merge/pull request, prune branches. Dropdown: revert changes, refresh git state, create merge/pull request, prune branches, change branch.
- Changes section lists everything since the last commit; click a file for the diff view. Right-click a changed file to commit or revert that single file.
- Commit Changes modal, Change Branch modal, Revert Uncommitted Changes modal.
- Protected primary branch: edit, format, lint and run dbt there, but commits are blocked and the user is prompted to commit to a new branch.
- Prune Branches: deletes local branches already removed from the remote; never the current branch; not available for managed repositories.
- Branch link opens the repo (GitHub or GitLab on multi-tenant accounts only).

### Work retention

| State | Stored | Notes |
|---|---|---|
| Unsaved | browser local storage | lost if the user switches branch or browser without saving (warning shown) |
| Saved, uncommitted | durable dbt storage | not in git; must commit and sync or revert before Change branch |
| Committed | git provider | safe |

### Merge conflicts

- Occur on pull or on commit when someone else changed the same lines.
- A **Commit and resolve** action bar replaces Commit. Conflicting files are red with a warning icon in the file catalog and in Changes.
- Two-pane editor: left pane (your changes) read-only, right pane (incoming) editable with the conflict markers. Delete markers, choose lines, Save (highlight disappears), repeat per file, then Commit and resolve.
- Limits: a conflicting file opened from Changes is fully read-only; every conflict must be resolved before the commit.

### Signed commits

Enterprise and Enterprise+ only. The platform generates a GPG keypair per user, signs commits made in the IDE, and the user uploads the public key to GitHub or GitLab (Azure DevOps unsupported). The dbt email must be a verified email at the git provider. Commits from other tools are not signed.

## 11. AI features

Platform services, usage-based, enabled by default, admin-controllable.

- **dbt Wizard** (preview, all accounts): agent grounded in the dbt Information Schema (lineage, tests, model health, semantic definitions). Modes: Explore only, Ask for approval (default, per-file approval of drafted edits), Edit files automatically. `@` scopes the agent to a model. It may request dbt commands (approve once, approve for the session, deny) and they are marked in the Commands tab. Also: debug job failures with a preconfigured skill, a dbt v2 migration workflow (iterative `dbt compile` and fix), custom skills for team conventions. Limits: Studio IDE only, no plan mode, chat history 90 days, new tab starts empty, commands over 5 minutes are cancelled.
- **dbt Copilot** (limited availability): one-click docs, tests, semantic models and metrics for a selected model; inline SQL edit with Cmd/Ctrl+B and accept/reject diff. Does not yet support the latest semantic model YAML spec.

## 12. Keyboard shortcuts

| Action | macOS | Windows/Linux |
|---|---|---|
| Full list of editor shortcuts | Fn-F1 | Fn-F1 |
| Open a file | Cmd-O | Ctrl-O |
| Close active tab | Option-W | Alt-W |
| Preview | Cmd-Enter | Ctrl-Enter |
| Compile | Cmd-Shift-Enter | Ctrl-Shift-Enter |
| Dbt functions list | type `__` | type `__` |
| Command palette | Cmd-P / Cmd-Shift-P | Ctrl-P / Ctrl-Shift-P |
| Multi-cursor | Option-Click, Shift-Option-Click | Alt-Click, Shift-Alt-Click |
| Invocation History Drawer | Ctrl-backtick | Ctrl-backtick |
| Block comment | Shift-Option-A | Shift-Alt-A |
| Save | Cmd/Ctrl-S | |

## 13. Status bar and options menu

Status button: warehouse and LSP connection (dbt v2) or engine server status (v1), with shortcuts to environment settings and credentials; the Status modal shows the current server error, debug logs and a restart option. dbt version display with a personal version override. Options menu: view status details, restart Studio (unsaved work lost), reinstall dependencies, clean dbt project (`dbt clean`), autofix deprecation warnings (prepares for dbt v2).

## 14. Sessions and limits

- Start-up: creation start (first launch, clones the repo, slowest), cold start (new session within one hour), hot start (resume within one hour).
- Session turns off after one hour without activity. Compile, preview and dbt invocations count as activity; editing and saving do not.
- Repositories over 6 GB need support approval. Credentials must be personal, not superuser or production. Saved-but-uncommitted changes block switching branch. Ad blockers can break files such as `google_adwords.sql`.

## 15. What depends on dbt platform services

The IDE itself and its hosted dev environment; Developer seat and per-user credentials; defer artifacts from production/staging jobs; dbt Wizard and Copilot; Semantic Layer and MetricFlow; build and view docs; signed commits; the scheduler and deployments.

## Appendix A: what other tools already offer (for comparison only)

**Obelisk dbt Studio** (a local clone of the Obelisk repository, read 2026-10-25): file tree with New Model / New YAML, multi-tab Monaco with split compiled-SQL pane, target selector, Save / Compile / Preview Data / dbt run / Run Tests, a dock with Data Preview, one-hop Model Lineage DAG and Terminal & Logs. Shells out to `dbt` per action; lineage from `manifest.json` with a regex fallback. Gaps against the table above: no autocomplete, no YAML schema validation, no lint or format, no streaming logs in Studio, no invocation history, no defer or `state:modified`, no git UI, no command bar or palette.

**Kestra dbt plugin** (<https://kestra.io/plugins/plugin-dbt>): `io.kestra.plugin.dbt.cli.DbtCLI` runs any dbt command in a container via a task runner, parses run results, takes `profiles` inline with `secret()`, has `storeManifest` / `loadManifest` through the KV store, optional `assets.enableAuto`, and `alertWebhook`. `dbt.cloud.TriggerRun` and `CheckStatus` drive dbt Cloud. The per-command tasks are deprecated. It is an execution task only; Kestra has no IDE for dbt.

## Appendix B: open questions for the decision (not answered here)

1. Identity dbt runs under on a Ranger-enforced engine (the person, or a workspace service account).
2. Git: use the platform's own file revisions, or connect a real git repository.
3. First target engine: Trino, StarRocks or both.
4. Whether interactive Compile/Preview shares a runner with workflow task execution, or has its own.
5. Which of sections 4 to 14 are in scope for a first release.
