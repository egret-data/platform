# Metabase-faithful dashboard style — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restyle egret's dashboard surfaces to look like Metabase — light-gray canvas, white flat-bordered cards (no shadow, 6px radius), header/tab/filter/edit chrome and typography — while keeping egret's teal brand.

**Architecture:** Pure visual restyle: add one OKLCH token (`--dashboard`, light + dark) and change Tailwind classes on existing dashboard components. No logic, data, or behavior changes. Branch `feat/metabase-dashboard-style` (off main, spec committed).

**Tech Stack:** React 18 + TS + Vite, Tailwind v4 (`@theme` / `@theme inline` in `src/index.css`), Vitest.

**Spec:** `docs/en/specs/2026-06-23-metabase-dashboard-style-design.md`.

**Radius scale (verified — `src/index.css:187-190`, shadcn-derived from `--radius: 0.625rem` = 10px):** `rounded-sm` = **6px**, `rounded-md` = **8px**, `rounded-lg` = **10px**. So Metabase's 6px card radius is egret **`rounded-sm`**; Metabase's 8px toolbar radius is egret `rounded-md` (the card toolbar already uses it — no change).

**Conventions (every task):** Read each file before editing. Styling-only — do NOT change logic/behavior or break existing role/text test queries. Full sweep before each commit (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`. Stage explicit paths. Configured git identity. End every commit message with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Do NOT push until Task 5.

## File map

- Modify `src/index.css` — add `--dashboard` token + `--color-dashboard` registration (Task 1)
- Modify `src/routes/explore/DashboardView.tsx` — canvas, card chrome, edit bar, header title (Tasks 2-3)
- Modify `src/components/chart/dashboard/DashboardCardView.tsx` — card title padding (Task 3)
- Modify `src/components/chart/dashboard/HeadingCard.tsx` — heading typography (Task 3)
- Modify `src/components/chart/dashboard/DashboardTabBar.tsx` — tab baseline (Task 4)
- Modify the dashboard filter-pill component(s) — `DashboardFilterBar.tsx` / `EditFilterChips.tsx` (Task 4)

---

### Task 1: Add the `--dashboard` canvas token (light + dark)

**Files:**
- Modify: `frontend/src/index.css`

- [ ] **Step 1: Add the light value to `:root`.** The light-mode color block starts at line ~93 (`:root { /* Light mode colors */ … }`). After the `--secondary: oklch(0.95 0.01 258.36);` line, add:
```css
  --dashboard: oklch(0.955 0.004 270); /* Metabase-style light-gray dashboard canvas (cards float on this) */
```

- [ ] **Step 2: Add the dark value to `.dark`.** The dark block starts at line ~132 (`.dark { … }`). After its `--secondary:` line, add:
```css
  --dashboard: oklch(0.16 0 0); /* darker than --card (0.21) so cards still pop in dark mode */
```

- [ ] **Step 3: Register the Tailwind color in `@theme inline`** (the block at line ~164 that maps `--color-*: var(--*)`). After the `--color-secondary: var(--secondary);` line, add:
```css
  --color-dashboard: var(--dashboard);
```

- [ ] **Step 4: Verify `bg-dashboard` resolves.** Full sweep (the build compiles Tailwind, so a missing token surfaces as a build error or an unstyled class):
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
```
Expected: all green. (Optional sanity check: `grep -n "dashboard" src/index.css` shows the three additions.)

- [ ] **Step 5: Commit.**
```bash
git add src/index.css
git commit -m "feat(dashboards): add --dashboard canvas token (Metabase-style gray, light + dark)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Canvas + card chrome + edit bar (`DashboardView.tsx`)

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`

- [ ] **Step 1: Canvas background.** Read the file. At line ~277 the page wrapper is:
```tsx
    <div className={view.isFullscreen ? "fixed inset-0 z-50 overflow-auto bg-muted/30 p-4" : "min-h-full bg-muted/30 p-4"}>
```
Replace **both** `bg-muted/30` with `bg-dashboard`:
```tsx
    <div className={view.isFullscreen ? "fixed inset-0 z-50 overflow-auto bg-dashboard p-4" : "min-h-full bg-dashboard p-4"}>
```

- [ ] **Step 2: Card chrome.** At line ~426 the card wrapper is:
```tsx
              <div className={`relative h-full overflow-hidden ${c.card_type === "heading" ? "" : "rounded-lg border bg-card shadow-sm"} ${editing && editPanel?.kind === "card" && editPanel.cardId === c.id ? "ring-2 ring-primary" : ""}`}>
```
Change the chart-card branch `rounded-lg border bg-card shadow-sm` → `rounded-sm border bg-card` (drop the shadow; 10px→6px radius). Keep the heading-card empty branch and the `ring-2 ring-primary` selected state exactly:
```tsx
              <div className={`relative h-full overflow-hidden ${c.card_type === "heading" ? "" : "rounded-sm border bg-card"} ${editing && editPanel?.kind === "card" && editPanel.cardId === c.id ? "ring-2 ring-primary" : ""}`}>
```

- [ ] **Step 3: Edit bar opacity.** At line ~279 the edit bar is `… mb-4 flex items-center gap-2 bg-primary px-4 py-2 …`. Soften the teal to ~85% to match Metabase: change `bg-primary` → `bg-primary/90`:
```tsx
        <div className="-mx-4 -mt-4 mb-4 flex items-center gap-2 bg-primary/90 px-4 py-2 text-sm font-medium text-primary-foreground">
```

- [ ] **Step 4: Full sweep + commit.**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboards): Metabase canvas + flat-bordered cards (gray bg, no shadow, 6px)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```
(If a dashboard integration test asserted the `shadow-sm`/`rounded-lg`/`bg-muted` class, update that assertion to the new class and note it. Most tests query by role/text and are unaffected.)

---

### Task 3: Header + card title + heading typography

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Modify: `frontend/src/components/chart/dashboard/DashboardCardView.tsx`
- Modify: `frontend/src/components/chart/dashboard/HeadingCard.tsx`

- [ ] **Step 1: Dashboard header title + bottom border.** In `DashboardView.tsx` the header row is (line ~290-291):
```tsx
      <div className="mb-3 flex items-center justify-between">
        <h1 className="text-lg font-semibold">{dash.name}</h1>
