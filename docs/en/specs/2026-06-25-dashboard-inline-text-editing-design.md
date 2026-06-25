# Inline Text/Heading Card Editing (+ `{{variable}}` hint) — Design

**Status:** Approved
**Date:** 2026-06-25
**Scope:** Frontend only (React/TS). No backend / API changes.

## Problem

Compared live with Metabase, egret's dashboard editor edits text/heading cards through
the right-hand settings panel (a `textarea`/`input`), while Metabase edits them **inline on
the canvas** — click the card, type directly. Two gaps from the comparison:

1. **#1 — Inline editing.** egret: add a text/heading card → click it → type in the right
   `CardSettingsPanel` "heading text"/"text body" field. Metabase: type directly in the card
   on the grid. Inline is far more fluid.
2. **#2 — `{{variable}}` interpolation.** Metabase injects filter/parameter values into text &
   heading content and hints at it ("connect widgets to {{variables}}").

**Key finding:** #2 is **already implemented**. `interpolateParams` (`params.ts:310`)
substitutes `{{slug}}` → the parameter's current value, and `DashboardCardView`
(`:62`, `:66`) already applies it to both text and heading cards. The only missing piece for
#2 is **discoverability** — a hint telling users they can use `{{slug}}`.

## Goals

- Text and heading cards are edited **inline on the canvas** in edit mode (type directly in
  the card), showing the **raw** text (literal `{{slug}}`).
- Newly-added text/heading cards **autofocus** their inline editor (add → type immediately).
- The settings panel for text/heading keeps its layout settings (auto-height, move-to-tab)
  but **drops** the now-redundant text field, and **gains a `{{slug}}` hint** listing the
  dashboard's available filter slugs.
- View mode is unchanged: interpolated + markdown-rendered (text) / interpolated styled line
  (heading).

## Non-Goals

- No backend changes; no change to how `interpolateParams` works (it already does `{{slug}}`).
- No "render markdown until focus" toggle — in edit mode the text card is always a raw
  editable `textarea` (simpler; YAGNI). View mode still renders markdown.
- No new parameter types or interpolation syntax.
- Link cards keep their panel-based url/label editing (out of scope).

## Current State (anchors)

- `components/chart/dashboard/HeadingCard.tsx` — presentational:
  `<div className="flex h-full items-center px-1 text-xl font-bold text-foreground">{text || <span className="text-muted-foreground">Heading</span>}</div>`.
- `components/chart/dashboard/TextCard.tsx` — presentational; renders GFM markdown via
  `ReactMarkdown` (XSS-safe), empty → `<div className="h-full p-2" />`.
- `components/chart/dashboard/DashboardCardView.tsx` `:61-68` — for `text`/`heading` it does
  `const text = interpolateParams(asString(card.options.text) ?? "", parameters ?? [], paramValues ?? {})`
  then `wrap(<TextCard text={text} />)` / `wrap(<HeadingCard text={text} />)`. `wrap` is the
  `AutoHeightMeasure` wrapper (active only when `autoHeight`).
- `components/chart/dashboard/CardSettingsPanel.tsx` `:33-43` — the text/heading block:
  ```tsx
  if (card.card_type === "text" || card.card_type === "heading") {
    return (
      <div className="flex flex-col gap-2 p-3">
        <div className="text-sm font-medium">{card.card_type === "heading" ? "Heading" : "Text"}</div>
        <textarea aria-label={card.card_type === "heading" ? "heading text" : "text body"} className={`${inp} h-24 py-1`}
          value={(card.options.text as string) ?? ""} onChange={(e) => onUpdateOptions({ text: e.target.value })} />
        {autoHeightRow}
        {moveToTab}
      </div>
    );
  }
  ```
- `routes/explore/DashboardView.tsx` — `addVirtualCard(card_type, options)` creates the draft
  card with id `new-${card_type}-${seq}`; `updateCardOptions(cardId, patch)` patches draft
  options; the card map renders `<DashboardCardView ... editing={editing} .../>`. Click-to-
  select (round 2) opens the card's settings panel via `openPanel({kind:"card"})` — this is
  UNCHANGED; the inline editor coexists with it.

## Design

### A. Inline editor in `HeadingCard` / `TextCard`

Both gain props `editing?: boolean`, `autoFocus?: boolean`, `onChange?: (text: string) => void`.
When `editing && onChange`, render an inline editor bound to the **raw** text; otherwise the
existing display.

