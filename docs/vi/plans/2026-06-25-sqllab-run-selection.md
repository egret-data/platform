# Kế hoạch: SQL Lab Run-Selection + Phím tắt

> Bản chi tiết (canonical): `docs/en/plans/2026-06-25-sqllab-run-selection.md`.
> Spec: `docs/{en,vi}/specs/2026-06-25-sqllab-run-selection-design.md`. Sub-project 1/4.

**Mục tiêu:** chạy SQL bôi đen (nút "Run selection"/"Run") + `Cmd/Ctrl+Enter`, theo selection,
no-op khi đang chạy. Chỉ frontend.

**Quy ước:** chạy trong `egret/frontend/`. Sweep trước mỗi commit:
`npm run typecheck && npm run lint && npm run build && npx vitest run`. Branch
`feat/sqllab-run-selection`. Commit kết bằng
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## Các task (4)
1. **`pickRunSql(full, selected)`** (`sqllab/pickRunSql.ts`) + unit test: selected có chữ→selected;
   whitespace/null/""→full.
2. **`SqlEditor`** thêm props `onRun?`/`onSelectionChange?`; `onMount` bắt editor: keybinding
   `CtrlCmd|Enter`→`onRun(pickRunSql(full, selected))`, listener `onDidChangeCursorSelection`→
   `onSelectionChange(selectedText)`; ref giữ callback mới nhất; disposables mảng. Props optional →
   app/test cũ vẫn xanh.
3. **`SqlTab`** state `selectedSql`; `run(sqlOverride?)` (guard `if(running) return`); nút Run
   selection-aware (label + `title="⌘/Ctrl+↵"`); truyền `onRun={(sql)=>run(sql)}` +
   `onSelectionChange={setSelectedSql}`. Test mới: stub `SqlEditor` (nút gọi onSelectionChange/onRun)
   + mock api → assert label đổi + submit nhận đúng SQL.
4. **Sweep + live verify** (Playwright, 0 console error): bôi đen `SELECT 2;` → "Run selection" chạy
   đúng; bỏ chọn → "Run" chạy toàn bộ; `Cmd/Ctrl+Enter`; inert khi chạy. Cleanup + final review →
   `finishing-a-development-branch`.

## Files
- **Mới:** `sqllab/pickRunSql.ts` (+test), `__tests__/SqlTab.runselection.test.tsx`.
- **Sửa:** `sqllab/SqlEditor.tsx`, `sqllab/SqlTab.tsx`. **Backend:** không.

## Lưu ý
- Monaco bị mock thành `<textarea>` trong test → keybinding/selection ở `onMount` không chạy trong
  unit test; coverage qua `pickRunSql` (thuần) + SqlTab test (stub SqlEditor) + live verify.
