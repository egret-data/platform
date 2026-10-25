# dbt Studio and a git-based, multi-environment development process: design notes

> **Status:** discussion record, canonical (English). **Nothing here is decided.** It keeps the positions stated by the product owner, the proposals made in response, and the questions still open, so a later spec can start from them instead of from memory.
> **Date:** 2026-10-25.
> **Inputs:** `docs/en/specs/2026-10-25-dbt-studio-ide-feature-research.md` (what the dbt platform Studio IDE does); a read of the Obelisk and dagron repositories; the Kestra dbt plugin page; the egret workflow schema (`migrations/0132_workflow_flows.sql`).

## 1. Why this exists

The wish is a dbt Studio inside egret. The discussion widened into one development process shared by dbt projects and workflows: edit in the UI, keep everything in git, promote through environments. This note records that thinking in order, marks who said what, and lists what must be checked before a spec.

Labels used below: **[Stated]** is a position the product owner gave. **[Proposed]** is a suggestion made in the discussion and not yet accepted. **[Open]** needs a decision. **[Unverified]** is a claim not checked against code.

## 2. Positions stated by the product owner

- **[Stated]** Git is the source of truth, as in GitOps. Changing environment should be simple.
- **[Stated]** One workspace is one project or team. Environments live **inside** a workspace; they are not separate workspaces (three workspaces per project would triple members, roles and secret setup).
- **[Stated]** A **personal environment** exists for each user. It runs with the personal user's own permissions, and lets the user change the environment, for example write to a personal schema. Only prod writes to the prod schema.
- **[Stated]** **prod and uat run as a service account.**
- **[Stated]** Developing on **real data** is the point: data development on fake data hides errors. Reads are of real data; writes go to the personal schema.
- **[Stated]** dbt development should be the model for developing workflows too: everything developed on the UI and integrated with git.

## 3. What already exists (checked)

- Flows are two tables: `workflow.flows` (the draft, unique on `(workspace_id, namespace, flow_id)`, `state` in `draft|live|disabled`) and `workflow.flow_versions` (immutable snapshots with references resolved at publish). There is no environment concept in the workflow schema.
- Egret mints a per-user engine token from its own machine IdP (claims `sub`, `preferred_username`, `groups`, `aud: engine`; 15 minute TTL). Ranger decisions and classification clearance follow that user.
- Namespace files (W9b), KV (W9c) and execution files (W9a) are scoped by workspace and namespace. Trigger runtime switches live in `workflow.disabled_triggers`.
- Egret has no dbt code. The only mention is in the view-governance guide (a dbt run with more than 8 threads over classified catalogs sees hold refusals).
- **[Unverified]** Project notes say every execution runs the saved draft, while the comment in migration 0132 describes the opposite intent. Confirm in code before any design that depends on which one is true.

## 4. Proposed model

### 4.1 Environments inside a workspace [Proposed]

| | personal (one per user) | uat | prod |
|---|---|---|---|
| Runs as | the user (delegated engine token) | service account | service account |
| Git ref | the user's branch (working copy) | `main` | a tag or SHA, with approval |
| Writes to | personal schema | uat schema | prod schema |
| Schedule triggers | off | optional | on |
| Reads | real data, within the user's grants | real data | real data |

A flow becomes a definition (identity `(workspace, namespace, flow_id)`) crossed with an environment, pointing at a deployed version. Two people editing the same flow no longer collide: each has a personal deployment. Every execution carries an `environment_id`.

### 4.2 Git model [Proposed]