```
Change to a 20px/700 title with a divider under the header row (`text-xl` is 1.25rem = 20px in egret's `@theme`; `font-bold` = 700):
```tsx
      <div className="mb-3 flex items-center justify-between border-b pb-3">
        <h1 className="text-xl font-bold">{dash.name}</h1>
```

- [ ] **Step 2: Card title padding** in `DashboardCardView.tsx` (line ~36):
```tsx
      {showHeader && <div className="px-3 pt-2 pb-1 text-sm font-medium">{headerText}</div>}
```
→ Metabase card-title padding (keep 14px medium; `text-foreground` for the darker title color):
```tsx
      {showHeader && <div className="px-4 pt-3 pb-2 text-sm font-medium text-foreground">{headerText}</div>}
```

- [ ] **Step 3: Heading card** in `HeadingCard.tsx` — bump to a section-heading look (`text-xl font-bold`), keep transparent/no-chrome:
```tsx
    <div className="flex h-full items-center px-1 text-xl font-bold text-foreground">
```
(Only the `text-lg font-semibold` → `text-xl font-bold` change; keep everything else including the muted "Heading" placeholder.)

- [ ] **Step 4: Full sweep + commit.**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/routes/explore/DashboardView.tsx src/components/chart/dashboard/DashboardCardView.tsx src/components/chart/dashboard/HeadingCard.tsx
git commit -m "feat(dashboards): Metabase header + card/heading typography

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Tab baseline + filter-pill chrome

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardTabBar.tsx`
- Modify: the dashboard filter-pill component(s) — read `frontend/src/components/chart/dashboard/DashboardFilterBar.tsx` and `frontend/src/components/chart/dashboard/EditFilterChips.tsx`

- [ ] **Step 1: Tab baseline.** Read `DashboardTabBar.tsx`. The container is `<div className="mb-3 flex items-center gap-1 border-b" role="tablist">` and the active tab uses `border-b-2 border-primary text-primary font-medium` (inactive `border-transparent text-muted-foreground`). To seat the active tab on a 2px baseline like Metabase, change the container border to 2px and pull tabs down 2px so the active indicator overlaps the baseline:
  - Container: `border-b` → `border-b-2 border-border`.
  - The tab item row: add `-mb-[2px]` to the `cls` string (the per-tab `flex items-center gap-1 px-3 py-1.5 text-sm border-b-2 …`) so the active tab's 2px border sits on the baseline. Keep the teal `border-primary text-primary font-medium` active state and the transparent inactive state.
  Concretely, set `const cls = \`-mb-[2px] flex items-center gap-1 px-3 py-1.5 text-sm border-b-2 ${active ? "border-primary text-primary font-medium" : "border-transparent text-muted-foreground"}\`;`.

- [ ] **Step 2: Filter pills.** Read `DashboardFilterBar.tsx` (view-mode filter widgets) and `EditFilterChips.tsx` (edit-mode chips). For each filter pill/button, align the chrome to Metabase: a `h-8 rounded-sm border bg-background px-2 text-sm` pill (6px radius) with its existing leading icon + label (+ trailing chevron if present); a filled/active filter uses `bg-secondary` instead of `bg-background`. Apply ONLY class changes — do not alter the value/selection logic or the existing aria-labels. If the current markup already uses an equivalent pill (e.g. a shared Button), adjust its variant/classes minimally to hit `h-8 rounded-sm border px-2 text-sm`; report what you found and changed.

