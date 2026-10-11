import { useRef, useState, type ReactNode } from "react";
import { Download, Eye, FileText, FileX } from "lucide-react";
import { useWorkflowExecutionFileMetas } from "@/api/generated/workflow/workflow";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { FilePreviewDrawer } from "@/components/workflows/executions/files/FilePreviewDrawer";
import {
  downloadHref,
  fileName,
  formatBytes,
  isNotFound,
  NO_LONGER_AVAILABLE,
} from "@/components/workflows/executions/files/fileModel";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/**
 * A `kestra:///` value as a file (W7b spec §8): its name (the URI on hover),
 * its size from `file/metas`, **Preview**, which opens the drawer, and
 * **Download**, a plain link to the file route. Kestra's `VarValue.vue`:
 * Download, Preview and the size side by side, and a missing file says so
 * instead.
 *
 * A file's bytes never change once stored, so its size is read once
 * (`staleTime: Infinity`) however often the execution around it polls.
 */
export function FileRow({
  ws,
  executionId,
  uri,
  label,
}: {
  ws: string;
  executionId: string;
  uri: string;
  /** Shown before the name, for a row that needs to say where the file is (an output's path). */
  label?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const previewButton = useRef<HTMLButtonElement>(null);
  const name = fileName(uri);
  const metas = useWorkflowExecutionFileMetas(
    ws,
    executionId,
    { path: uri },
    { query: { queryKey: workflowsKeys.executionFileMetas(executionId, uri), staleTime: Infinity } },
  );
  const size = metas.data?.status === 200 ? metas.data.data.size : undefined;
  const gone = metas.isError && isNotFound(metas.error);

  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-sm" data-testid="file-row">
      {label ? <span className="min-w-0 break-all font-mono text-xs text-muted-foreground">{label}</span> : null}
      <span className="inline-flex min-w-0 items-center gap-1.5">
        {gone ? (
          <FileX aria-hidden className="size-4 shrink-0 text-muted-foreground" />
        ) : (
          <FileText aria-hidden className="size-4 shrink-0 text-muted-foreground" />
        )}
        <span className="truncate font-mono text-xs" title={uri}>
          {name}
        </span>
      </span>
      {gone ? (
        // Error before data (FE-STATE-1): a missing file offers nothing to open.
        <span className="text-muted-foreground">{NO_LONGER_AVAILABLE}</span>
      ) : (
        <>
          {metas.isError ? (
            <span className="text-muted-foreground">The size could not be read; the file may still open.</span>
          ) : size === undefined ? (
            <Skeleton className="h-4 w-14" data-testid="file-size-loading" />
          ) : (
            <span className="tabular-nums text-muted-foreground">{formatBytes(size)}</span>
          )}
          <span className="flex items-center gap-1">
            <Button
              ref={previewButton}
              type="button"
              variant="outline"
              size="sm"
              className="h-7"
              aria-label={`Preview ${name}`}
              onClick={() => setOpen(true)}
            >
              <Eye aria-hidden />
              Preview
            </Button>
            <Button asChild variant="outline" size="sm" className="h-7">
              <a href={downloadHref(ws, executionId, uri)} download={name} aria-label={`Download ${name}`}>
                <Download aria-hidden />
                Download
              </a>
            </Button>
          </span>
        </>
      )}
      <FilePreviewDrawer
        ws={ws}
        executionId={executionId}
        uri={uri}
        size={size}
        open={open}
        onOpenChange={setOpen}
        returnFocusTo={previewButton}
      />
    </div>
  );
}
