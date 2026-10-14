# Follow-ups to W7d: Monaco shortcuts scoped to their editor, and a KV create that refuses to overwrite

**Status:** implemented, 2026-10-14.

**Depends on:** W7d (#669), which scoped Ctrl/Cmd+S to its editor and left the two items below; W9c (#654), the KV store and its routes.

The frontend rulebook `docs/en/frontend-rules.md` binds every frontend line.

## 1. What was checked in the code

Each premise of the brief was read in the code before anything was written.

| Claim | Where | Holds? |
|---|---|---|
| Standalone `addCommand` registers a keybinding in the shared keybinding service, discards the disposable, and is not scoped to the editor | `monaco-editor` 0.56.0, `esm/vs/editor/standalone/browser/standaloneCodeEditor.js:85-94`: `addDynamicKeybinding(commandId, keybinding, handler, whenExpression)`, its disposable dropped and the command id returned, the `when` only the caller's `context`. `addAction` (`:98-148`): `precondition = editorId == this.getId()` and the keybindings' `when` includes it (`:109`, `:111`), and the `DisposableStore` is returned | Yes |
| The resolver answers the most recently registered matching keybinding | `esm/vs/platform/keybinding/common/keybindingResolver.js:281-290`, `_findCommand` walks the matches from the last | Yes |
| W7d's two Ctrl/Cmd+S sites use `addAction` and dispose on unmount | `frontend/src/components/workflows/editor/CodeTab.tsx:62`, `frontend/src/components/workflows/namespaces/files/NamespaceFileEditor.tsx:164` | Yes |
| Remaining `addCommand` sites | `frontend/src/components/sqllab/SqlEditor.tsx:150` and `frontend/src/routes/explore/dataapi/detail/Query.tsx:760`, both `CtrlCmd \| Enter`. No other `addCommand(` under `frontend/src` (grep) | Yes, exactly two |
| The KV PUT always overwrites | `crates/egret-workflow/src/storage/kv/routes.rs:656` (`overwrite: true`) | Yes |
| The store supports `overwrite: false`, refusing a live latest row with `Conflict` | `crates/egret-workflow/src/storage/kv/store.rs:1145`, under the key's lock, judged by `KvEntry::live` (`store.rs:112`: neither soft-deleted nor expired; `expired` read at `clock_timestamp()` by `wcols!`, `store.rs:131-135`) | Yes |
| The store's "busy" refusal is also a `Conflict` | `store.rs:833` (`busy`) | Yes: the API cannot tell the two apart by variant, so the store change in §3.1 is needed |
| `kvApi.ts` passes headers through the generated fetcher | `frontend/src/components/workflows/namespaces/kv/kvApi.ts` `putKvValue`: `workflowPutKvValue(ws, namespace, key, body, { headers })` | Yes |
| The create drawer already asks before overwriting a key the LIST showed | `KvEditDrawer.tsx` `save()`: `existingKeys.has(form.key)` opens "Overwrite “X”?" | Yes; the gap is a key created after the list was read |

## 2. Item 1: every Monaco shortcut is an editor action

The two `addCommand(CtrlCmd | Enter)` become `editor.addAction({ id, label, keybindings: [CtrlCmd | Enter], run })`, and the disposable is disposed when the component unmounts and when its editor is replaced (a second `onMount`), as W7d did for Ctrl/Cmd+S.

- `SqlEditor`: action `egret.sql.run` ("Run SQL"), its disposable kept with the completion provider and the selection listener the component already disposes. The read-only guard (`onRunRef`) is unchanged.
- `Query.tsx`'s script editor: action `egret.dataapi.preview` ("Run the preview"), held in a ref disposed on unmount and before a remount registers again. The `trashedRef` guard is unchanged.

Why: an `addCommand` keybinding answers in whichever editor has focus, the newest registration wins, and it is never removed. SQL Lab with two tabs, or the Data API page switching between SQL and script, would run the wrong editor's handler, including one that has unmounted.

**Tests** (jsdom, the existing per-suite Monaco stubs extended with `getId` and `addAction`, and `addCommand` REMOVED from the stubs, so a regression throws): two editors' actions each run only their own handler; unmount disposes the action; `SqlEditor`, `Query`, `Statements` and `EndpointCreate` keep their existing Ctrl/Cmd+Enter assertions through the action.

## 3. Item 2: `If-None-Match: *` on `PUT …/kv/{key}`

### 3.1 Backend

- `put_value` reads `If-None-Match`. Absent: `overwrite: true`, as today. Exactly `*` (whitespace trimmed, on every occurrence): `overwrite: false`. Any other value: 400 `If-None-Match on a KV write takes only "*"`, before the body is read. KV has no ETags, so an entity-tag list could only be misread. This is why KV takes `If-None-Match` where nsfiles takes `if_revision`: KV has no revision to compare against, so its only precondition is "absent".
- The store gains `try_set`, which answers a refused non-overwriting write as `Ok(Err(live_entry))`; `set` stays as it was (it maps that to Kestra's `Conflict` message, so the `kv.Set` task with `overwrite: false` is unchanged). The route maps `Err(entry)` to **409** with `code: "kv_exists"` and the message `conflict: key 'X' already exists in namespace 'N' (revision R); nothing was written`. Nothing is staged past the staging object the store forgets, no row is written, no audit is emitted.
- **Liveness is the store's own:** the key exists only if its latest row is live, `KvEntry::live`, judged under the key's lock at `clock_timestamp()`. A key whose latest revision is soft-deleted or expired does NOT exist: a create succeeds and writes the next revision (`max + 1`, so the history is kept), exactly as the GET answers 404 for it.
- 409 rather than RFC 9110's 412: the API's other "the thing changed" refusals are 409 with a `code` (`file_changed`, `draft_changed`), and the client branches on `code`. The busy 409 (no `code`) is unchanged.
- openapi: the header is a documented parameter of the PUT, the 409 row names both cases. `openapi.json` and the generated client are regenerated.

**Tests** (`crates/egret-workflow/tests/kv_api.rs`, the binary's `MemStore`): a create of a new key succeeds; a create of a live key is 409 `kv_exists` and the row table and objects are unchanged; a create over a soft-deleted key and over an expired key succeeds as the next revision; without the header a live key is overwritten as before; a malformed `If-None-Match` is 400 and writes nothing.

### 3.2 Frontend

- `putKvValue` takes `onlyIfAbsent`; when set, it adds the header `If-None-Match: *` (still the generated `workflowPutKvValue` through `customInstance`, FE-DATA-2).
- The create drawer sends it on a create of a key the list did not show. It does not send it on an edit, nor after the user confirmed "Overwrite" for a key the list showed.
- A 409 whose `code` is `kv_exists` opens "A key named X was created meanwhile — overwrite?"; Overwrite resends the same body without the header; Cancel keeps the form as typed (FE-ACTION-3).

**Tests** (`NamespaceKvTab.test.tsx`, the fake server answering 409 `kv_exists` to a PUT carrying the header): the header on create only; not on edit; not after the list-based Overwrite; the 409 dialog, Overwrite resending without the header, Cancel sending nothing more.

## 4. Proof the tests bite

One mutation per item, each run against its tests and reverted:

- Item 1: `SqlEditor` back to `addCommand`.
- Item 2: the route ignores the header (`overwrite: true` always).