- [ ] **Step 3: Full sweep + commit.**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/DashboardTabBar.tsx src/components/chart/dashboard/DashboardFilterBar.tsx src/components/chart/dashboard/EditFilterChips.tsx
git commit -m "feat(dashboards): Metabase tab baseline + filter-pill chrome

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```
(Stage only the files you actually changed — if `EditFilterChips.tsx` needed no change, omit it.)

---

### Task 5: Live verification + PR

**Files:** none (verification only).

- [ ] **Step 1: Final sweep** (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run` → green.

- [ ] **Step 2: Run the stack** (frontend-only; `npm run dev`), log in `admin@egret.local` / `Admin12345!`, eng workspace. **Do NOT touch "a1".** Create a NEW throwaway dashboard with a Table + a Bar + a Scalar + a Heading card. Keep Metabase open (localhost:3001, `alice@test.com` / `123123a@`, the "A look at Orders" x-ray) for side-by-side comparison. Delete the throwaway at the end.

- [ ] **Step 3: Live-verify (Playwright MCP, 0 console errors):**
1. **Canvas + cards:** the dashboard canvas is a clear light gray; chart cards are white tiles with a 1px border and **no drop shadow**; card corner radius ≈ 6px (measure `getComputedStyle(card).borderRadius` ≈ `6px`; `boxShadow` is `none`). Cards visibly "float" on the gray like the Metabase x-ray.
2. **Header:** dashboard title is 20px bold with a 1px divider beneath the header row.
3. **Card title:** padded `px-4 pt-3`; heading card renders as a 20px bold section heading with no border/background.
4. **Tabs:** the active tab indicator is teal and sits on a 2px baseline.
5. **Filter pills:** match Metabase height (~32px), 6px radius, 1px border.
6. **Edit mode:** teal edit bar (slightly softened), white card toolbar pill (hover-revealed), SE resize handle, grid overlay — all intact.
7. **Dark mode:** toggle the theme — the canvas is darker than the cards (cards still readable/white-ish), borders visible, brand teal intact, 0 errors.
8. **Existing dashboard:** open PARITY **read-only** (do NOT save) — coherent under the new style, no overlap, 0 errors.

- [ ] **Step 4: Clean up & open PR.** Delete the throwaway dashboard. Then:
```bash
git push -u origin feat/metabase-dashboard-style
```
Open PR `feat(dashboards): Metabase-faithful dashboard style (canvas, cards, header, tabs, filters)`; summarise the restyle (gray canvas + flat-bordered cards + header/tab/filter/typography), note egret keeps its teal brand and it's a frontend-only visual change. End the PR body with the `🤖 Generated with [Claude Code]` footer. Squash-merge when CI is green (a flaky `egret-ranger` rust test may need a rerun — unrelated to this frontend change).

---

## Self-Review

**Spec coverage:**
- `--dashboard` token (light + dark) + `@theme inline` registration → Task 1. ✓
- Canvas `bg-dashboard` + card `rounded-sm border bg-card` (no shadow) + edit bar `/90` → Task 2. ✓
- Header 20px/700 + bottom border, card-title padding, heading typography → Task 3. ✓
- Tab 2px baseline (teal indicator), filter-pill chrome → Task 4. ✓
- Live verify (egret vs Metabase, light + dark, PARITY read-only) + PR → Task 5. ✓
- Teal brand kept (no `--primary` change anywhere); dashboard-only scope (no chart-builder/SQL-Lab files touched). ✓

**Radius correctness:** Metabase 6px = egret `rounded-sm` (verified `--radius-sm = --radius - 4px = 6px`). The plan uses `rounded-sm` for the card (not `rounded-md`, which is 8px). The card edit toolbar (already `rounded-md` = 8px) matches Metabase's 8px toolbar — intentionally unchanged.

**Placeholder scan:** No TBD/TODO. Task 4 Step 2 is the only "read then adapt" instruction — bounded with exact target classes (`h-8 rounded-sm border bg-background px-2 text-sm`, active `bg-secondary`) and a "report what you changed" requirement, because the current filter-pill markup must be read first.

**Type/class consistency:** `bg-dashboard` (Task 2) depends on `--color-dashboard` (Task 1). `rounded-sm` (6px) used consistently for card + filter pills. `text-xl font-bold` used for both the dashboard header (Task 3 Step 1) and heading card (Task 3 Step 3). No behavior/prop changes, so no signature drift.

**Test note:** styling has no unit logic; the guard is the full sweep staying green (existing dashboard tests query by role/text, not by these classes) + the live verify. If any test asserted a changed class, update it minimally (called out in Task 2 Step 4).
