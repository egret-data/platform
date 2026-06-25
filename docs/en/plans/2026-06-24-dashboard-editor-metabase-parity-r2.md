# Dashboard Editor — Metabase Parity Round 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make egret's dashboard editor look/feel like Metabase — icon toolbar on the title row, Cancel/Save in the banner, combined `T▾`, on-demand sidebar, per-tab `▾` menu, inline name editing, clearer section previews, and click-to-select cards.

**Architecture:** Frontend-only React/TS. `DashboardEditToolbar` becomes a compact icon group (with a `T▾` dropdown and a redrawn section preview) placed on the title row; `DashboardView` moves Cancel/Save into the edit banner, edits the dashboard name inline (`draftName`), drives the right sidebar off `editPanel` (no auto-open), and selects a card on click. `DashboardTabBar` gains a per-tab dropdown menu; `CardEditToolbar` drops the redundant gear.

**Tech Stack:** React 18, TypeScript, Vite, Vitest, Tailwind v4 (OKLCH tokens), shadcn `dropdown-menu`, `lucide-react`.

**Conventions:**
- All commands run from `egret/frontend/`.
- Full sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- Git identity is already configured — never override the author.
- End every commit message with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Branch `feat/dashboard-edit-metabase-polish` (already checked out; round-1 work is here).
- The git repo root is `egret/`, so `git add` paths need the `frontend/` prefix.
- Never touch the user's `a1` dashboard; live-verify on a throwaway dashboard and delete it after.

**Spec:** `docs/en/specs/2026-06-24-dashboard-editor-metabase-parity-r2-design.md`

---

## File Structure

**Modified:**
- `frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx` — icon-only group, `T▾` dropdown, redrawn `SectionLayoutPreview`.
- `frontend/src/components/chart/dashboard/CardEditToolbar.tsx` — remove the gear (Settings) button + prop.
- `frontend/src/components/chart/dashboard/DashboardTabBar.tsx` — per-tab `▾` dropdown menu.
- `frontend/src/components/chart/dashboard/EditSidebar.tsx` — header close (hide) + back (to add); drop collapse.
- `frontend/src/routes/explore/DashboardView.tsx` — banner Cancel/Save, title-row icon toolbar + inline name, on-demand sidebar, click-to-select.

Tasks touching `DashboardView.tsx` (3, 4, 5) run sequentially; each commits independently.

---

## Task 1: DashboardEditToolbar → icon-only + `T▾` + redrawn section preview

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx`

The component keeps the SAME `Props` (`onAddChart/onAddText/onAddHeading/onAddLink/onAddSection/onAddFilter`) so `DashboardView` need not change for this task. It changes from a full-width labeled strip to a compact inline icon group: `+`, `T▾` (dropdown: Heading/Text), `🔗`, `▦` (Section dropdown), `⏚`. The section preview is redrawn so each card reads as a separate block.

- [ ] **Step 1: Replace the component file**

Replace the ENTIRE contents of `frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx` with:

```tsx
import { Plus, Type, Link2, LayoutGrid, Filter, ChevronDown } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "@/components/ui/dropdown-menu";
import { layoutOptions, type SectionLayout } from "./dashboard-sections";

interface Props {
  onAddChart: () => void;
  onAddText: () => void;
  onAddHeading: () => void;
  onAddLink: () => void;
  onAddSection: (layout: SectionLayout) => void;
  onAddFilter: () => void;
}

const ICON = "inline-flex size-8 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground";

// Mini layout sketch with visible gaps so each card reads as a separate block.
const PREVIEW_COLS = 24;
const PREVIEW_WIDTH = 64; // px
const PREVIEW_ROW_PX = 2.4; // px per grid row
const COL_PX = PREVIEW_WIDTH / PREVIEW_COLS;
const GAP = 1; // px gap inset between blocks

function SectionLayoutPreview({ layout }: { layout: SectionLayout }) {
  const cards = layout.build({ col: 0, row: 0 });
  const rows = Math.max(1, ...cards.map((c) => c.grid_row + c.size_y));
  return (
    <div className="relative shrink-0 overflow-hidden rounded border border-border bg-card" style={{ width: PREVIEW_WIDTH, height: rows * PREVIEW_ROW_PX }}>
      {cards.map((c, i) => (
        <div
          key={i}
          className={`absolute rounded-[1px] ${c.card_type === "heading" ? "bg-foreground/30" : "bg-primary/70"}`}
          style={{
            left: c.grid_col * COL_PX + GAP / 2,
            top: c.grid_row * PREVIEW_ROW_PX + GAP / 2,
            width: Math.max(1, c.size_x * COL_PX - GAP),
            height: Math.max(1, c.size_y * PREVIEW_ROW_PX - GAP),
          }}
        />
      ))}
    </div>
  );
}

