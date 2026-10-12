import { useId, useState } from "react";
import { Download, FileQuestion, FileX, RotateCw } from "lucide-react";
import { useWorkflowPreviewExecutionFile } from "@/api/generated/workflow/workflow";
import type { FilePreviewDto } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import {
  downloadHref,
  fileName,
  formatBytes,
  isIonFile,
  isNotFound,
  NO_LONGER_AVAILABLE,
} from "@/components/workflows/executions/files/fileModel";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/**
 * The row caps the preview offers. The route's default is 100 and its limit
 * 5000 (`maxRows` in `1..5000`), so these are Kestra's `FilePreviewForm.vue`
 * choices up to that limit.
 */
export const PREVIEW_ROW_CAPS = [50, 100, 250, 500, 1000, 5000] as const;
const DEFAULT_ROWS = 100;

const errorMessage = (e: unknown) => (e instanceof Error && e.message ? e.message : "the request failed");

/**
 * A file's preview, in a drawer on the right (W7b spec §8): the first rows of
 * `file/preview`, an Ion file's as JSON values and any other file's as lines.
 * It says when the file cannot be previewed, when the preview stops short (with
 * the row cap), and when the file is no longer there. Kestra's
 * `FilePreviewDrawer.vue` and `FilePreview.vue`: the row-count picker, the
 * truncation notice and Download; egret has no encoding choice, because the
 * route reads text as UTF-8 only.
 *
 * A `ui/sheet` (a Radix dialog sliding in from the right), so focus moves into
 * it, stays there, and Escape closes it; on close, focus goes to
 * `returnFocus()` (the Preview button, or its row when the button has gone)
 * rather than wherever the browser last had it. The header and the toolbar
 * (Rows, Download) stay put; only the rows scroll.
 */
export function FilePreviewDrawer({
  ws,
  executionId,
  uri,
  size,
  open,
  onOpenChange,
  returnFocus,
}: {
  ws: string;
  executionId: string;
  uri: string;
  /** From `file/metas`, when the row has it. */
  size?: number;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Where focus goes on close. */
  returnFocus?: () => HTMLElement | null;
}) {
  const name = fileName(uri);
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        onCloseAutoFocus={(e) => {
          const target = returnFocus?.();
          if (!target) return;
          e.preventDefault();
          target.focus();
        }}
      >
        <SheetHeader>
          <SheetTitle className="break-all font-mono text-base">{`Preview of ${name}`}</SheetTitle>
          <SheetDescription className="break-all font-mono text-xs">
            {uri}
            {size !== undefined ? <span className="tabular-nums">{` · ${formatBytes(size)}`}</span> : null}
          </SheetDescription>
        </SheetHeader>
        {/* Mounted only while open: Radix unmounts the content when closed, so nothing is read before. */}
        <PreviewBody ws={ws} executionId={executionId} uri={uri} />
      </SheetContent>
    </Sheet>
  );
}

/** Download, a plain link to the file route (`downloadHref`): shared by the row and the drawer. */
export function DownloadLink({ ws, executionId, uri, className }: { ws: string; executionId: string; uri: string; className?: string }) {
  const name = fileName(uri);
  return (
    <Button asChild variant="outline" size="sm" className={cn(className)}>
      <a href={downloadHref(ws, executionId, uri)} download={name} aria-label={`Download ${name}`}>
        <Download aria-hidden />
        Download
      </a>
    </Button>
  );
}

function PreviewBody({ ws, executionId, uri }: { ws: string; executionId: string; uri: string }) {
  const [maxRows, setMaxRows] = useState<number>(DEFAULT_ROWS);
  const rowsId = useId();
  const query = useWorkflowPreviewExecutionFile(
    ws,
    executionId,
    { path: uri, maxRows },
    // A stored file never changes: one read per row cap.
    { query: { queryKey: workflowsKeys.executionFilePreview(executionId, uri, maxRows), staleTime: Infinity } },
  );
  const preview = query.data?.status === 200 ? query.data.data : undefined;

  // Error first (FE-STATE-1); a 404 is its own state, not a failure to retry.
  if (query.isError && isNotFound(query.error)) {
    return (
      <EmptyState
        icon={FileX}
        title={NO_LONGER_AVAILABLE}
        description="It was removed after the retention period, or this execution cannot read it. There is nothing to preview or download."
      />
    );
  }

  // A flex column filling the sheet: the toolbar and any notice stay put, and
  // only the rows' list scrolls (`min-h-0 flex-1 overflow-auto`).
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <Label htmlFor={rowsId}>Rows</Label>
          <Select value={String(maxRows)} onValueChange={(v) => setMaxRows(Number(v))}>
            <SelectTrigger id={rowsId} size="sm" className="w-24 tabular-nums">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {PREVIEW_ROW_CAPS.map((n) => (
                <SelectItem key={n} value={String(n)}>
                  {String(n)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <DownloadLink ws={ws} executionId={executionId} uri={uri} />
      </div>
      {query.isError ? (
        <Alert variant="destructive">
          <AlertDescription className="flex flex-wrap items-center gap-3">
            <span>The preview could not be loaded: {errorMessage(query.error)}. Try again, or download the file.</span>
            <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
              <RotateCw aria-hidden />
              Retry
            </Button>
          </AlertDescription>
        </Alert>
      ) : !preview ? (
        <div className="grid gap-2" data-testid="preview-loading">
          {Array.from({ length: 6 }, (_, i) => (
            <Skeleton key={i} className="h-4 w-full" />
          ))}
        </div>
      ) : (
        <PreviewRows uri={uri} preview={preview} maxRows={maxRows} />
      )}
    </div>
  );
}

function PreviewRows({ uri, preview, maxRows }: { uri: string; preview: FilePreviewDto; maxRows: number }) {
  if (!preview.previewable) {
    // Also this module's marker in `scripts/assert-workflows-lazy.mjs`.
    return (
      <EmptyState
        icon={FileQuestion}
        title="This file cannot be previewed"
        description="It is not text or Ion, or its Ion does not parse. Download it to open it."
      />
    );
  }
  if (preview.rows.length === 0) {
    return <p className="text-sm text-muted-foreground">The file is empty.</p>;
  }
  const ion = isIonFile(uri);
  const n = preview.rows.length;
  return (
    <>
      {preview.truncated ? (
        <p className="shrink-0 text-sm text-muted-foreground tabular-nums" role="note">
          {n < maxRows
            ? `Showing the first ${n} rows: the preview's size limit was reached before ${maxRows} rows. Download the file to read all of it.`
            : `Showing the first ${maxRows} rows; the file has more. Choose more rows, or download the file to read all of it.`}
        </p>
      ) : null}
      <ol
        aria-label={ion ? "Values" : "Lines"}
        data-testid="preview-scroll"
        className="min-h-0 flex-1 overflow-auto rounded-md border bg-muted/40 font-mono text-xs"
      >
        {preview.rows.map((row, i) => (
          <li key={i} className="flex gap-3 border-b px-2 py-1 last:border-b-0">
            <span aria-hidden className="w-10 shrink-0 select-none text-right tabular-nums text-muted-foreground">
              {i + 1}
            </span>
            <pre data-testid="preview-row" className="min-w-0 whitespace-pre-wrap break-all">
              {ion || typeof row !== "string" ? JSON.stringify(row, null, 2) : row}
            </pre>
          </li>
        ))}
      </ol>
    </>
  );
}