- Trunk-based. `main` is the reference state; short-lived feature branches; changes enter `main` by pull request. GitFlow with long-lived `develop` and `release` branches is not proposed.
- An environment is not a branch. Each environment has a **binding**: `{repository, path, ref, auto-sync or not}`, where `ref` is a branch, tag or SHA (the Argo CD style).
- **Deploy** moves the ref. **Rollback** moves it back. A reconciler compares the tree at the ref with the workspace state and publishes new `flow_versions`, each stamped with the commit SHA.
- Only **definitions** live in git: flows, namespace files, dbt projects. Operational state does not: executions, KV values, secrets, variable values, the runtime trigger switches.
- Pull request checks reuse existing parts: the publish-time validation (a pure function), a semantic `plan` that shows what a merge changes in each environment (an idea from dagron's `dagron plan`), and `dbt compile` for dbt.

### 4.3 What changes for workflows [Proposed]

- In an environment managed by git (uat, prod), the API and UI **refuse** direct writes; only the reconciler writes. Refusing, not ignoring, matches the no-silent-drops direction.
- In personal, the "draft" is the working copy of the user's branch. Save writes the working copy, Commit pushes the branch, Run runs the saved copy as today. Present semantics survive in personal and are tightened only where git governs.
- The Publish button becomes Commit / Open PR.
- Emergency edits go through a hotfix branch and pull request, or a break-glass switch that still produces a commit.

### 4.4 Data and identity [Proposed, partly Unverified]

- Personal reads real data under the user's grants and clearance. This works because engine tokens are per user.
- **Masking can differ between personal and prod**, since prod runs with the service account's clearance. uat running as the same service account as prod is where that difference shows up.
- Redirecting writes to the personal schema must be enforced **structurally**. Rewriting SQL is fragile (project history records engine-lexer mismatches). Proposed: (a) flows and dbt name their targets through an environment variable (dbt has `target` and `generate_schema_name`; workflows would use something like `{{ env.schema }}`), and (b) a backstop in which personal runs authenticate as a **derived principal** whose policy, compiled by egret as Ranger Admin, reads as the user and writes only to the personal schema, so a hard-coded prod write is refused by the engine.
- **[Unverified]** Whether the SP4a permission model can express "write only to one schema" for a derived principal.
- Secrets: prefer identity (the user's engine token) over secrets. Prod secrets must not be readable from personal.
- Tasks with external side effects (webhook, HTTP, chat, email) need a per-environment policy: allow, deny, or dry-run.

## 5. Costs that the environment dimension brings

Everything keyed by `(workspace, namespace)` gets an environment dimension, or personal can overwrite prod:

- KV store, namespace files and execution files: the rule "a flow writes only its own namespace" becomes "its own namespace **in its own environment**". Option: copy-on-write overlays where personal reads through to uat/prod and writes to its own layer.
- Trigger state: depends-on windows, runtime enable switches, concurrency, backfill.
- Secrets and variables per environment.
- Subflows resolve within the same environment; execution lists, metrics and logs filter by environment.
- Plugin configuration and the container allowlist: decide whether they vary by environment.

## 6. Prerequisites and risks

- **Service account as a principal** is a hard dependency for uat and prod. Project notes place it after the data-governance debt; it is not a full principal today **[Unverified: re-read current state]**.
- **Promotion is a privilege-escalation path.** Someone with no prod write grant could deploy a flow that writes prod through the service account. The rights to edit a flow, to change an environment binding and to approve a deployment must be separate, and the pull request review is part of the control.
- Git credentials need a home in the workspace's secret store, and changing a binding is a sensitive permission.
- Two-way drift: people may commit outside the UI. Needs a webhook or poll, and a rule for which side wins.
- Existing flows created on the UI need a one-time export into a repository.
- Before deploying, check that the target environment has the connections and tables the flow references.

## 7. Options considered

| Option | Verdict so far |
|---|---|
| One workspace per environment (dev, staging, prod) | Rejected by the product owner: multiplies workspaces, members, roles and secrets. |
| Database as the source of truth, git as a sync target (Kestra style) | Simpler and keeps Draft/Publish, but git is only a backup and review aid, not the control point. Not chosen. |
| GitFlow (long-lived `develop`, `release`) | Not proposed: extra merges and cherry-picks without benefit. |
| Environment as a namespace prefix or `{{ env.X }}` variable only | Does not isolate state or identity; not proposed. |

## 8. Module question for dbt [Proposed]

- A dedicated dbt module (its own navigation area, like Data API and Workflow) for the Studio and the project store, plus a thin `dbt` task in the workflow engine for scheduled production runs.
- A shared git-backed layer under both (connect repository, branch, commit, diff, revert, conflicts), working on a file tree and knowing nothing about dbt or flows.
- The dbt module should reuse the container runner of the workflow engine (W5a), not workflow executions, so that Compile and Preview do not each create an execution. **[Unverified]** Whether the W5a runner can be called outside an execution. If not, either extract a shared primitive or hide an execution per interactive call.
- Kestra ships `io.kestra.plugin.dbt.cli.DbtCLI`. It could run through the W5b plugin host if the dbt plugin is added to `plugin-host/plugins.lock`, but the host currently covers plugins without storage or secret needs, and `profiles` with `secret()` and `storeManifest` (KV) need checking. **[Unverified]** A spike is the way to find out. The alternative is `egret.container` with a dbt image.
- Kestra has no dbt IDE; the Studio is new work either way.

## 9. Open decisions

1. **Scope of the first release.** Full environment dimension across KV, files, triggers and secrets, or a first release limited to dbt and flows that do not write KV or files, with personal and prod only. Leaning: the narrower one.
2. **Service account.** Build it as its own package first, or stand in a technical user for prod at the start.
3. **Secrets in personal.** The user's own secrets, a shared dev set, or none (engine access by token only).
4. **Prod promotion.** Approval of a tag or SHA, or automatic from `main`.
5. **First engine.** Trino, StarRocks or both.
6. **Git depth.** Revisions of namespace files only, or a real repository with branches and conflict handling.
7. **Flow state in git-managed environments.** Whether the existing draft is removed there or kept as a read-only mirror.

## 10. To check before writing a spec

- Whether executions read the draft or the published version (section 3).
- The SP4a permission model: derived principal that reads as the user and writes to one schema.
- The state of the service account principal work.
- Whether the W5a container runner is callable outside an execution, and its start-up time for `dbt compile`.
- The dbt adapters for Trino and StarRocks and the dbt-core versions they pin (the local `dbt-starrocks` requires `dbt-core>=1.9`).
- How the Kestra git tasks work and whether they are reachable through the plugin host.
- Where the environment dimension must reach (a survey of every table keyed by workspace and namespace).
