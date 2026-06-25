# Kế hoạch triển khai: SQL Lab History / Saved / Toolbar UX

> Bản tiếng Anh là bản chuẩn (canonical) và là bản agent thực thi:
> `docs/en/plans/2026-06-25-sqllab-history-saved-ux.md`. Bản này là tham chiếu tiếng Việt.

**Mục tiêu:** Làm History/Saved giàu thông tin (rows kiểu Superset: badge status, DB, thời gian
tương đối, số dòng, action mỗi hàng), sửa limit bị cắt + cho nhập custom, đưa lỗi query ra khỏi
toolbar vào panel Results kèm dòng thời gian chạy (đo phía client). Chỉ frontend.

**Spec:** `docs/en/specs/2026-06-25-sqllab-history-saved-ux-design.md`

## Các task (chi tiết + code đầy đủ ở bản en)

1. **`lib/relativeTime.ts`** — `timeAgo` / `fullTimestamp` (dayjs relativeTime) + test.
2. **`QueryStatusBadge.tsx`** — badge màu theo status (success/destructive/processing/warning/secondary) + test.
3. **`api/sqllab.ts`** — thêm `useUpdateSavedQuery` (PUT `/ws/{ws}/sqllab/queries/{id}`).
4. **`SaveQueryDialog.tsx`** — chế độ edit (prefill name/description, đổi title + nút) + test.
5. **`ResultsGrid.tsx`** — prop `error` (block đỏ) + `statusLine` (footer) + test.
6. **`HistoryTab.tsx`** — list rows giàu (badge/DB/time/rows, Copy, Re-run, empty state) + test.
7. **`SavedTab.tsx`** — list rows giàu (name/desc/DB/updated, Edit qua dialog, Delete có
   AlertDialog xác nhận, empty state) + test.
8. **`SqlTab.tsx`** — bỏ HistoryTab/SavedTab inline, import bản mới; Tabs controlled + tự chuyển
   sang Results khi chạy; đưa lỗi vào ResultsGrid; đo elapsed phía client; limit nới `w-36` +
   mục "Custom…" (ô số inline, Enter/blur commit, clamp ≤ 100000); `run` nhận thêm db override
   để Re-run đúng database + test tích hợp.
9. **Sweep + verify live (Playwright) + finish branch.**

Quy ước: chạy lệnh trong `egret/frontend/`; sweep đầy đủ
`npm run typecheck && npm run lint && npm run build && npx vitest run`; `git add` cần prefix
`frontend/`; branch `feat/sqllab-history-saved-ux`; cuối commit thêm dòng
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