/** Edit-mode add controls as a compact icon group (placed on the title row). */
export function DashboardEditToolbar({ onAddChart, onAddText, onAddHeading, onAddLink, onAddSection, onAddFilter }: Props) {
  return (
    <div className="flex items-center gap-1">
      <button type="button" className={ICON} aria-label="add chart" title="Add a chart" onClick={onAddChart}>
        <Plus className="h-4 w-4" />
      </button>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" className={ICON} aria-label="add text or heading" title="Add a heading or text">
            <Type className="h-4 w-4" />
            <ChevronDown className="h-3 w-3" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onClick={onAddHeading}>Heading</DropdownMenuItem>
          <DropdownMenuItem onClick={onAddText}>Text</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <button type="button" className={ICON} aria-label="add link" title="Add a link" onClick={onAddLink}>
        <Link2 className="h-4 w-4" />
      </button>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" className={ICON} aria-label="add section" title="Add a section">
            <LayoutGrid className="h-4 w-4" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {layoutOptions.map((layout) => (
            <DropdownMenuItem key={layout.id} className="gap-2" onClick={() => onAddSection(layout)}>
              <SectionLayoutPreview layout={layout} />
              <span>{layout.label}</span>
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>

      <button type="button" className={ICON} aria-label="add filter" title="Add a filter" onClick={onAddFilter}>
        <Filter className="h-4 w-4" />
      </button>
    </div>
  );
}
```

- [ ] **Step 2: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS. (The component is still rendered by `DashboardView` in its current spot; it now shows as a small icon group there until Task 3 moves it — that's fine.)

- [ ] **Step 3: Run the suite**

Run: `npx vitest run`
Expected: PASS. If any existing test asserted the old labeled buttons (e.g. queried a button named exactly `Chart`/`Text`/`Heading`/`Link`/`Section`/`Filter` text), it belongs to `DashboardView.test.tsx` and is handled in Task 3. If a failure appears here that is purely about those text labels, note it and proceed — Task 3 fixes those assertions. Do NOT weaken assertions in this task.

If the failure is anything OTHER than those label lookups, STOP and report.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx
git commit -m "feat(dashboard): icon-only edit toolbar with T-dropdown and clearer section previews

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Remove the gear (Settings) from CardEditToolbar

**Files:**
- Modify: `frontend/src/components/chart/dashboard/CardEditToolbar.tsx`
- Test: any colocated CardEditToolbar test (search `__tests__` for `card settings` / `CardEditToolbar`).

Clicking a card will open its settings (Task 5), so the gear is redundant.

- [ ] **Step 1: Edit the component**

In `frontend/src/components/chart/dashboard/CardEditToolbar.tsx`:

(a) Remove `Settings` from the import line. It currently is:
```tsx
import { Settings, BarChart3, Copy, RefreshCw, Trash2 } from "lucide-react";
```
Change to:
```tsx
import { BarChart3, Copy, RefreshCw, Trash2 } from "lucide-react";
```

(b) Remove `onSettings` from the `Props` interface (the line `onSettings: () => void;`).

(c) Remove `onSettings` from the destructured params: change
```tsx
export function CardEditToolbar({ cardType, onSettings, onViz, onDuplicate, onReplace, onRemove }: Props) {
```
to:
```tsx
export function CardEditToolbar({ cardType, onViz, onDuplicate, onReplace, onRemove }: Props) {
```

(d) Delete the Settings button line:
```tsx
      <button type="button" aria-label="card settings" title="Settings" className={btn} onClick={onSettings}><Settings className="h-4 w-4" /></button>
```

Leave the viz / duplicate / replace / remove buttons unchanged.

- [ ] **Step 2: Update DashboardView's usage (prevent a TS error)**

In `frontend/src/routes/explore/DashboardView.tsx`, the `CardEditToolbar` usage passes `onSettings`. Remove that one prop line:
```tsx
                    onSettings={() => openPanel({ kind: "card", cardId: c.id })}
```
(Leave `onViz`, `onDuplicate`, `onReplace`, `onRemove` intact. Clicking the card replaces the gear; that click handler is added in Task 5.)

- [ ] **Step 3: Update any CardEditToolbar test**

Search: `npx vitest run` and read failures. If a test asserts the `card settings` button exists, remove that assertion (the button is intentionally gone). Do not weaken unrelated assertions.

- [ ] **Step 4: Typecheck + build + test**

Run: `npm run typecheck && npm run build && npx vitest run`
Expected: PASS (after adapting any gear-button assertion).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/dashboard/CardEditToolbar.tsx frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): drop redundant card settings gear (click selects the card)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: DashboardView chrome — banner Cancel/Save, title-row icon toolbar, inline name

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`

Read the file first; line numbers below are approximate — match by the quoted text.

- [ ] **Step 1: Add `draftName` state**

Near the other draft state (after `const [draftTabs, setDraftTabs] = useState<DraftTab[] | null>(null);`), add:
```tsx
  const [draftName, setDraftName] = useState("");
```

- [ ] **Step 2: Seed/clear `draftName` in startEdit/cancelEdit**

In `startEdit`, after `setDraftTabs(dt);` add:
```tsx
    setDraftName(dashboard.name);
```
In `cancelEdit`, append `setDraftName("");` so the line becomes:
```tsx
  function cancelEdit() { setEditing(false); setDraftCards(null); setDraftParameters(null); setDraftTabs(null); setEditPanel(null); setDraftName(""); }
```

- [ ] **Step 3: Save the edited name**

In `onSave`, the `update.mutateAsync` call currently passes `name: dashboard.name`. Change that one property to:
```tsx
name: draftName.trim() || dashboard.name,
```
(Leave `saveDescription` — a separate function — using `name: dashboard.name`; it is a view-mode description-only save and must not pick up the draft.)

- [ ] **Step 4: Banner gets Cancel/Save**

Replace the edit-banner block:
```tsx
      {editing && (
        <div className="-mx-4 -mt-4 mb-4 flex items-center gap-2 bg-primary/90 px-4 py-2 text-sm font-medium text-primary-foreground">
          <Pencil className="h-4 w-4" /> You're editing this dashboard
        </div>
      )}
```
with:
```tsx
      {editing && (
        <div className="-mx-4 -mt-4 mb-4 flex items-center justify-between gap-2 bg-primary/90 px-4 py-2 text-sm font-medium text-primary-foreground">
          <span className="flex items-center gap-2"><Pencil className="h-4 w-4" /> You're editing this dashboard</span>
          <span className="flex items-center gap-2">
            <Button size="sm" variant="ghost" className="text-primary-foreground hover:bg-white/15 hover:text-primary-foreground" onClick={cancelEdit}>Cancel</Button>
            <Button size="sm" variant="secondary" onClick={onSave} disabled={update.isPending}>Save</Button>
          </span>
        </div>
      )}
```

- [ ] **Step 5: Remove the standalone toolbar block**

Delete the standalone `DashboardEditToolbar` block (it moves to the title row in Step 6):
```tsx
      {editing && (
        <DashboardEditToolbar
          onAddChart={() => openPanel({ kind: "add" })}
          onAddText={() => addVirtualCard("text", { text: "" })}
          onAddHeading={() => addVirtualCard("heading", { text: "" })}
          onAddLink={() => addVirtualCard("link", { url: "", label: "" })}
          onAddSection={addSection}
          onAddFilter={addFilter}
        />
      )}
```

- [ ] **Step 6: Title row — inline name + toolbar (edit) / Edit (view)**

The title row currently is:
```tsx
      <div className="mb-3 flex items-center justify-between border-b pb-3">
        <h1 className="text-xl font-bold">{dash.name}</h1>
        <div className="flex gap-2">
          {!editing && (
            <>
```
Change the `<h1>` line and the wrapper `<div className="flex gap-2">` opening:
- Replace `<h1 className="text-xl font-bold">{dash.name}</h1>` with:
```tsx
        {editing ? (
          <input aria-label="dashboard name" value={draftName} onChange={(e) => setDraftName(e.target.value)}
            className="min-w-0 flex-1 rounded border border-transparent bg-transparent text-xl font-bold focus:border-border focus:bg-background focus:px-2 focus:outline-none" />
        ) : (
          <h1 className="text-xl font-bold">{dash.name}</h1>
        )}
```
- Change `<div className="flex gap-2">` to `<div className="flex items-center gap-2">` (so the icon toolbar aligns vertically).

Then replace the editing branch of that right-side group:
```tsx
          {editing ? (
            <>
              <Button size="sm" variant="ghost" onClick={cancelEdit}>Cancel</Button>
              <Button size="sm" onClick={onSave} disabled={update.isPending}>Save</Button>
            </>
          ) : (
            <Button size="sm" variant="outline" onClick={startEdit}>Edit</Button>
          )}
```
with:
```tsx
          {editing ? (
            <DashboardEditToolbar
              onAddChart={() => openPanel({ kind: "add" })}
              onAddText={() => addVirtualCard("text", { text: "" })}
              onAddHeading={() => addVirtualCard("heading", { text: "" })}
              onAddLink={() => addVirtualCard("link", { url: "", label: "" })}
              onAddSection={addSection}
              onAddFilter={addFilter}
            />
          ) : (
            <Button size="sm" variant="outline" onClick={startEdit}>Edit</Button>
          )}
```
(The `{!editing && (<>…bookmark/refresh/fullscreen/info…</>)}` block in between stays exactly as is.)

- [ ] **Step 7: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS. `Button` and `DashboardEditToolbar` are already imported. `Pencil` is still used in the banner + empty state.

- [ ] **Step 8: Fix affected tests**

Run: `npx vitest run`. Adapt failures honestly (do not gut):
- `DashboardView.test.tsx`: the add controls are now icon buttons with these `aria-label`s — `add chart`, `add text or heading` (a `T▾` dropdown; its `Heading`/`Text` items are inside a Radix menu — see note), `add link`, `add section`, `add filter`. Update any query that clicked a text-labeled `Text`/`Heading`/`Link`/`Filter`/`Chart` button to use the new aria-labels. For the `add text`/`add heading` flows that previously clicked separate buttons: those now live behind the `T▾` dropdown. If opening a Radix dropdown is unreliable in jsdom for this test, assert instead that the `add text or heading` trigger exists and move the click-through verification to the live Playwright pass (Task 7). Keep assertions that verify a real outcome.
- A name-edit test should be ADDED here (Step 9).
- If Cancel/Save lookups break because they moved into the banner, they are still buttons named `Cancel`/`Save` — `getByRole("button", { name: "Save" })` still works.

- [ ] **Step 9: Add a name round-trip test**

In `frontend/src/routes/explore/__tests__/DashboardView.test.tsx` (or the existing save test), add a test that enters edit mode, changes the `dashboard name` input, clicks Save, and asserts the `update` mutation was called with the new `name`. Use the file's existing render/mock harness. Concretely (adapt imports/helpers to the file's existing pattern):

```tsx
it("saves the edited dashboard name", async () => {
  const { update } = setup(); // use the file's existing setup that exposes the update mock
  await user.click(screen.getByRole("button", { name: "Edit" }));
  const nameInput = screen.getByLabelText("dashboard name");
  await user.clear(nameInput);
  await user.type(nameInput, "Renamed Dash");
  await user.click(screen.getByRole("button", { name: "Save" }));
  expect(update.mutateAsync).toHaveBeenCalledWith(expect.objectContaining({ name: "Renamed Dash" }));
});
```
If the file's harness differs (e.g. the update mock is named differently), match the existing pattern used by the current width/save test in the same file.

- [ ] **Step 10: Typecheck + build + test**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all PASS.

- [ ] **Step 11: Commit**

```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/routes/explore/__tests__/
git commit -m "feat(dashboard): banner Cancel/Save, title-row icon toolbar, inline name edit

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: On-demand sidebar (driven by `editPanel`)

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Modify: `frontend/src/components/chart/dashboard/EditSidebar.tsx`

- [ ] **Step 1: EditSidebar — close (hide) + back (to add); drop collapse**

In `frontend/src/components/chart/dashboard/EditSidebar.tsx`:

(a) Change the import `import { PanelLeftClose } from "lucide-react";` to:
```tsx
import { X } from "lucide-react";
```

(b) In `Props`, replace `onCollapse: () => void;` with `onBack: () => void;`. (Keep `onClose: () => void;`.)

(c) Replace the header block:
```tsx
      <div className="flex items-center justify-between border-b px-2 py-1.5">
        {onPanel
          ? <button type="button" aria-label="back to add" className="text-xs text-muted-foreground hover:text-foreground" onClick={props.onClose}>‹ Add cards</button>
          : <span className="px-1 text-xs font-medium text-muted-foreground">Add cards</span>}
        <button type="button" aria-label="collapse sidebar" title="Hide panel" className="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground" onClick={props.onCollapse}><PanelLeftClose className="h-4 w-4" /></button>
      </div>
```
with:
```tsx
      <div className="flex items-center justify-between border-b px-2 py-1.5">
        {onPanel
          ? <button type="button" aria-label="back to add" className="text-xs text-muted-foreground hover:text-foreground" onClick={props.onBack}>‹ Add cards</button>
          : <span className="px-1 text-xs font-medium text-muted-foreground">Add cards</span>}
        <button type="button" aria-label="close panel" title="Close" className="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground" onClick={props.onClose}><X className="h-4 w-4" /></button>
      </div>
```

- [ ] **Step 2: DashboardView — remove `sidebarCollapsed`, simplify `openPanel`**

(a) Delete the state line:
```tsx
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
```
(b) Replace `openPanel`:
```tsx
  // Opening any settings panel must reveal the sidebar even if the user collapsed it.
  const openPanel = useCallback((p: EditPanel) => { setEditPanel(p); setSidebarCollapsed(false); }, []);
```
with:
```tsx
  const openPanel = useCallback((p: EditPanel) => { setEditPanel(p); }, []);
```
(c) Remove the now-unused `PanelLeftOpen` import at the top of the file (the import line `import { Info, Pencil, Star, PanelLeftOpen } from "lucide-react";` → `import { Info, Pencil, Star } from "lucide-react";`).

- [ ] **Step 3: DashboardView — sidebar renders only when `editPanel != null`**

Replace the sidebar block:
```tsx
        {editing && (sidebarCollapsed ? (
          <div className="sticky top-4 self-start">
            <button type="button" aria-label="expand sidebar" title="Show panel" className="inline-flex size-9 items-center justify-center rounded-lg border bg-card text-muted-foreground hover:bg-accent hover:text-foreground" onClick={() => setSidebarCollapsed(false)}>
              <PanelLeftOpen className="h-4 w-4" />
            </button>
          </div>
        ) : (
          <EditSidebar
            panel={editPanel}
            charts={charts ?? []}
            parameters={parameters}
            cardColumns={cardColumns}
            vizForPanel={vizForPanel}
            onChangeVizOverride={(next) => { if (editPanel?.kind === "viz") updateCardOptions(editPanel.cardId, { vizOverride: next }); }}
            tabs={tabs}
            cardForPanel={editPanel?.kind === "card" ? (cards as DraftCard[]).find((c) => c.id === editPanel.cardId) : undefined}
            chartForCard={editPanel?.kind === "card" ? chartById.get((cards as DraftCard[]).find((c) => c.id === editPanel.cardId)?.chart_id ?? "") : undefined}
            paramForPanel={editPanel?.kind === "filter" ? parameters.find((p) => p.id === editPanel.paramId) : undefined}
            onAddChart={(c) => addCard(c)}
            onUpdateCardOptions={(patch) => { if (editPanel?.kind === "card") updateCardOptions(editPanel.cardId, patch); }}
            onMoveCardToTab={(tabKey) => { if (editPanel?.kind === "card") setDraftCards(moveCardToTab((cards as DraftCard[]), editPanel.cardId, tabKey)); }}
            onChangeParam={(next) => setDraftParameters(parameters.map((x) => x.id === next.id ? { ...next, slug: x.slug } : x))}
            onDeleteParam={() => { if (editPanel?.kind === "filter") { const id = editPanel.paramId; setDraftParameters(parameters.filter((x) => x.id !== id)); setEditPanel({ kind: "add" }); } }}
            onClose={() => setEditPanel({ kind: "add" })}
            onCollapse={() => setSidebarCollapsed(true)}
          />
        ))}
```
with (note: `editing && editPanel` guard; `onClose` now hides, new `onBack` returns to add):
```tsx
        {editing && editPanel && (
          <EditSidebar
            panel={editPanel}
            charts={charts ?? []}
            parameters={parameters}
            cardColumns={cardColumns}
            vizForPanel={vizForPanel}
            onChangeVizOverride={(next) => { if (editPanel?.kind === "viz") updateCardOptions(editPanel.cardId, { vizOverride: next }); }}
            tabs={tabs}
            cardForPanel={editPanel?.kind === "card" ? (cards as DraftCard[]).find((c) => c.id === editPanel.cardId) : undefined}
            chartForCard={editPanel?.kind === "card" ? chartById.get((cards as DraftCard[]).find((c) => c.id === editPanel.cardId)?.chart_id ?? "") : undefined}
            paramForPanel={editPanel?.kind === "filter" ? parameters.find((p) => p.id === editPanel.paramId) : undefined}
            onAddChart={(c) => addCard(c)}
            onUpdateCardOptions={(patch) => { if (editPanel?.kind === "card") updateCardOptions(editPanel.cardId, patch); }}
            onMoveCardToTab={(tabKey) => { if (editPanel?.kind === "card") setDraftCards(moveCardToTab((cards as DraftCard[]), editPanel.cardId, tabKey)); }}
            onChangeParam={(next) => setDraftParameters(parameters.map((x) => x.id === next.id ? { ...next, slug: x.slug } : x))}
            onDeleteParam={() => { if (editPanel?.kind === "filter") { const id = editPanel.paramId; setDraftParameters(parameters.filter((x) => x.id !== id)); setEditPanel({ kind: "add" }); } }}
            onClose={() => setEditPanel(null)}
            onBack={() => setEditPanel({ kind: "add" })}
          />
        )}
```

- [ ] **Step 4: DashboardView — don't auto-open the sidebar on edit**

In `startEdit`, change `setEditPanel({ kind: "add" })` to:
```tsx
    setEditPanel(null);
```

- [ ] **Step 5: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS. The flex wrapper `<div className={editing || showInfo ? "flex gap-4" : ""}>` still applies; when `editPanel` is null the grid column simply takes full width (no sidebar sibling). Verify `PanelLeftOpen` is no longer referenced.

- [ ] **Step 6: Fix affected tests**

Run: `npx vitest run`. Likely affected: `DashboardView.editsidebar.test.tsx` and any test that assumed the sidebar is visible immediately on entering edit. Update them so the sidebar is asserted to appear AFTER clicking the `add chart` toolbar button (or opening a card/filter), and to disappear after clicking `close panel`. Keep the assertions meaningful (they should still verify the chart list / panel content).

- [ ] **Step 7: Typecheck + build + test**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all PASS.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/components/chart/dashboard/EditSidebar.tsx frontend/src/routes/explore/__tests__/
git commit -m "feat(dashboard): open the edit sidebar on demand (driven by editPanel)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Click a card to select/configure it

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`

The gear was removed in Task 2; clicking a card now opens its settings panel.

- [ ] **Step 1: Add the click handler to the inner card wrapper**

The inner card wrapper currently opens with:
```tsx
              <div className={`relative h-full overflow-hidden ${
                c.card_type === "heading" ? "bg-dashboard"
                : c.card_type === "placeholder" ? ""
                : "rounded-sm border bg-card"
              } ${editing && editPanel?.kind === "card" && editPanel.cardId === c.id ? "ring-2 ring-primary" : ""}`}>
```
Replace that opening tag with (adds an edit-only `onClick` that selects non-placeholder cards, and a pointer cursor cue):
```tsx
              <div
                onClick={editing && c.card_type !== "placeholder" ? () => openPanel({ kind: "card", cardId: c.id }) : undefined}
                className={`relative h-full overflow-hidden ${editing && c.card_type !== "placeholder" ? "cursor-pointer" : ""} ${
                c.card_type === "heading" ? "bg-dashboard"
                : c.card_type === "placeholder" ? ""
                : "rounded-sm border bg-card"
              } ${editing && editPanel?.kind === "card" && editPanel.cardId === c.id ? "ring-2 ring-primary" : ""}`}>
```

Notes for the implementer:
- The floating `CardEditToolbar` is a sibling OUTSIDE this inner wrapper (it's under the outer `group` div, positioned `-top-3.5`), so clicking a toolbar button (viz/duplicate/replace/remove) does NOT bubble into this `onClick`. Leave the toolbar as is.
- Placeholders keep their own click (`PlaceholderCard onPick`) — the `c.card_type !== "placeholder"` guard ensures the wrapper doesn't also intercept.
- View mode (`!editing`) passes `undefined`, so drill/cross-filter on chart click is unchanged.

- [ ] **Step 2: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS.

- [ ] **Step 3: Add/extend a click-to-select test**

In `frontend/src/routes/explore/__tests__/DashboardView.test.tsx`, add a test: enter edit mode, click a chart card's body, assert the card settings panel opened (e.g. the sidebar now shows a card-settings control such as the `auto height` checkbox or `move card to tab`, depending on what the existing harness renders). Use the file's existing render harness and a seeded chart card. Example shape (adapt selectors to the harness):

```tsx
it("opens card settings when a card is clicked in edit mode", async () => {
  setup(/* a dashboard with at least one chart card */);
  await user.click(screen.getByRole("button", { name: "Edit" }));
  // click the card body (not the floating toolbar)
  await user.click(screen.getByTestId("dashcard-body") /* or the card's container by its chart name */);
  expect(await screen.findByLabelText("auto height")).toBeInTheDocument();
});
```
If there's no stable test id for the card body, select the card container by its rendered chart title and click it. If the harness can't easily reach the card body, keep this verification for the live Playwright pass (Task 7) and instead assert the simpler invariant that the gear button no longer exists (`expect(screen.queryByLabelText("card settings")).toBeNull()`).

- [ ] **Step 4: Typecheck + build + test**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/routes/explore/__tests__/
git commit -m "feat(dashboard): click a card to open its settings

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Per-tab `▾` menu in DashboardTabBar

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardTabBar.tsx`
- Test: `frontend/src/routes/explore/__tests__/DashboardView.tabs.test.tsx` (adapt).

Replace the hover `Copy`/`X` icons with a single `▾` dropdown (Rename / Duplicate / Delete).

- [ ] **Step 1: Edit the component**

In `frontend/src/components/chart/dashboard/DashboardTabBar.tsx`:

(a) Change the import line `import { Copy, X, Plus } from "lucide-react";` to:
```tsx
import { ChevronDown, Plus } from "lucide-react";
```
and add below it:
```tsx
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "@/components/ui/dropdown-menu";
```

(b) Replace the per-tab editing controls block:
```tsx
            {editing && (
              <>
                <button type="button" aria-label={`duplicate tab ${t.name}`} title="Duplicate tab" className="rounded p-0.5 text-muted-foreground opacity-0 transition-opacity hover:bg-accent hover:text-foreground group-hover:opacity-100" onClick={() => onDuplicate(t.key)}><Copy className="h-3.5 w-3.5" /></button>
                <button type="button" aria-label={`delete tab ${t.name}`} title="Delete tab" className="rounded p-0.5 text-destructive opacity-0 transition-opacity hover:bg-accent group-hover:opacity-100 disabled:cursor-not-allowed disabled:opacity-30"
                  disabled={ordered.length < 2} onClick={() => onDelete(t.key)}><X className="h-3.5 w-3.5" /></button>
              </>
            )}
```
with:
```tsx
            {editing && (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button type="button" aria-label={`tab menu ${t.name}`} title="Tab options" className="rounded p-0.5 text-muted-foreground opacity-0 transition-opacity hover:bg-accent hover:text-foreground group-hover:opacity-100"><ChevronDown className="h-3.5 w-3.5" /></button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start">
                  <DropdownMenuItem onClick={() => setRenaming(t.key)}>Rename</DropdownMenuItem>
                  <DropdownMenuItem onClick={() => onDuplicate(t.key)}>Duplicate</DropdownMenuItem>
                  {ordered.length >= 2 && (
                    <DropdownMenuItem className="text-destructive" onClick={() => onDelete(t.key)}>Delete</DropdownMenuItem>
                  )}
                </DropdownMenuContent>
              </DropdownMenu>
            )}
```
(Keep the `onDoubleClick={() => { if (editing) setRenaming(t.key); }}` on the tab button as a shortcut.)

- [ ] **Step 2: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS. Confirm `Copy` and `X` are no longer referenced anywhere in the file.

- [ ] **Step 3: Adapt the tabs test**

Run: `npx vitest run`. In `DashboardView.tabs.test.tsx`, any test that clicked `duplicate tab <name>` / `delete tab <name>` buttons must now open the `tab menu <name>` dropdown and click the `Duplicate` / `Delete` menu item. If opening the Radix menu is unreliable in jsdom for these tests, assert the `tab menu <name>` trigger exists and verify duplicate/delete behavior in the live Playwright pass (Task 7) — but keep at least one assertion that the menu trigger renders per tab in edit mode. Do not delete coverage silently; if you move a check to live, say so in the commit message.

- [ ] **Step 4: Typecheck + build + test**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/dashboard/DashboardTabBar.tsx frontend/src/routes/explore/__tests__/
git commit -m "feat(dashboard): per-tab dropdown menu (rename/duplicate/delete)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Full sweep + live verification + cleanup

**Files:** none (verification only).

- [ ] **Step 1: Full sweep**

Run from `egret/frontend/`:
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
```
Expected: all PASS, 0 lint errors (the 2 pre-existing warnings in the map files are unrelated).

- [ ] **Step 2: Live verification (Playwright, throwaway dashboard, 0 console errors)**

Dev stack is up (frontend `http://localhost:5173`, backend `:8080`). Log in `admin@egret.local` / `Admin12345!`. Create a **throwaway** dashboard (the `+ Dashboard` button on `/explore/eng/dashboards`); never use `a1`. With Metabase (`localhost:3001`, alice@test.com/123123a@) open for side-by-side reference, verify in edit mode:
1. **Banner**: Cancel + Save are in the green "You're editing" banner (right).
2. **Title row**: the add controls are compact icons on the right (`+`, `T▾`, `🔗`, `▦`, `⏚`); the dashboard name is an editable input.
3. **`T▾`**: opens a menu with `Heading` and `Text`; each adds the right card.
4. **Section**: the dropdown previews show real mini-layouts (not solid blocks); inserting a template + filling a placeholder still works.
5. **Sidebar**: not shown on entering edit; `+` opens the chart list; `close` (X) hides it; opening a card/filter shows it.
6. **Tabs**: each tab has a `▾` menu (Rename / Duplicate / Delete); rename/duplicate/delete work; delete disabled with one tab.
7. **Inline name**: edit the name → Save → reload → the new name persists.
8. **Click-to-select**: clicking a card body opens its settings panel (ring highlight); there is no gear button on the hover toolbar.
9. **Console**: `browser_console_messages` (per-navigation) shows 0 errors throughout.

Capture before/after screenshots to `/Users/1pro-l4r8/Documents/github/data-portal/` (parent dir, NOT the repo).

- [ ] **Step 3: Cleanup**

Permanently delete the throwaway dashboard (Trash → Delete permanently → type `delete`). Delete the scratch screenshots from the parent dir. Confirm `git status` shows only intended source/spec/plan changes.

- [ ] **Step 4: Final whole-branch review**

Dispatch the final code reviewer over the round-2 diff (the commits added since the round-1 work), then proceed to `superpowers:finishing-a-development-branch`.

---

## Self-Review notes

- **Spec coverage:** A (banner Cancel/Save + title-row toolbar) → Task 3 Steps 4,6. B (`T▾`) → Task 1. C (section preview) → Task 1. D (on-demand sidebar) → Task 4. E (per-tab menu) → Task 6. F (inline name) → Task 3 Steps 1-3,6,9. G (click-to-select + remove gear) → Tasks 2 + 5. Tests → per-task + Task 7 live. ✓
- **Type consistency:** `DashboardEditToolbar` `Props` unchanged across Tasks 1/3. `EditSidebar` swaps `onCollapse`→`onBack` (Task 4 updates both the component and the single call site). `CardEditToolbar` drops `onSettings` (Task 2 updates component + call site together). `openPanel` signature unchanged. `editPanel` kinds (`add`/`card`/`viz`/`filter`/null) unchanged. ✓
- **Ordering:** Task 2 removes `onSettings` and updates the DashboardView call site in the same commit, so the tree compiles. Task 3's title-row edit and Task 4's sidebar edit touch different regions of DashboardView; sequential commits keep it green. ✓
- **jsdom/Radix caveat:** dropdown menu open-through (T▾, Section, tab `▾`) may be unreliable in jsdom; the plan asserts trigger presence in unit tests and verifies open-through live (Task 7), without dropping meaningful coverage.