`HeadingCard`:
```tsx
interface Props { text: string; editing?: boolean; autoFocus?: boolean; onChange?: (text: string) => void }
export function HeadingCard({ text, editing, autoFocus, onChange }: Props) {
  if (editing && onChange) {
    return (
      <input
        aria-label="heading text"
        className="dash-nodrag h-full w-full bg-transparent px-1 text-xl font-bold text-foreground placeholder:font-normal placeholder:text-muted-foreground focus:outline-none"
        placeholder="Heading"
        autoFocus={autoFocus}
        value={text}
        onChange={(e) => onChange(e.target.value)}
      />
    );
  }
  return (
    <div className="flex h-full items-center px-1 text-xl font-bold text-foreground">
      {text || <span className="text-muted-foreground">Heading</span>}
    </div>
  );
}
```

`TextCard`:
```tsx
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

interface Props { text: string; editing?: boolean; autoFocus?: boolean; onChange?: (text: string) => void }
export function TextCard({ text, editing, autoFocus, onChange }: Props) {
  if (editing && onChange) {
    return (
      <textarea
        aria-label="text body"
        className="dash-nodrag h-full w-full resize-none bg-transparent p-2 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none"
        placeholder="Write text…  ({{filter}} inserts a filter value)"
        autoFocus={autoFocus}
        value={text}
        onChange={(e) => onChange(e.target.value)}
      />
    );
  }
  if (!text.trim()) return <div className="h-full p-2" />;
  return (
    <div className="dash-markdown h-full overflow-auto p-2 text-sm [&_h1]:mb-1 [&_h1]:text-lg [&_h1]:font-semibold [&_h2]:mb-1 [&_h2]:text-base [&_h2]:font-semibold [&_ul]:list-disc [&_ul]:pl-5 [&_ol]:list-decimal [&_ol]:pl-5 [&_a]:text-primary [&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
    </div>
  );
}
```

`dash-nodrag` keeps text selection inside the editor from triggering react-grid-layout drag.
(Dragging the card still uses the top `dash-draghandle` strip.) The editor's click bubbles to
the card wrapper, so click-to-select still opens the settings panel — intended (inline edit +
panel coexist). No `stopPropagation`.

### B. `DashboardCardView` — raw in edit, interpolated in view

Add props `onEditText?: (text: string) => void` and `autoFocusText?: boolean`. For text and
heading, show **raw** text while editing and **interpolated** text otherwise:
```tsx
  if (card.card_type === "text") {
    const raw = asString(card.options.text) ?? "";
    const display = editing ? raw : interpolateParams(raw, parameters ?? [], paramValues ?? {});
    return wrap(<TextCard text={display} editing={editing} autoFocus={autoFocusText} onChange={onEditText} />);
  }
  if (card.card_type === "heading") {
    const raw = asString(card.options.text) ?? "";
    const display = editing ? raw : interpolateParams(raw, parameters ?? [], paramValues ?? {});
    return wrap(<HeadingCard text={display} editing={editing} autoFocus={autoFocusText} onChange={onEditText} />);
  }
```
(`onEditText` is only passed for text/heading in edit mode — see C. When `editing` is false,
`onChange` is undefined so the display branch renders.)

### C. `DashboardView` — thread the editor callback + autofocus the new card

- Add state `const [newCardId, setNewCardId] = useState<string | null>(null);`.
- In `addVirtualCard`, compute the id up front so it can be tracked, and set `newCardId` for
  text/heading so the freshly-added card autofocuses its inline editor. (Keep the existing
  position/size logic; only hoist the id and add `setNewCardId`.) For example:
  ```tsx
  function addVirtualCard(card_type: "text" | "heading" | "link", options: Record<string, unknown>) {
    const id = `new-${card_type}-${cardSeqRef.current++}`;
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      const { w, h } = sizeForViz(card_type).default;
      const pos = getPositionForNewDashCard(base, w, h);
      const newCard: DraftCard = {
        id, chart_id: null, card_type, grid_col: pos.grid_col, grid_row: pos.grid_row,
        size_x: w, size_y: h, options, tab_id: null, tabKey: activeTab?.key ?? null,
      };
      return [...base, newCard];
    });
    if (card_type === "text" || card_type === "heading") setNewCardId(id);
  }
  ```
- In the card map's `<DashboardCardView .../>`, pass:
  ```tsx
  onEditText={editing && (c.card_type === "text" || c.card_type === "heading") ? (t) => updateCardOptions(c.id, { text: t }) : undefined}
  autoFocusText={c.id === newCardId}
  ```
- Clear `newCardId` when leaving edit mode (in `cancelEdit` and the `onSave` success path and
  the `[id]` route effect): `setNewCardId(null);` — keeps it from autofocusing stale ids.

### D. `CardSettingsPanel` — drop the text field, add the `{{slug}}` hint

