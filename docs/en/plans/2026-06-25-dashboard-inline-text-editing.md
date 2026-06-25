# Inline Text/Heading Card Editing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Edit dashboard text/heading cards inline on the canvas (type directly in the card), autofocus on add, drop the redundant panel text field, and add a `{{slug}}` discoverability hint.

**Architecture:** `HeadingCard`/`TextCard` gain an inline-editor branch (rendered when `editing && onChange`), bound to the **raw** card text. `DashboardCardView` shows raw text while editing and interpolated text in view mode, and threads an `onEditText` callback + `autoFocusText`. `DashboardView` tracks the just-added card to autofocus it and wires `onEditText` to `updateCardOptions`. `CardSettingsPanel` drops the text field and shows the available `{{slug}}` hint. `{{variable}}` interpolation already exists (`interpolateParams`) and is unchanged.

**Tech Stack:** React 18, TypeScript, Vite, Vitest + Testing Library, Tailwind v4 (OKLCH tokens), react-markdown.

**Conventions:**
- All commands run from `egret/frontend/`.
- Full sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- Git identity already configured — never override the author.
- End every commit message with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Branch `feat/dashboard-inline-text-editing` (already checked out). Git repo root is `egret/`, so `git add` paths need the `frontend/` prefix.
- Never touch the user's `a1` dashboard; live-verify on a throwaway dashboard and delete it after.

**Spec:** `docs/en/specs/2026-06-25-dashboard-inline-text-editing-design.md`

---

## File Structure

**Modified:**
- `frontend/src/components/chart/dashboard/HeadingCard.tsx` — inline `<input>` editor branch + props.
- `frontend/src/components/chart/dashboard/TextCard.tsx` — inline `<textarea>` editor branch + props.
- `frontend/src/components/chart/dashboard/DashboardCardView.tsx` — raw-in-edit / interpolated-in-view; pass `editing`/`onEditText`/`autoFocusText` to text & heading cards.
- `frontend/src/components/chart/dashboard/CardSettingsPanel.tsx` — drop the text/heading text field; add the `{{slug}}` hint.
- `frontend/src/routes/explore/DashboardView.tsx` — `newCardId` state + autofocus; thread `onEditText`/`autoFocusText`; clear `newCardId` on edit exit.
- Tests: `frontend/src/components/chart/dashboard/__tests__/cards.test.tsx` (Task 1); `frontend/src/routes/explore/__tests__/DashboardView.test.tsx` + `DashboardView.editsidebar.test.tsx` (Task 2).

