# Data Apps — list actions

**Status:** proposed, 2026-10-06. Decisions A1–A3 below were made with the user; the rest are
this document's proposals and are marked as such.

## 1. Problem

The Apps list (`frontend/src/routes/explore/Apps.tsx`) can create an app, open it and edit it.
Nothing else. The backend already serves rename, trash, restore, bookmark, folder and sharing
for apps, and the page offers none of them, so an author cannot rename an app, tidy it into a
folder, bin it or share it without calling the API by hand. Every other Explore list
(Dashboards, Charts, Datasets) has these, on a shared scaffold.

Two actions the other lists have do not exist for apps on the backend at all: **delete
permanently** and **duplicate**. And the list response does not say what the caller may do to
each row, so the page cannot withhold an action the server will refuse.

## 2. Decisions

| # | Decision | Made by |
|---|---|---|
| A1 | **Delete permanently** is in scope: a new backend route plus the Trash-view action. | user |
| A2 | **Duplicate** is in scope: a new backend route plus the row action. | user |
| A3 | The backend reports **per-row** `can_write` and `can_manage`; the page shows only what the caller can do. | user |
| A4 | Permanent delete is allowed **only on a trashed app** (409 otherwise). Two deliberate steps before content is destroyed. | proposed |
| A5 | Permanent delete needs **`app:manage`** on the app, like sharing — the same bar Dashboards set for theirs. | proposed |
| A6 | Duplicate needs **`app:write` on the source** and workspace `app:write`. Not `app:read`: the copy is made from the **draft**, and a reader may not see an unpublished draft today (`draft_get` requires `app:write`). | proposed |
| A7 | A duplicate copies the draft's source and layout only. No versions, no shares, no bookmarks; the caller owns it; it lands in the source's folder; it is named `Copy of <name>`. A trashed app cannot be duplicated (409). | proposed |
| A8 | The page moves onto the shared list scaffold (`ListPageShell`), and the per-row **Open** / **Edit** buttons move into the row menu, as on the other lists. The header button becomes `+ App`. | proposed |

## 3. Backend

All in `crates/egret-apps` (`routes.rs`, `store.rs`), OpenAPI regenerated.

### 3.1 Per-row capabilities (A3)

`AppSummary` and `AppDetail` gain:

- `can_write: bool` — the caller holds `app:write` on this app.
- `can_manage: bool` — the caller holds `app:manage` on this app.

