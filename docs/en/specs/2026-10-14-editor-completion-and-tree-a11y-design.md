# Follow-ups: SQL completion scoped to its editor, and tree semantics for the namespaces list

**Status:** design, 2026-10-14.

**Depends on:** W7d (#669), which added the namespaces list and the namespace Files tree; #670, which scoped Monaco's keyboard shortcuts to their editor and left SQL completion as it was.

The frontend rulebook `docs/en/frontend-rules.md` binds every frontend line.

## 1. What was checked in the code

Each premise of the brief was read in the code before anything was written.

| Claim | Where | Holds? |
|---|---|---|
| `SqlEditor` registers a completion provider for the language `sql` on every mount | `frontend/src/components/sqllab/SqlEditor.tsx:137`, `monaco.languages.registerCompletionItemProvider("sql", …)`, disposed on unmount and on a second mount | Yes |
| Monaco keeps providers per language and asks every provider of one score, merging the answers | `monaco-editor` 0.56.0: `registerCompletionItemProvider` registers on `ILanguageFeaturesService.completionProvider` (`esm/vs/editor/standalone/browser/standaloneLanguages.js:448`); `provideSuggestionItems` asks each provider in `registry.orderedGroups(model)` and pushes every answer (`esm/vs/editor/contrib/suggest/browser/suggest.js:185-211`). Two providers registered for `"sql"` have the same score, so both answer for every SQL model | Yes |
| With two SQL Lab tabs open, each editor offers both tabs' schemas | `frontend/src/routes/explore/SqlLab.tsx:61` renders only the ACTIVE tab (`<SqlTab key={active.id} …>`); the other tab's editor is unmounted and its provider disposed. The Data API pages and the dataset page also render one `SqlEditor` at a time | **No, not today.** The defect is latent: it shows the moment two `SqlEditor`s are mounted together (each would answer with its own schema and its own copy of the keywords for both models). The fix is kept because it is cheap and the component is shared |
| Other language-global registrations (`registerHoverProvider` and the like) in `frontend/src` | `grep -rnE "languages\.(register\|set)[A-Za-z]*\(" frontend/src` finds only `SqlEditor.tsx:137`. The flow editor's YAML completion and hover come from `monaco-yaml` (`frontend/src/components/workflows/editor/monacoYaml.ts`): ONE instance for the app, configured per schema ETag and scoped to flow models by `fileMatch: [FLOW_MODEL_GLOB]`. It is per app on purpose (configuring twice stacks an instance that cannot be turned off) and is not per editor | Only `SqlEditor` is per editor |
| The namespaces list has no tree semantics | `frontend/src/routes/workflows/NamespacesList.tsx`: a `ui/table` whose rows are indented by `data-depth` and padding; no `role`, no level, nothing to expand or collapse, and each row's link is its own Tab stop | Yes |
| The namespace Files tree lacks tree semantics | `frontend/src/components/workflows/namespaces/files/FileTree.tsx:225-301`: `role="tree"`, `role="treeitem"` with `aria-level`, `aria-expanded`, `aria-selected`, a roving `tabIndex`, Up/Down, Right, Left, Home/End, Enter/Space, F2, Delete, Shift+F10; tested in `NamespaceFilesTab.test.tsx` | **No.** It already has them. Not changed (§4) |

## 2. SQL completion

**Each provider answers only for its own editor's model.** `SqlEditor`'s provider compares the model Monaco hands it with `editor.getModel()`, read at call time, and returns `{ suggestions: [] }` for any other model. Read at call time rather than captured at mount, so a model swapped under the editor (`@monaco-editor/react`'s `path`) is still answered.

Considered and not taken: one shared provider dispatching on the model URI to a registry. It removes the N-providers-per-language shape, but adds module state that must be kept in step with mounts and unmounts; the per-model check keeps all state inside the component that already disposes it.

Disposal is unchanged: the provider is disposed on unmount and when `onMount` runs again.

**Test** (`frontend/src/components/sqllab/__tests__/SqlEditor.test.tsx`): the fake Monaco records every provider with its language and whether it was disposed, and each fake editor has its own model. Two `SqlEditor`s with different schemas are mounted; asking every live `sql` provider for each model, as Monaco does, gives the first editor's table and not the second's, and the other way round, and each keyword once. A further test disposes on unmount.

## 3. The namespaces list as a treegrid

The list keeps its table (FE-PAGE-4) and becomes a WAI-ARIA **treegrid**, the pattern for a table whose rows form a tree:

- The table takes `role="treegrid"`, an `aria-label` of "Namespaces", and the caption as its description (`aria-describedby`), with a screen-reader-only line saying which keys work.
- Each body row is focusable and carries `aria-level` (depth + 1), `aria-setsize` and `aria-posinset` among its siblings, and `aria-expanded` when it has children. A leaf has no `aria-expanded`.
- Cells are `role="gridcell"`; the header row's cells stay column headers.
- **One Tab stop, roving:** the focused row while it is shown, else the first row. Each row's link takes no Tab stop (`tabIndex={-1}`); the row's Enter follows it.
- Keys on a row: Down and Up move to the next and previous visible row; Home and End to the first and last; Right expands a collapsed row, and on an expanded row moves to its first child; Left collapses an expanded row, and otherwise moves to the parent row; Enter opens the namespace.
- **Collapsing is new.** A row with children shows a chevron button that collapses and expands it, for the mouse; it takes no Tab stop, as the arrows do the same. Everything starts expanded, as today. The folded set lives in memory only: it is not a selection anyone would share (FE-PAGE-2) and not a stored preference. A collapsed row hides every row beneath it; the summary still counts the filtered namespaces.

- A key with Alt, Cmd or Ctrl is left to the browser (Alt/Cmd+Left is Back; Cmd/Ctrl+Enter opens the link elsewhere through the row's link).
- A move focuses the target row at once. If a refetch or filter removes the focused row while focus was in the grid, focus goes to the nearest row above it, else the first.
- The table may take `role="treegrid"` under lint: `jsx-a11y/no-noninteractive-element-to-interactive-role` allows `treegrid` on `<table>` (`frontend/eslint-rules/plugins.js`, FE-A11Y-1).

Row focus mode only: the cells hold numbers, so moving between cells would add keys and nothing to read.

**Tests** (`frontend/src/routes/workflows/__tests__/NamespacesList.test.tsx`): the roles, levels, set sizes, positions and `aria-expanded` on the unsorted fixture; one Tab stop; Down, Up, Home, End; Right expanding then entering, Left collapsing then going to the parent; Enter navigating; the chevron collapsing for the mouse.

## 4. Not changed

- The Files tree already implements the tree pattern (§1). Two small gaps are recorded as follow-ups rather than fixed here: its rows' menu button is an interactive element inside a `treeitem` (it takes no Tab stop, so it is only for the mouse), and it does not set `aria-setsize`/`aria-posinset` (not needed while every sibling is in the DOM, which is the case: a folder lists all its children).
- `monaco-yaml` (§1).

## 5. Proof the tests bite

One mutation per item, each run against its tests and reverted; the results are recorded in the PR:

- Item 1: the model check removed from the provider.
- Item 2: the row's `aria-level` removed, and separately the Left key's move to the parent removed.