**Ordering:** Task 1 changes only the two card components (the inline branch renders only when `onChange` is passed, which Task 2 wires) — so the app and all integration tests stay green after Task 1. Task 2 wires the editor end-to-end, removes the panel field, and updates the integration tests **in one commit** (these changes are interdependent: an inline editor with `aria-label="text body"` and a panel field with the same label can't both exist, so they must flip together).

**Backend:** none.

---

## Task 1: Inline editor branch in HeadingCard + TextCard

**Files:**
- Modify: `frontend/src/components/chart/dashboard/HeadingCard.tsx`
- Modify: `frontend/src/components/chart/dashboard/TextCard.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/cards.test.tsx`

- [ ] **Step 1: Write the failing tests**

In `frontend/src/components/chart/dashboard/__tests__/cards.test.tsx`, the current imports are:
```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { TextCard } from "../TextCard";
import { HeadingCard } from "../HeadingCard";
import { LinkCard } from "../LinkCard";
```
Change the first two import lines to add `vi` and `fireEvent`:
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
```
Then add a new describe block (anywhere after the existing ones):
```tsx
describe("inline text/heading editing", () => {
  it("HeadingCard shows an editor bound to text when editing", () => {
    const onChange = vi.fn();
    render(<HeadingCard text="Hi" editing onChange={onChange} />);
    const input = screen.getByLabelText("heading text") as HTMLInputElement;
    expect(input.value).toBe("Hi");
    fireEvent.change(input, { target: { value: "Hello" } });
    expect(onChange).toHaveBeenCalledWith("Hello");
  });

  it("HeadingCard shows the display (no editor) when not editing", () => {
    render(<HeadingCard text="Hi" />);
    expect(screen.queryByLabelText("heading text")).toBeNull();
    expect(screen.getByText("Hi")).toBeInTheDocument();
  });

  it("TextCard editor shows raw text (not rendered markdown) when editing", () => {
    const onChange = vi.fn();
    render(<TextCard text="## Notes" editing onChange={onChange} />);
    const ta = screen.getByLabelText("text body") as HTMLTextAreaElement;
    expect(ta.value).toBe("## Notes");
    expect(screen.queryByRole("heading")).toBeNull();
    fireEvent.change(ta, { target: { value: "# New" } });
    expect(onChange).toHaveBeenCalledWith("# New");
  });

  it("TextCard renders markdown when not editing", () => {
    render(<TextCard text="# Title" />);
    expect(screen.queryByLabelText("text body")).toBeNull();
    expect(screen.getByRole("heading", { name: "Title" })).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run the tests, verify they FAIL**

Run: `npx vitest run src/components/chart/dashboard/__tests__/cards.test.tsx`
Expected: FAIL — `getByLabelText("heading text")` / `"text body"` not found (no inline editor yet).

- [ ] **Step 3: Add the inline editor to HeadingCard**

Replace the entire `frontend/src/components/chart/dashboard/HeadingCard.tsx` with:
```tsx
/** A dashboard heading card: a single styled line, no card chrome. In edit mode it is an inline
 *  input bound to the raw text (so {{slug}} stays literal). */
export function HeadingCard({ text, editing, autoFocus, onChange }: { text: string; editing?: boolean; autoFocus?: boolean; onChange?: (text: string) => void }) {
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

- [ ] **Step 4: Add the inline editor to TextCard**

Replace the entire `frontend/src/components/chart/dashboard/TextCard.tsx` with:
```tsx
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/** A dashboard text card: GFM markdown (XSS-safe; react-markdown does not render raw HTML).
 *  In edit mode it is an inline textarea bound to the raw markdown (so {{slug}} stays literal). */
export function TextCard({ text, editing, autoFocus, onChange }: { text: string; editing?: boolean; autoFocus?: boolean; onChange?: (text: string) => void }) {
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

- [ ] **Step 5: Run the tests, verify they PASS**

Run: `npx vitest run src/components/chart/dashboard/__tests__/cards.test.tsx`
Expected: PASS (the 4 new tests + the existing card tests).

- [ ] **Step 6: Full sweep (app behavior unchanged — nothing passes `onChange` yet)**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all PASS, 0 lint errors. (No integration test should change — `DashboardCardView` does not yet pass `onChange`, so the inline branch never renders in the app.)

- [ ] **Step 7: Commit**

```bash
git add frontend/src/components/chart/dashboard/HeadingCard.tsx frontend/src/components/chart/dashboard/TextCard.tsx frontend/src/components/chart/dashboard/__tests__/cards.test.tsx
git commit -m "feat(dashboard): inline editor branch for heading/text cards

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Wire inline editing end-to-end + panel hint + adapt integration tests

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardCardView.tsx`
- Modify: `frontend/src/components/chart/dashboard/CardSettingsPanel.tsx`
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/routes/explore/__tests__/DashboardView.test.tsx`, `frontend/src/routes/explore/__tests__/DashboardView.editsidebar.test.tsx`

These changes are interdependent and land together to keep the suite green (the inline editor's `aria-label="text body"` and the panel's old field with the same label cannot coexist).

- [ ] **Step 1: DashboardCardView — raw in edit, interpolated in view; pass editor props**

In `frontend/src/components/chart/dashboard/DashboardCardView.tsx`:

(a) Add two props to the `DashboardCardView` signature. The current signature destructures
`{ ws, card, chart, refreshNonce, parameters, paramValues, onValueClick, blocked, editing, autoHeight, onAutoHeight, rowHeight }`. Add `onEditText` and `autoFocusText`:
```tsx
export function DashboardCardView({ ws, card, chart, refreshNonce, parameters, paramValues, onValueClick, blocked, editing, autoHeight, onAutoHeight, rowHeight, onEditText, autoFocusText }: { ws: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number; parameters?: DashboardParameter[]; paramValues?: Record<string, unknown>; onValueClick?: (ctx: DrillContext, x: number, y: number) => void; blocked?: boolean; editing?: boolean; autoHeight?: boolean; onAutoHeight?: (rows: number) => void; rowHeight?: number; onEditText?: (text: string) => void; autoFocusText?: boolean }) {
```

(b) Replace the `text` and `heading` branches. They currently read:
```tsx
  if (card.card_type === "text") {
    const text = interpolateParams(asString(card.options.text) ?? "", parameters ?? [], paramValues ?? {});
    return wrap(<TextCard text={text} />);
  }
  if (card.card_type === "heading") {
    const text = interpolateParams(asString(card.options.text) ?? "", parameters ?? [], paramValues ?? {});
    return wrap(<HeadingCard text={text} />);
  }
```
with (raw while editing, interpolated otherwise; thread the editor props):
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

- [ ] **Step 2: CardSettingsPanel — drop the text field, add the `{{slug}}` hint**

In `frontend/src/components/chart/dashboard/CardSettingsPanel.tsx`, replace the text/heading block:
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
with:
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
(`parameters`, `autoHeightRow`, `moveToTab`, `inp` are already in scope. `inp` becomes unused only if no other branch uses it — it is still used by the `link` and chart branches, so leave it.)

- [ ] **Step 3: DashboardView — newCardId state + autofocus + thread editor props**

In `frontend/src/routes/explore/DashboardView.tsx`:

(a) Add state near the other edit state (e.g. after `const [newCardId... ]` does not exist yet — add it after the `replacingId`/`fillingPlaceholderId` states):
```tsx
  const [newCardId, setNewCardId] = useState<string | null>(null);
```

(b) Update `addVirtualCard` to hoist the id and set `newCardId` for text/heading. It currently is:
```tsx
  function addVirtualCard(card_type: "text" | "heading" | "link", options: Record<string, unknown>) {
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      const { w, h } = sizeForViz(card_type).default;
      const pos = getPositionForNewDashCard(base, w, h);
      const newCard: DraftCard = {
        id: `new-${card_type}-${cardSeqRef.current++}`, chart_id: null, card_type,
        grid_col: pos.grid_col, grid_row: pos.grid_row, size_x: w, size_y: h, options, tab_id: null,
        tabKey: activeTab?.key ?? null,
      };
      return [...base, newCard];
    });
  }
```
Replace it with (hoist `id`; set `newCardId` for text/heading):
```tsx
  function addVirtualCard(card_type: "text" | "heading" | "link", options: Record<string, unknown>) {
    const id = `new-${card_type}-${cardSeqRef.current++}`;
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      const { w, h } = sizeForViz(card_type).default;
      const pos = getPositionForNewDashCard(base, w, h);
      const newCard: DraftCard = {
        id, chart_id: null, card_type,
        grid_col: pos.grid_col, grid_row: pos.grid_row, size_x: w, size_y: h, options, tab_id: null,
        tabKey: activeTab?.key ?? null,
      };
      return [...base, newCard];
    });
    if (card_type === "text" || card_type === "heading") setNewCardId(id);
  }
```

(c) Clear `newCardId` on every edit-exit path. Add `setNewCardId(null);`:
- in `cancelEdit` (append to the existing one-liner),
- in `onSave`'s success line (alongside `setEditing(false); ... setDraftName("");`),
- in the `[id]` route `useEffect` (alongside the other `setDraft*(null)` resets).

(d) In the card map, the `<DashboardCardView ... />` element passes a set of props ending with
`onValueClick={editing ? undefined : (ctx, x, y) => onCardValueClick(c, ctx, x, y)} />`. Add two
props to that element:
```tsx
                    onEditText={editing && (c.card_type === "text" || c.card_type === "heading") ? (t) => updateCardOptions(c.id, { text: t }) : undefined}
                    autoFocusText={c.id === newCardId}
```
(Place them among the other `<DashboardCardView>` props; order does not matter.)

- [ ] **Step 4: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS.

- [ ] **Step 5: Adapt the affected integration tests**

Run: `npx vitest run`. Three tests will fail because text/heading editing moved inline and the panel field is gone. Fix them (they must keep asserting real behavior):

(5a) `frontend/src/routes/explore/__tests__/DashboardView.test.tsx` — `it("adds a text card and edits its body", ...)`. It currently opens the T▾ menu, then `openLastCardSettings()` (clicks the card to open the panel), then edits `getByLabelText(/text body/i)`. Now the editor is inline on the card (autofocused on add). Replace the test body with:
```tsx
it("adds a text card and edits its body inline", async () => {
  const user = userEvent.setup();
  renderAt();
  fireEvent.click(screen.getByRole("button", { name: /edit/i }));
  await user.click(screen.getByRole("button", { name: "add text or heading" }));
  await user.click(await screen.findByRole("menuitem", { name: "Text" }));
  const editor = screen.getByLabelText("text body") as HTMLTextAreaElement;
  fireEvent.change(editor, { target: { value: "## Notes" } });
  expect(editor.value).toBe("## Notes");
});
```

(5b) `DashboardView.test.tsx` — `it("adds a heading card", ...)`. Replace its body with:
```tsx
it("adds a heading card", async () => {
  const user = userEvent.setup();
  renderAt();
  fireEvent.click(screen.getByRole("button", { name: /edit/i }));
  await user.click(screen.getByRole("button", { name: "add text or heading" }));
  await user.click(await screen.findByRole("menuitem", { name: "Heading" }));
  expect(screen.getByLabelText("heading text")).toBeInTheDocument();
});
```

If an `openLastCardSettings()` helper in this file becomes unused after these edits, remove it (and any now-unused imports) to keep lint clean. `userEvent` is already imported in this file (used by the Radix menu tests); if not, add `import userEvent from "@testing-library/user-event";`.

(5c) `frontend/src/routes/explore/__tests__/DashboardView.editsidebar.test.tsx` — `it("clicking a card opens the card panel", ...)` asserts `getByLabelText(/heading text|text body/)`, which now matches the inline editor rather than the panel. Re-point it to assert the panel opened via a panel-only control (the new hint), so it still verifies "clicking a card opens its settings panel":
```tsx
it("clicking a card opens the card panel", () => {
  renderView();
  fireEvent.click(screen.getByRole("button", { name: "Edit" }));
  fireEvent.click(screen.getAllByTestId("dashcard-body")[0]);
  expect(screen.getByText(/Insert a filter value/i)).toBeInTheDocument();
});
```
(If `renderView`'s first card is not a text/heading card, the hint won't show — pick the index of a text/heading card in that fixture, matching how the round-2 version of this test targeted a heading card at index 0. Read the fixture and use the correct index so it deterministically asserts the panel hint.)

- [ ] **Step 6: Add a view-mode interpolation test**

In `DashboardView.test.tsx` (or wherever card rendering is tested with params), add a small test that a text card with `{{slug}}` shows the interpolated value in VIEW mode. If the existing harness makes this awkward, instead add a focused `DashboardCardView`-level test in a new/!existing card test — but prefer reusing the `DashboardView.test.tsx` harness. Concretely, adapt to the file's render helper; the assertion is: a text card whose `options.text` is `"Hi {{q}}"` with a parameter `q` set to `"X"` renders `"Hi X"` in view mode. If wiring a parameter value through the harness is heavy, you MAY instead assert it at the component boundary:
```tsx
// in cards.test.tsx is NOT possible (interpolation lives in DashboardCardView), so use a DashboardCardView render
// if too heavy in the integration harness, document that interpolation is covered by the existing
// interpolateParams unit tests (params) + the live Playwright pass, and skip adding a new one.
```
Decision rule: if you can set a param value in the existing `DashboardView` test harness within ~10 lines, add the view-mode interpolation test; otherwise rely on the existing `interpolateParams` coverage + Task 3 live verify and note that in the commit message. Do NOT add a flaky or contrived test.

- [ ] **Step 7: Full sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all PASS, 0 lint errors.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/components/chart/dashboard/DashboardCardView.tsx frontend/src/components/chart/dashboard/CardSettingsPanel.tsx frontend/src/routes/explore/DashboardView.tsx frontend/src/routes/explore/__tests__/
git commit -m "feat(dashboard): inline text/heading editing on canvas + {{slug}} panel hint

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Full sweep + live verification + cleanup

**Files:** none (verification only).

- [ ] **Step 1: Full sweep**

Run from `egret/frontend/`:
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
```
Expected: all PASS, 0 lint errors (the 2 pre-existing map-file warnings are unrelated).

- [ ] **Step 2: Live verification (Playwright, throwaway dashboard, 0 console errors)**

Dev stack is up (frontend `http://localhost:5173`, backend `:8080`). Log in `admin@egret.local` / `Admin12345!`. Create a **throwaway** dashboard (`+ Dashboard` on `/explore/eng/dashboards`); never use `a1`. In edit mode, verify:
1. **Add heading** (toolbar `T▾` → Heading) → its inline input is **autofocused**; type "Sales Overview" directly on the card; it shows on the card.
2. **Add text** (`T▾` → Text) → type markdown inline (e.g. `**bold** note`); raw text shows while editing.
3. **Panel**: click the heading/text card → the settings panel shows the **hint** ("Insert a filter value with {{slug}}" + available slugs) and **no** text field; auto-height/move-to-tab still present.
4. **`{{slug}}` interpolation**: add a Filter (e.g. slug `category`), put `{{category}}` in a text card, Save → in **view** mode set the filter value → the card shows the value (not the literal `{{category}}`).
5. **Persistence**: Save → reload → the heading/text content persists; view mode renders markdown.
6. **Console**: `browser_console_messages` (per-navigation) shows 0 errors throughout.

Capture before/after screenshots to `/Users/1pro-l4r8/Documents/github/data-portal/` (parent dir, NOT the repo).

- [ ] **Step 3: Cleanup**

Permanently delete the throwaway dashboard (Trash → Delete permanently → type `delete`). Delete the scratch screenshots from the parent dir. Confirm `git status` shows only intended source/spec/plan changes.

- [ ] **Step 4: Final whole-branch review**

Dispatch the final code reviewer over the branch diff (`git diff main...HEAD`), then proceed to `superpowers:finishing-a-development-branch`.

---

## Self-Review notes

- **Spec coverage:** A (inline editors) → Task 1 + Task 2 Step 1. B (panel coexist, drop field) → Task 2 Step 2. C (#2 hint) → Task 2 Step 2. Autofocus → Task 2 Step 3. Raw-in-edit/interpolated-in-view → Task 2 Step 1. Tests → Task 1 (component) + Task 2 (integration) + Task 3 (live). ✓
- **Type consistency:** `HeadingCard`/`TextCard` props `{ text; editing?; autoFocus?; onChange? }` defined in Task 1, consumed in Task 2 Step 1. `DashboardCardView` new props `onEditText?: (text:string)=>void` + `autoFocusText?: boolean` defined and passed (Task 2 Steps 1,3). `newCardId` state used consistently. `aria-label="heading text"`/`"text body"` on the inline editors (Task 1) matches the test queries (Tasks 1,2). ✓
- **Green-per-commit:** Task 1's inline branch only renders when `onChange` is passed; nothing passes it until Task 2, so integration tests stay green after Task 1. Task 2 flips the panel field → inline editor + updates the 3 affected tests in the same commit. ✓
- **Ambiguity note:** Task 2 Step 6 (view-mode interpolation test) has a decision rule rather than a fixed test because the existing harness's param-value wiring is unknown to the plan author; the implementer picks based on what the harness supports, without adding a contrived test.
