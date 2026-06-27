import { useRef, useState } from "react";
import {
  ChevronLeft,
  Download,
  File as FileIcon,
  Folder,
  Loader2,
  Trash2,
  Upload,
} from "lucide-react";

import {
  type StorageConn,
  type StorageObject,
  downloadObject,
  useDeleteObject,
  useStorageObjects,
  useUploadObject,
} from "@/api/storage";
import { ApiError } from "@/api/client";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";

/** Human-readable byte size, e.g. "1.2 KB". */
function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
  const value = bytes / Math.pow(1024, i);
  return `${i === 0 ? value : value.toFixed(1)} ${units[i]}`;
}

function formatDate(iso: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  return d.toLocaleString();
}

/** Last non-empty path segment of a key or folder path (handles trailing "/"). */
function lastSegment(path: string): string {
  const parts = path.replace(/\/$/, "").split("/");
  return parts[parts.length - 1] || path;
}

/** Parent prefix of the current prefix (or "" at root). */
function parentPrefix(prefix: string): string {
  const trimmed = prefix.replace(/\/$/, "");
  const idx = trimmed.lastIndexOf("/");
  return idx === -1 ? "" : trimmed.slice(0, idx + 1);
}

export default function FileBrowser({
  ws,
  conn,
  onClose,
}: {
  ws: string;
  conn: StorageConn;
  onClose?: () => void;
}) {
  const [prefix, setPrefix] = useState("");
  const [deleteTarget, setDeleteTarget] = useState<StorageObject | null>(null);

  const listing = useStorageObjects(ws, conn.id, prefix);
  const uploadMutation = useUploadObject(ws, conn.id);
  const deleteMutation = useDeleteObject(ws, conn.id);

  const fileInputRef = useRef<HTMLInputElement>(null);

  async function handleFileSelected(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    // Reset the input so re-selecting the same file fires onChange again.
    e.target.value = "";
    if (!file) return;
    const bytes = await file.arrayBuffer();
    uploadMutation.mutate({ key: prefix + file.name, bytes });
  }

  // Breadcrumb segments from current prefix.
  const segments = prefix.replace(/\/$/, "").split("/").filter(Boolean);

  return (
    <section>
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            {onClose && (
              <Button
                variant="ghost"
                size="sm"
                onClick={onClose}
                aria-label="Back to connections"
              >
                <ChevronLeft className="h-4 w-4" />
                Back
              </Button>
            )}
            <h2 className="truncate text-base font-semibold">{conn.name}</h2>
          </div>
          <p className="text-sm text-muted-foreground">
            Browse files in <span className="font-mono">{conn.bucket}</span>
          </p>
        </div>
        <div className="flex items-center gap-2">
          <input
            ref={fileInputRef}
            type="file"
            className="hidden"
            aria-label="Upload file"
            onChange={handleFileSelected}
          />
          <Button
            onClick={() => fileInputRef.current?.click()}
            disabled={uploadMutation.isPending}
          >
            {uploadMutation.isPending ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <Upload className="h-4 w-4" />
            )}
            Upload
          </Button>
        </div>
      </div>

      {/* Breadcrumb */}
      <nav className="mt-4 flex flex-wrap items-center gap-1 text-sm" aria-label="Breadcrumb">
        <button
          type="button"
          className="text-muted-foreground hover:text-foreground"
          onClick={() => setPrefix("")}
        >
          root
        </button>
        {segments.map((seg, i) => {
          const target = segments.slice(0, i + 1).join("/") + "/";
          return (
            <span key={target} className="flex items-center gap-1">
              <span className="text-muted-foreground/50">/</span>
              <button
                type="button"
                className="text-muted-foreground hover:text-foreground"
                onClick={() => setPrefix(target)}
              >
                {seg}
              </button>
            </span>
          );
        })}
        {prefix !== "" && (
          <Button
            variant="ghost"
            size="sm"
            className="ml-2 text-muted-foreground"
            onClick={() => setPrefix(parentPrefix(prefix))}
            aria-label="Up one level"
          >
            <ChevronLeft className="h-4 w-4" />
            Up
          </Button>
        )}
      </nav>

      <div className="mt-4">
        {listing.isLoading ? (
          <ListSkeleton rows={4} cols={3} />
        ) : listing.isError ? (
          (listing.error as ApiError | null)?.status === 403 ? (
            <Alert>
              <Folder className="h-4 w-4" />
              <AlertTitle>Insufficient permissions</AlertTitle>
              <AlertDescription>
                Requires workspace-admin to browse storage.
              </AlertDescription>
            </Alert>
          ) : (
            <Alert variant="destructive">
              <AlertTitle>Failed to load files</AlertTitle>
              <AlertDescription>Something went wrong. Please try again.</AlertDescription>
            </Alert>
          )
        ) : listing.data &&
          listing.data.folders.length === 0 &&
          listing.data.objects.length === 0 ? (
          <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
            <p>Empty</p>
          </div>
        ) : (
          <Card className="divide-y divide-border p-0">
            {listing.data?.folders.map((folder) => (
              <button
                key={folder}
                type="button"
                className="flex w-full items-center gap-3 px-4 py-2.5 text-left text-sm hover:bg-muted/50"
                onClick={() => setPrefix(folder)}
              >
                <Folder className="h-4 w-4 shrink-0 text-muted-foreground" />
                <span className="font-medium">{lastSegment(folder)}</span>
              </button>
            ))}
            {listing.data?.objects.map((obj) => (
              <div
                key={obj.key}
                className="flex items-center gap-3 px-4 py-2.5 text-sm"
              >
                <FileIcon className="h-4 w-4 shrink-0 text-muted-foreground" />
                <span className="min-w-0 flex-1 truncate font-medium">
                  {lastSegment(obj.key)}
                </span>
                <span className="w-20 shrink-0 text-right text-muted-foreground">
                  {formatBytes(obj.size)}
                </span>
                <span className="hidden w-40 shrink-0 text-right text-muted-foreground sm:inline">
                  {formatDate(obj.last_modified)}
                </span>
                <div className="flex shrink-0 items-center gap-1">
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-muted-foreground"
                    onClick={() => downloadObject(ws, conn.id, obj.key)}
                    aria-label={`Download ${lastSegment(obj.key)}`}
                  >
                    <Download className="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-muted-foreground hover:text-destructive"
                    onClick={() => setDeleteTarget(obj)}
                    aria-label={`Delete ${lastSegment(obj.key)}`}
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </div>
              </div>
            ))}
          </Card>
        )}
      </div>

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={
          deleteTarget
            ? `Delete «${lastSegment(deleteTarget.key)}»?`
            : "Delete object?"
        }
        description="This object will be permanently removed from the bucket."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget.key);
        }}
      />
    </section>
  );
}
