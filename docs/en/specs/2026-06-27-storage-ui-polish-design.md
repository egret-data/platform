# Storage UI polish — provider dropdown + MinIO-console file browser redesign — Design

**Date:** 2026-06-27 · **Status:** Design — approved · **Branch:** `feat/storage-ui-polish` (off main)
**Scope:** FE-only polish on the storage area (no backend change). Reference: MinIO Console object browser.

## Goal
1. **Provider dropdown** in the Add/Edit-connection form (currently provider is implicitly "s3"): a `Select` with **S3 / MinIO** active + **GCS / Azure (coming soon)** disabled — so the UI is provider-aware (BE already reserves the `provider` column; real GCS/Azure backends are a later epic).
2. **Redesign the file browser** to look like the MinIO Console object browser (the current flat list is too plain): a proper toolbar + breadcrumb + a columned table with **file-type icons**, a per-row **actions menu (⋮)**, row hover, and a nicer empty state. Keep ALL existing behaviour (navigate / upload / download / delete / new folder / move-rename).

## 1. Provider dropdown (`StoragePage.tsx`)
- In the Add/Edit connection dialog, replace the implicit provider with a shadcn `Select` (provider): options `s3` ("S3 / MinIO") enabled; `gcs` ("Google Cloud Storage — coming soon"), `azure` ("Azure Blob — coming soon") rendered DISABLED. Default `s3`. The form already sends `provider` in the request; just make it a visible, selectable field (only `s3` is selectable now). The connection table already shows the provider.

## 2. File browser redesign (`FileBrowser.tsx` + a `fileIcon` helper)

### Layout (MinIO-console-like)
- **Header row:** `‹ Back` + the connection name; a **breadcrumb** with a bucket icon then the path segments (root = the bucket; each segment clickable); on the right a **toolbar**: `⟳ Refresh` (refetch), `+ New folder`, `⬆ Upload`.
- **Table** (shadcn-less plain `<table>` styled, matching the app): columns **[type icon] · Name · Last modified · Size · [⋮]**.
  - **Folders first** (sorted), folder icon, click row (name) → navigate in; **Files** after, with a **type icon** (see helper); size human-readable ("1.2 MB"), last-modified formatted.
  - Row **hover** highlight; the whole name cell for a folder is the navigate target; an **`⋮` dropdown** (shadcn `DropdownMenu`) at the row end with actions:
    - File: **Download**, **Move / Rename**, **Delete**.
    - Folder: **Open**, **Move / Rename**, **Delete** (recursive).
  - (No inline action buttons — they move into the `⋮` menu for a clean MinIO look.)
- **Empty state:** a centered muted folder icon + "This folder is empty" + a short hint ("Upload files or create a folder").
- **Loading:** a table-row skeleton. **403** → admin notice (unchanged). **Error** → alert.
- Keep the New-folder dialog + the Move/Rename dialog + the ConfirmDeleteDialog from the current implementation (just trigger them from the `⋮` menu / toolbar).

### `fileIcon(name): LucideIcon` helper (file-type icons — folds in the requested feature)
Map by extension (lowercase, after the last `.`):
- `csv`,`tsv` → `Table2`; `parquet`,`orc`,`avro` → `Database` (or `Columns3`); `xlsx`,`xls` → `Sheet`/`Table2`; `json`,`ndjson` → `Braces`; `txt`,`md`,`log` → `FileText`; `png`,`jpg`,`jpeg`,`gif`,`svg`,`webp` → `Image`; `zip`,`gz`,`tar`,`7z` → `FileArchive`; `pdf` → `FileText`; `py`,`sql`,`js`,`ts`,`rs`,`yaml`,`yml`,`toml` → `FileCode`; default → `File`. Folder → `Folder`. (Use lucide-react icons that exist; pick the closest available.)

## 3. Testing
- **FE (Vitest, mock `@/api/storage`):**
  - Provider: the Add dialog renders the provider `Select`; only `s3` is selectable (gcs/azure disabled); submit still sends `provider: "s3"`.
  - File browser: renders the table with type icons (a `.csv` row shows the csv icon, a `.parquet` row the parquet icon, a folder the folder icon — assert by icon test-id/aria or the lucide class); the `⋮` menu exposes Download/Move-Rename/Delete for files and Open/Move-Rename/Delete for folders; clicking a folder name navigates (prefix change); New folder / Upload / Move / Delete still call the right hooks; empty state renders when no folders+objects.
  - Keep existing storage tests green (the actions moved into the `⋮` menu — update selectors accordingly).

## 4. Risks / notes
- FE-only — no backend or contract change; all hooks unchanged.
- Don't regress behaviour while moving actions into the `⋮` menu (download/move/delete/navigate must all still work) — the Playwright live check + the Vitest updates cover it.
- shadcn `DropdownMenu` + `Select` exist in the project (`src/components/ui/`). Use them; for the table use plain markup (no shadcn table primitive).
- React 18: any shadcn primitive used with `asChild` needs forwardRef — use the existing components as-is (they already work elsewhere).

## 5. Out of scope (later)
- A right-side **object detail drawer** (preview/metadata/share) like MinIO — nice follow-on.
- Multi-select + bulk actions; drag-and-drop upload; rewind/versioning.
- Real GCS/Azure backends (the dropdown only advertises them).