Both are evaluated per object with the same `ObjectRef` the mutating handlers build
(`kind: "app"`, the row's `folder_id`), so the flag and the later refusal cannot disagree.
This is unlike `DashboardSummary.can_manage`, which is workspace-scoped and identical on every
row: app permissions are granted per app through shares, so one answer for the whole list would
be wrong for a share-only collaborator.

Cost: two PDP checks per row on the list. Acceptable for a list of this size; if it shows up in
a profile, batch it then.

### 3.2 `POST /api/ws/{workspace}/apps/{id}/duplicate` (A2, A6, A7)

1. Resolve the app in the caller's workspace (404 otherwise, identical body).
2. Require `app:write` on the source object and workspace `app:write`.
3. Refuse a trashed source: 409 `Trashed`.
4. Read the source draft (source + layout) from the object store at its current revision.
5. Create a new app through the same path `create` uses — name `Copy of <name>` (truncated to
   the existing name limit), the source's description and `folder_id`, owner = caller — with
   that source and layout as its revision-1 draft.
6. Emit `app:duplicate` (`{ "from": <source id> }`). Return 201 `AppDetail`.

### 3.3 `DELETE /api/ws/{workspace}/apps/{id}` (A1, A4, A5)

1. Resolve the app in the caller's workspace (404 otherwise).
2. Require `app:manage` on the object.
3. Require the app to be trashed: otherwise 409 with a body saying to move it to Trash first.
4. In one transaction:
   - delete its `analytics.bookmarks` rows (`entity_type = 'app'`; polymorphic, no FK);
   - delete its `access_grants` rows (`scope_type = 'object'`, `scope_id = <id>`; polymorphic);
   - delete its `tag_assignments` rows (`object_type = 'app'`; polymorphic);
   - delete the `analytics.apps` row. Drafts, versions, sessions, grants and pod tokens cascade
     (migrations 0161, 0163, 0165).
5. After commit, delete the object-store prefix `…/{workspace}/{app}/` (`delete_prefix`).
6. Emit `app:delete`. Return 204.

**Order matters.** Rows go first, objects second: a failure between them leaves orphan objects
that nothing references, never rows that point at missing objects. The object delete is
best-effort — its failure is logged and does not fail a request whose rows are already gone.

**Running pods.** Deleting the app cascades its pod tokens and grants, so a run pod for it can
no longer call the pod API and its viewers' requests are refused at the proxy; the idle reaper
then removes the pod. The delete does not reach into the launcher to stop pods itself.

**Things to verify, not assume** (each is a test in the plan):

- the `app_versions` append-only trigger lets the cascade through, including the
  `restored_from` self-reference between versions of the same app;
- an app id from another workspace is 404, and its rows and objects are untouched;
- the prefix handed to `delete_prefix` is exactly one app's prefix. A neighbouring app's
  objects must survive.

Destructive tests run against `MemStore` and the per-process test database only — never a
shared bucket.

## 4. Frontend

### 4.1 Page

`Apps.tsx` is rebuilt on the scaffold `Dashboards.tsx` uses:

- **Header:** title `Apps` (`Trash` in the trash view); action `+ App`, shown when the caller
  can create.
- **Toolbar:** Bookmarks filter · Bulk select · Trash (view switch, always present) · New folder.
- **Search** by name; **summary strip** (apps, folders).
- **Table:** Name · Status · Updated, sortable. Folder sections with collapse, drag-to-folder
  and folder rename/delete/share, exactly as Dashboards (`groupByFolder`,
  `showFolderSections`, `FolderHeaderRow`).
- **Row:** bookmark star · name · status badge (`Draft only` / `Published vN`).
  The name links to the published app when there is one, else to the editor when the caller
  can write, else it is plain text.

### 4.2 Row menu (`AppActionsMenu`)

| Item | Shown when | Notes |
|---|---|---|
| Open | always | disabled with the reason when nothing is published |
| Edit | `can_write` | |
| Rename… | `can_write` | dialog: name + description, `PATCH` |
| Duplicate | `can_write` | navigates to the copy's editor |
| Move to folder | `can_write` | existing `MoveToFolderItems` |
| Share… | `can_manage` | existing `ShareDialog`, `kind="app"` |
| Move to Trash | `can_write` | toast |

Trash view: **Restore** (`can_write`) and **Delete permanently** (`can_manage`, through
`ConfirmDeleteDialog`). A row with neither renders no menu trigger.

Actions are **withheld, not disabled**, for the reason Dashboards gives: a greyed destructive
control advertises a capability the reader does not have. Open is the exception — it is not a
mutation, and its disabled state explains itself.

### 4.3 Bulk

Bulk select is offered when at least one visible row has `can_write`. Checkboxes appear only on
writable rows, so "Move n to Trash" can never count a row the server will refuse. Actions: move
to folder, move to Trash.

### 4.4 API layer

`frontend/src/api/apps.ts` gains `useApps(ws, { trashed })` and `useAppMutations(ws)` (update,
trash, restore, remove, duplicate, addBookmark, removeBookmark), each invalidating the generated
list key. The client is regenerated with `npm run gen:api` — never bare `orval`.

## 5. Out of scope

- Version history from the list (it stays on the edit page).
- Certification, subscriptions.
- Stopping pods synchronously on delete.

## 6. Known problem this inherits

Sharing a **folder** requires `folder:manage`, which no role holds — a defect that predates this
work and already affects the other lists. The folder Share action is wired here for parity and
will fail the same way until that is fixed separately.

## 7. Verification

- Rust: unit and DB-backed tests for the three backend changes, including the checks in §3.3
  and a permission matrix (reader, share-only writer, workspace admin, other workspace).
- Frontend: component tests for the menu's gating in every capability combination, the trash
  view, bulk selection over mixed-permission rows, and the name link's three targets.
- Live, in a browser against a running stack: every menu action once, as an admin and as a
  share-only collaborator; a permanent delete confirmed by listing the bucket.
