Package F2 of the known-issues fix programme (#708). Spec: `docs/superpowers/specs/2026-10-21-f2-storage-browse-secrets.md`.

## KI-206 (S2, security): stored SQL Lab results readable with `storage:read`

**Premise (demo stack, main `50235eed`):** a new user holding only `data_steward` listed `apps/`, `query-results/`, `workflow/` at the root of `demo-rustfs` and downloaded `query-results/<id>.jsonl` (200, another user's rows). Reproduced.

**Rule:** in egret's own bucket (`[storage] bucket`), the prefixes egret writes (`engine.results_prefix`, `apps.storage.prefix`, `workflow.storage.prefix`) are closed to anyone who is not a platform admin. Browse leaves them out and says so (`hiddenSystemFolders`, and a line on the page); listing or downloading under them is a 403 whose message says why and who can. The match ignores case and runs on the key and on each further percent-decoding. A platform admin may open them (the role already holds the platform storage credentials and impersonation; refusing it here would protect nothing).

**One place:** `egret-controlplane/src/storage_scope.rs` (`ConnView::scope_key` for every key both routes take, `ConnView::visible` for every listing entry). Path tricks are a 400 for everyone, admin included: `.`/`..` segments, empty segments (leading `/`, `//`), `\`, control characters, also once percent-decoded. The connection's store is opened through a new `StorageScope::opener` seam so the routes are tested end to end with a fake S3 that cleans paths and ignores case, as a lenient server would.

## KI-129 (S3): Browse ignores the connection's Key prefix

**Premise:** `sandbox-lake` (Key prefix `sandbox/`) listed the bucket root; `download` ignored the prefix too. Reproduced.

**Fix:** keys taken and returned by Browse and Download are relative to the connection's Key prefix; the listing carries `root`, and the page's root crumb reads `warehouse/sandbox`. A malformed stored prefix is refused rather than trusted.

## KI-207 (S3): system-managed secrets editable and offered in pickers

**Premise:** reproduced from the code (the demo stack had no engine, so no such secret existed there): the six minting sites create ordinary secrets, the routes rename/rotate/delete any secret, every picker lists all of them.

**Fix:** `workspace_secrets.system_managed` (migration **0200**), set by `WorkspaceSecretStore::create_system`, which the minting sites now call (engine internal, monitor, keystore, Trino delegator, StarRocks root, minted Catalog Client credential). Existing rows are backfilled from the engine and catalog-attachment columns that point at them, plus the exact name and description of a minted secret for rows no column points at any more. `PUT`/`DELETE /control-plane/secrets/{id}` answer 409 *«name» is managed by egret: it is rotated and deleted with the engine or catalog that created it, not here*; the owning engines and catalogs still rotate and delete through the store. The API carries `system_managed`. Frontend: a *System* badge in the Secrets list; Delete, Rotate value and Edit disabled with that reason (list and detail page); every secret picker (`SecretPicker`, catalog kinds, engine create/security, storage create/detail) leaves them out.

## Evidence

- Red then green, backend (through the real handlers and the Cedar PDP, as a `data_steward`): `crates/egret-controlplane/tests/storage_browse_scope.rs` (5 of 6 failed on the old handlers; the "normal key still works" control passed both times), `crates/egret-controlplane/tests/system_secrets.rs` (failed on the old routes: no flag, 200 on rename/rotate/delete). Unit tests for the key rule in `storage_scope.rs`; engine-mgmt tests now assert the root and delegator passwords are minted as system secrets.
- Red then green, frontend: `StorageBrowser.test.tsx` (new), `ControlPlaneSecrets.test.tsx`, `SecretPicker.test.tsx` (4 failed on the old components, pass now).
- Checks: `cargo fmt --check`, `cargo clippy -p egret-controlplane -p egret-secrets -p egret-engine-mgmt -p egret-bin --all-targets -D warnings`, `cargo test -p egret-secrets -p egret-controlplane -p egret-engine-mgmt` (green); `npm run lint` (0 errors), `tsc -b`, `vitest run` (7803/7804; the one failure, `OutputsTree.test.tsx`, is a timing flake under load and passes alone), `npm run build`.
- `openapi.json` regenerated (`cargo run -p egret-bin -- openapi`), client via `npm run gen:api`.

## Live check (Docker demo stack, `egret` + `frontend` rebuilt from this branch, 1440x900 light)

- As a fresh `data_steward` user: the `demo-rustfs` root lists only the warehouse folders and the line *apps, query-results, workflow are not shown…*; `?prefix=query-results/` shows the 403 sentence; download of a results file, of `QUERY-RESULTS/…` and of `query%252Dresults%252F…` is 403; `../`, leading `/` and `//` keys are 400; a parquet data file still downloads (200). `sandbox-lake` opens at `warehouse/sandbox` (empty) instead of the bucket root.
- As admin: the root lists `apps/`, `query-results/`, `workflow/` and a results file downloads.
- Secrets: an `f2-` secret flagged `system_managed` in the DB (standing in for a minted one: the demo has no engine) shows the *System* badge, its Delete/Rotate/Edit are disabled with the reason, `PUT`/`DELETE` answer 409, and the catalog JDBC *Password secret* picker does not offer it. The test secret and user were removed afterwards.

## Not verified / notes

- The minting path itself (an engine apply creating `engine-internal-*` etc. with the flag) is covered by tests, not by a live engine on the demo stack.
- KI-207's side note (*Used by* and *Created* cut off at 1440 px) did not reproduce with the demo's secrets at 1440x900.
- Migration number 0200: shared namespace, renumber at merge if another package takes it.
- Docs (`egret-data/docs`): per the programme, rows are closed in the wave's docs PR; the Storage Browse and Secrets pages should mention the hidden system folders and the System badge.
- Found and not fixed: `storage:read` still lets a data steward download the raw Iceberg data files of every warehouse in the bucket (e.g. `…/data/*.parquet`), which also bypasses column masks and row filters; that is the same class as KI-206 but about table data, not egret's own prefixes.

Lets the docs close: KI-206, KI-129, KI-207.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

## Artifacts

Screenshots and logs: scratchpad `f2/` (before-*.png, after-*.png, live-api-after.txt, red-storage.log, fe-red.log, checks.log).
