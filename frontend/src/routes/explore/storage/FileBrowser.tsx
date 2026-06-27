import { useRef, useState } from "react";
import {
  ChevronLeft,
  Download,
  File as FileIcon,
  Folder,
  FolderInput,
  FolderPlus,
  Loader2,
  Trash2,
  Upload,
} from "lucide-react";

import {
  type StorageConn,
  downloadObject,
  useCreateFolder,
  useDeleteObject,
  useMoveObject,
  useStorageObjects,
  useUploadObject,
} from "@/api/storage";
import { ApiError } from "@/api/client";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

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

type DeleteTarget =
  | { kind: "file"; key: string }
  | { kind: "folder"; path: string };

/** A move/rename target carries enough info to compute trailing-slash semantics. */
interface MoveTarget {
  kind: "file" | "folder";
  /** Full relative key/path of the item. Folders end with "/". */
  from: string;
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
  const [deleteTarget, setDeleteTarget] = useState<DeleteTarget | null>(null);

  const [newFolderOpen, setNewFolderOpen] = useState(false);
  const [newFolderName, setNewFolderName] = useState("");
  const [newFolderError, setNewFolderError] = useState<string | null>(null);

  const [moveTarget, setMoveTarget] = useState<MoveTarget | null>(null);
  const [moveValue, setMoveValue] = useState("");
  const [moveError, setMoveError] = useState<string | null>(null);

  const listing = useStorageObjects(ws, conn.id, prefix);
  const uploadMutation = useUploadObject(ws, conn.id);
  const deleteMutation = useDeleteObject(ws, conn.id);
  const createFolderMutation = useCreateFolder(ws, conn.id);
  const moveMutation = useMoveObject(ws, conn.id);

  const fileInputRef = useRef<HTMLInputElement>(null);

  async function handleFileSelected(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    // Reset the input so re-selecting the same file fires onChange again.
    e.target.value = "";
    if (!file) return;
    const bytes = await file.arrayBuffer();
    uploadMutation.mutate({ key: prefix + file.name, bytes });
  }

  function openNewFolder() {
    setNewFolderName("");
    setNewFolderError(null);
    setNewFolderOpen(true);
  }

  function submitNewFolder() {
    const name = newFolderName.trim();
    if (!name) {
      setNewFolderError("Folder name cannot be empty.");
      return;
    }
    if (name.includes("/")) {
      setNewFolderError("Folder name cannot contain «/».");
      return;
    }
    // Server appends the trailing slash; we pass current prefix + name.
    createFolderMutation.mutate(prefix + name, {
      onSuccess: () => setNewFolderOpen(false),
      onError: (err) =>
        setNewFolderError(err.message || "Failed to create folder."),
    });
  }

  function openMove(target: MoveTarget) {
    setMoveTarget(target);
    // Prefill with the full relative key/path so the user can edit it freely.
    setMoveValue(target.from);
    setMoveError(null);
  }