Replace the text/heading block (`:33-43`) with one that removes the `<textarea>` and adds the
hint listing the dashboard's filter slugs:
```tsx
  if (card.card_type === "text" || card.card_type === "heading") {
    const slugs = parameters.map((p) => p.slug);
    return (
      <div className="flex flex-col gap-2 p-3">
        <div className="text-sm font-medium">{card.card_type === "heading" ? "Heading" : "Text"}</div>
        <p className="text-xs text-muted-foreground">
          Edit the {card.card_type} on the card. Insert a filter value with <code className="rounded bg-muted px-1">{"{{slug}}"}</code>.
          {slugs.length > 0 && (
            <> Available: {slugs.map((s) => <code key={s} className="mr-1 rounded bg-muted px-1">{`{{${s}}}`}</code>)}</>
          )}
        </p>
        {autoHeightRow}
        {moveToTab}
      </div>
    );
  }
```
(`parameters` is already a prop of `CardSettingsPanel`. `autoHeightRow` and `moveToTab` are the
existing computed rows.)

## Testing

**Unit (Vitest):**
- `HeadingCard` / `TextCard` (colocated card test): with `editing` + `onChange`, renders an
  editable control bound to `text` and fires `onChange` on input; without `editing`, renders
  the display (heading line / rendered markdown). For `TextCard` editing, the raw text is shown
  in the `textarea` (markdown NOT rendered).
- `DashboardView`: the two restored card-add tests in `DashboardView.test.tsx` (`adds a text
  card and edits its body`, `adds a heading card`) currently open the settings panel and edit
  via `getByLabelText(/text body|heading text/i)`. **Update them** to edit **inline**: after
  adding (via the `T▾` dropdown), type into the on-canvas editor (the heading `<input>` /
  text `<textarea>`) and assert the draft updated (e.g. on Save the payload carries the typed
  text, or the editor reflects the value). The inline editor has no aria-label by default —
  add `aria-label="heading text"` / `aria-label="text body"` to the inline `input`/`textarea`
  (matching the old panel labels) so tests and screen readers can target them. (Update the
  component code in A to include these aria-labels.)
- `DashboardView.editsidebar.test.tsx` "clicking a card opens the card panel" asserts
  `getByLabelText(/heading text|text body/)`. Since the panel no longer has that field, update
  it to assert the panel opened via a still-present control — the hint text (e.g.
  `getByText(/Insert a filter value/i)`) or `getByLabelText("auto height")`. Keep the
  assertion meaningful (panel opened).
- A view-mode interpolation test (add if not present): a text card with `"Hi {{q}}"` and a
  parameter `q` whose value is set renders "Hi <value>" in view mode (the existing
  `interpolateParams` behavior, now verified through the card).
- Autofocus: adding a heading focuses its inline `input` (assert `document.activeElement` is
  the heading input, or the input has focus). jsdom honors `autoFocus` on mount.

**Live (Playwright, throwaway dashboard, 0 console errors):** add a heading → it autofocuses;
type directly on the card; add a text card → type markdown inline; create a filter, put
`{{slug}}` in a text card, set the filter value in view mode → the card shows the value; the
settings panel for text/heading shows the hint + available slugs and no text field; Save →
reload → content persists. Delete the throwaway dashboard + scratch shots afterward.

## Files

**Modified:**
- `frontend/src/components/chart/dashboard/HeadingCard.tsx` — inline `<input>` editor + props.
- `frontend/src/components/chart/dashboard/TextCard.tsx` — inline `<textarea>` editor + props.
- `frontend/src/components/chart/dashboard/DashboardCardView.tsx` — raw-in-edit / interpolated-
  in-view; pass `editing`/`onEditText`/`autoFocusText` to text & heading.
- `frontend/src/components/chart/dashboard/CardSettingsPanel.tsx` — drop the text field; add the
  `{{slug}}` hint.
- `frontend/src/routes/explore/DashboardView.tsx` — `newCardId` state + autofocus; thread
  `onEditText`/`autoFocusText`; clear `newCardId` on edit exit.

**Backend:** none.

## Risks / Notes

- The inline `<input>`/`<textarea>` must bind to **raw** `options.text` (not interpolated), so
  the user edits literal `{{slug}}`. `DashboardCardView` passes raw only when `editing`.
- Give the inline editors `aria-label="heading text"` / `aria-label="text body"` (reusing the
  old panel labels) so existing tests/screen readers can target them.
- `dash-nodrag` on the editors prevents text-selection from starting a grid drag.
- Auto-height: in edit mode the editor fills `h-full`; auto-height measurement stays a
  view-mode concern (unchanged). Don't try to grow the textarea with content in this iteration.
- OKLCH tokens only; no literal colors.
