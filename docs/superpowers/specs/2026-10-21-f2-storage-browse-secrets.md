# F2: Storage browse and system secrets (KI-206, KI-129, KI-207)

Package F2 of the known-issues fix programme
(`docs/superpowers/plans/2026-10-21-known-issues-fix-programme.md`).

## Premise, re-checked on the Docker demo stack (main `50235eed`)

| Row | Result |
|---|---|
| KI-206 | Reproduces. A new user holding only `data_steward` lists `query-results/`, `apps/` and `workflow/` at the root of `demo-rustfs` and downloads `query-results/<id>.jsonl` (200, another user's rows). |
| KI-129 | Reproduces. `sandbox-lake` (Key prefix `sandbox/`) lists the bucket root; `download` ignores the prefix too. |
| KI-207 | Reproduces by reading the code: the six creation sites mint ordinary named secrets, the Secrets routes rotate, rename and delete any of them, and every picker lists `GET /control-plane/secrets` unfiltered. The demo stack has no engine at the time of the check, so no such secret exists there. |

## Decisions

**Storage (KI-206, KI-129).** One module, `egret-controlplane/src/storage_scope.rs`, resolves every key Browse and Download take and filters every listing:

- A key is relative to the connection's Key prefix. Listings answer keys relative to it and carry `root` (the prefix) so the page can show where it is.
- A key is a plain object key: no `.`/`..` segment, no empty segment (leading `/`, `//`), no `\`, no control character; the same checks run on each further percent-decoding, so a server that cleans or decodes paths cannot be walked out of the prefix. A broken key is a 400.
- In the platform's own bucket (`[storage] bucket`), the prefixes egret writes (`engine.results_prefix`, `apps.storage.prefix`, `workflow.storage.prefix`) are closed to anyone who is not a platform admin: a 403 that says why, and listings leave them out (`hiddenSystemFolders` names what was left out). The match ignores case. These hold other users' data (post-mask SQL Lab results, app sources, workflow files), which `storage:read` was never meant to grant.
- A platform admin may open them: the role already holds the platform's storage credentials and impersonation, so refusing it here would protect nothing. Path tricks stay refused for an admin.
- The connection's bucket is compared by name only (not endpoint), so an alias of the same server cannot reopen the prefixes; a same-named bucket elsewhere is over-closed, which is the safe side.

**System secrets (KI-207).** A `system_managed` column on `workspace_secrets` (migration 0200), set by the code that mints the secret (`WorkspaceSecretStore::create_system`), not inferred from the name. Existing rows are backfilled from the engine and catalog-attachment columns that point at them, plus the fixed name and description of a minted secret for rows no column points at any more. The Secrets routes refuse to rename, rotate or delete a system secret (409, with the reason); the engines and catalogs that own them keep rotating and deleting them through the store. The API carries `system_managed`; the Secrets list shows a *System* badge and no row actions for them; every secret picker leaves them out.
