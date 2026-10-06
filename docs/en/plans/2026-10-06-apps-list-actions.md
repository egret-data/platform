# Data Apps list actions — implementation plan

Spec: `docs/en/specs/2026-10-06-apps-list-actions-design.md`. Branch `feat/apps-list-actions`.

Two parts, in order: the backend (one agent — its three tasks all edit `routes.rs` and
`store.rs`), then the frontend (one agent). The controller reviews each part and does the live
check itself.

Rules for every task: red test first; `cargo fmt`; no `#[sqlx::test]` (use the crate's
`egret_test_db` fixtures); destructive tests on `MemStore` and the test database only; export
`DATABASE_URL` before any cargo test.

## Part B — backend (`crates/egret-apps`)

### B1 — per-row `can_write` / `can_manage`

- Add both fields to `AppSummary` and `AppDetail` (`routes.rs`), with doc comments saying they
  are per object.
- Compute them with the PDP against the same `ObjectRef` `app_object_ref` builds. `AppSummary::of`
  becomes async or takes the two booleans; pick whichever keeps `list` readable.
- Every handler that returns either DTO fills them in (list, create, get_one, update, restore…):
  find them with the compiler, not by memory.
- Tests (`tests/http.rs` or the nearest existing route test file):
  - a workspace admin sees `true/true`;
  - a member with only `app:read` sees `false/false`;
  - a user holding an object-level write share on ONE app sees `can_write: true` on that row and
    `false` on another row in the same list;
  - for each of those callers, the flag agrees with what `PATCH` on that app then answers.

### B2 — `POST /ws/{workspace}/apps/{id}/duplicate`

- `AppStore::duplicate(ws, user, source_id) -> AppRow`: read the source draft, create the copy
  with it. Reuse `create`'s insert path; do not hand-roll a second one.
- Route + `utoipa` annotation + audit event `app:duplicate`.
- Tests:
  - the copy's draft source and layout equal the source's, byte for byte, read back through
    `AppStore::draft`;
  - the copy has revision 1, no versions, the caller as owner, the source's folder and
    description, the name `Copy of …`;
  - changing the copy's draft afterwards leaves the source's draft unchanged (distinct objects);
  - a caller with only `app:read` on the source gets 403 and no new app exists;
  - a trashed source gets 409; a source in another workspace gets 404.

### B3 — `DELETE /ws/{workspace}/apps/{id}`

- `AppStore::delete(ws, id)`: the transaction in spec §3.3 step 4, then `delete_prefix` after
  commit. Add an `app_prefix(ws, app)` helper beside the existing key helpers so the prefix and
  the keys cannot drift apart.
- Route + annotation + audit event `app:delete`; 204 on success.
- Tests:
  - an app with a draft, a layout, two published versions and a restored version (so a
    `restored_from` row exists) is deleted: no row left in `apps`, `app_drafts`, `app_versions`,
    `app_grants`, pod tokens, bookmarks or `access_grants` for it; no object left under its
    prefix in `MemStore`;
  - a SECOND app in the same workspace, seeded the same way, is untouched: rows and every object;
  - not trashed → 409, nothing deleted;
  - caller with `app:write` but not `app:manage` → 403, nothing deleted;
  - an id from another workspace → 404, nothing deleted there;
  - the object store failing on `delete_prefix` still returns 204 and leaves no rows (use a
    store wrapper that errors on delete).
- Mutation check before calling it done: remove the `deleted_at` guard and confirm the 409 test
  fails; widen the prefix by one path segment and confirm the second-app test fails.

### B4 — OpenAPI and client

- Regenerate `openapi.json` the way the repo does (`cargo run -p egret-bin -- openapi`, check
  the justfile for the exact recipe) and `npm run gen:api` in `frontend/`.
- `cargo test -p egret-apps -p egret-server`, `cargo clippy -p egret-apps -p egret-bin
  --all-targets -- -D warnings`.

## Part F — frontend

### F1 — API layer (`src/api/apps.ts`)

- `useApps(ws, { trashed })`; `useAppMutations(ws)` with update, trash, restore, remove,
  duplicate, addBookmark, removeBookmark. Follow `api/dashboards.ts` for shape and invalidation.
- Tests beside `api/__tests__/folders.apps.test.tsx`: each mutation hits the right method and
  path and invalidates the list key.

### F2 — `AppActionsMenu` and `RenameAppDialog` (`src/components/apps/`)

- Menu per spec §4.2. Model on `DashboardActionsMenu`; reuse `RowActionsMenu` and
  `MoveToFolderItems`.
- Tests: the table in §4.2 as cases — for each of `can_write` × `can_manage` × trashed ×
  published, the exact set of items rendered, and no trigger at all when the set is empty.

### F3 — `Apps.tsx` on the list scaffold

- Rebuild on `ListPageShell` / `ListStates` following `Dashboards.tsx` line for line where the
  behaviour is the same (folders, collapse, drag, bulk, trash switch, search, sort, summary).
- Per-row gating from `can_write` / `can_manage`; bulk checkboxes only on writable rows.
- Keep `NewAppDialog`'s copy; the header button becomes `+ App`.
- Update `routes/explore/__tests__/Apps.test.tsx`: existing cases still hold where behaviour is
  unchanged; add trash view, bookmark filter, bulk over mixed-permission rows, the three name
  link targets, delete-permanently confirmation.

### F4 — gates

`npm run lint`, `npm run typecheck`, `npm test` (whole suite — a per-file run cannot see a mock
another file needs), `npm run build`.

## Part V — controller's verification

- Whole-branch review of both parts against the spec.
- `cargo test --workspace` and the frontend gates on the final tree.
- Live check on a local stack (egret + Vite + RustFS + Postgres), in a browser: every menu
  action as an admin; the same list as a share-only collaborator; a permanent delete confirmed
  by listing the bucket before and after. Results in the PR description.