  function submitMove() {
    if (!moveTarget) return;
    const raw = moveValue.trim();
    if (!raw) {
      setMoveError("Path cannot be empty.");
      return;
    }
    let from = moveTarget.from;
    let to = raw;
    if (moveTarget.kind === "folder") {
      // Both endpoints must end with "/" for a folder move.
      from = from.endsWith("/") ? from : `${from}/`;
      to = to.endsWith("/") ? to : `${to}/`;
    } else {
      // For a file the destination must NOT end with "/" (a stray slash orphans
      // a marker). The source is the plain file key.
      to = to.replace(/\/+$/, "");
    }
    if (from === to) {
      setMoveError("Source and destination are the same.");
      return;
    }
    moveMutation.mutate(
      { from, to },
      {
        onSuccess: () => setMoveTarget(null),
        onError: (err) => setMoveError(err.message || "Failed to move."),
      },
    );
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
          <Button variant="outline" onClick={openNewFolder}>
            <FolderPlus className="h-4 w-4" />
            New folder
          </Button>
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
              <div
                key={folder}
                className="flex items-center gap-3 px-4 py-2.5 text-sm hover:bg-muted/50"
              >
                <button
                  type="button"
                  className="flex min-w-0 flex-1 items-center gap-3 text-left"
                  onClick={() => setPrefix(folder)}
                >
                  <Folder className="h-4 w-4 shrink-0 text-muted-foreground" />
                  <span className="truncate font-medium">
                    {lastSegment(folder)}
                  </span>
                </button>
                <div className="flex shrink-0 items-center gap-1">
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-muted-foreground"
                    onClick={(e) => {
                      e.stopPropagation();
                      openMove({ kind: "folder", from: folder });
                    }}
                    aria-label={`Move or rename ${lastSegment(folder)}`}
                  >
                    <FolderInput className="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-muted-foreground hover:text-destructive"
                    onClick={(e) => {
                      e.stopPropagation();
                      setDeleteTarget({ kind: "folder", path: folder });
                    }}
                    aria-label={`Delete folder ${lastSegment(folder)}`}
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </div>
              </div>
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
                    className="text-muted-foreground"
                    onClick={() => openMove({ kind: "file", from: obj.key })}
                    aria-label={`Move or rename ${lastSegment(obj.key)}`}
                  >
                    <FolderInput className="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-muted-foreground hover:text-destructive"
                    onClick={() => setDeleteTarget({ kind: "file", key: obj.key })}
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

      {/* New folder dialog */}
      <Dialog
        open={newFolderOpen}
        onOpenChange={(o) => {
          if (!o) setNewFolderError(null);
          setNewFolderOpen(o);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>New folder</DialogTitle>
            <DialogDescription>
              Create an empty folder in the current location.
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-2">
            <Label htmlFor="new-folder-name">Folder name</Label>
            <Input
              id="new-folder-name"
              value={newFolderName}
              onChange={(e) => {
                setNewFolderName(e.target.value);
                setNewFolderError(null);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") submitNewFolder();
              }}
              placeholder="my-folder"
              autoFocus
            />
            {newFolderError && (
              <p className="text-sm text-destructive">{newFolderError}</p>
            )}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setNewFolderOpen(false)}>
              Cancel
            </Button>
            <Button
              onClick={submitNewFolder}
              disabled={createFolderMutation.isPending}
            >
              {createFolderMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Create
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Move / rename dialog (shared for files & folders) */}
      <Dialog
        open={moveTarget !== null}
        onOpenChange={(o) => {
          if (!o) {
            setMoveTarget(null);
            setMoveError(null);
          }
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              Move or rename {moveTarget?.kind === "folder" ? "folder" : "file"}
            </DialogTitle>
            <DialogDescription>
              Edit the destination path. Move within the bucket or rename in
              place.
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-2">
            <Label htmlFor="move-path">New path</Label>
            <Input
              id="move-path"
              value={moveValue}
              onChange={(e) => {
                setMoveValue(e.target.value);
                setMoveError(null);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") submitMove();
              }}
              autoFocus
            />
            {moveError && (
              <p className="text-sm text-destructive">{moveError}</p>
            )}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setMoveTarget(null)}>
              Cancel
            </Button>
            <Button onClick={submitMove} disabled={moveMutation.isPending}>
              {moveMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Move
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={
          deleteTarget
            ? deleteTarget.kind === "folder"
              ? `Delete «${lastSegment(deleteTarget.path)}»?`
              : `Delete «${lastSegment(deleteTarget.key)}»?`
            : "Delete object?"
        }
        description={
          deleteTarget?.kind === "folder"
            ? `Delete folder «${lastSegment(
                deleteTarget.path,
              )}» and ALL its contents?`
            : "This object will be permanently removed from the bucket."
        }
        confirmText="delete"
        onConfirm={() => {
          if (!deleteTarget) return;
          if (deleteTarget.kind === "folder") {
            deleteMutation.mutate({ key: deleteTarget.path, recursive: true });
          } else {
            deleteMutation.mutate({ key: deleteTarget.key });
          }
        }}
      />
    </section>
  );
}
