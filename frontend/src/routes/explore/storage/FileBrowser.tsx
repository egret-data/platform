import { useRef, useState } from "react";
import {
  Braces,
  ChevronLeft,
  Database,
  File as FileIcon,
  FileArchive,
  FileCode,
  FileText,
  Folder,
  FolderPlus,
  Image as ImageIcon,
  Loader2,
  MoreVertical,
  RefreshCw,
  Sheet,
  Table2,
  Upload,
  type LucideIcon,
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

/** Pick a Lucide icon for a file based on its extension. */
function fileIcon(name: string): LucideIcon {
  const dot = name.lastIndexOf(".");
  const ext = dot === -1 ? "" : name.slice(dot + 1).toLowerCase();
  switch (ext) {
    case "csv":
    case "tsv":
      return Table2;
    case "parquet":
    case "orc":
    case "avro":
      return Database;
    case "xlsx":
    case "xls":
      return Sheet;
    case "json":
    case "ndjson":
      return Braces;
    case "txt":
    case "md":
    case "log":
    case "pdf":
      return FileText;
    case "png":
    case "jpg":
    case "jpeg":
    case "gif":
    case "svg":
    case "webp":
      return ImageIcon;
    case "zip":
    case "gz":
    case "tar":
    case "7z":
      return FileArchive;
    case "py":
    case "sql":
    case "js":
    case "ts":
    case "tsx":
    case "rs":
    case "yaml":
    case "yml":
    case "toml":
      return FileCode;
    default:
      return FileIcon;
  }
}

/** Human label for a storage provider id. */
function providerLabel(provider: string): string {
  return provider === "s3" ? "S3 / MinIO" : provider;
}

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

  const meta = [
    conn.purpose || null,
    providerLabel(conn.provider),
    conn.bucket,
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <section>
      <Card className="overflow-hidden p-0">
        {/* 1 — Connection info header */}
        <div className="flex items-start justify-between gap-4 border-b border-border p-4">
          <div className="flex min-w-0 items-start gap-3">
            {onClose && (
              <Button
                variant="ghost"
                size="sm"
                onClick={onClose}
                aria-label="Back to connections"
                className="-ml-2 text-muted-foreground"
              >
                <ChevronLeft className="h-4 w-4" />
                Back
              </Button>
            )}
            <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-foreground/90 text-background">
              <Database className="h-5 w-5" />
            </div>
            <div className="min-w-0">
              <h2 className="truncate text-base font-semibold">{conn.name}</h2>
              <p className="truncate text-sm text-muted-foreground">{meta}</p>
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <input
              ref={fileInputRef}
              type="file"
              className="hidden"
              aria-label="Upload file"
              onChange={handleFileSelected}
            />
            <Button
              variant="outline"
              onClick={() => listing.refetch()}
              aria-label="Refresh"
            >
              <RefreshCw className="h-4 w-4" />
              Refresh
            </Button>
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

        {/* 2 — Breadcrumb bar */}
        <div className="border-b border-border p-3">
          <nav
            className="flex flex-wrap items-center gap-2 rounded-md bg-muted/40 px-3 py-2 text-sm"
            aria-label="Breadcrumb"
          >
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-muted-foreground"
              onClick={() => setPrefix(parentPrefix(prefix))}
              disabled={prefix === ""}
              aria-label="Up one level"
            >
              <ChevronLeft className="h-4 w-4" />
            </Button>
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
                <span key={target} className="flex items-center gap-2">
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
          </nav>
        </div>

        {/* 3 — Object table */}
        <div>
          {listing.isLoading ? (
            <div className="p-4">
              <ListSkeleton rows={4} cols={3} />
            </div>
          ) : listing.isError ? (
            <div className="p-4">
              {(listing.error as ApiError | null)?.status === 403 ? (
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
                  <AlertDescription>
                    Something went wrong. Please try again.
                  </AlertDescription>
                </Alert>
              )}
            </div>
          ) : listing.data &&
            listing.data.folders.length === 0 &&
            listing.data.objects.length === 0 ? (
            <div className="flex flex-col items-center justify-center gap-2 p-12 text-center text-muted-foreground">
              <Folder className="h-10 w-10 text-muted-foreground/60" />
              <p className="font-medium">This folder is empty</p>
              <p className="text-sm">Upload files or create a folder.</p>
            </div>
          ) : (
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-border text-xs text-muted-foreground">
                  <th className="w-9 px-4 py-2" aria-hidden />
                  <th className="px-2 py-2 text-left font-medium">Name</th>
                  <th className="hidden px-4 py-2 text-left font-medium sm:table-cell">
                    Last Modified
                  </th>
                  <th className="px-4 py-2 text-right font-medium">Size</th>
                  <th className="w-12 px-4 py-2" aria-hidden />
                </tr>
              </thead>
              <tbody>
                {listing.data?.folders.map((folder) => (
                  <tr
                    key={folder}
                    className="border-b border-border last:border-0 hover:bg-muted/50"
                  >
                    <td className="px-4 py-2.5 align-middle">
                      <Folder
                        className="h-4 w-4 text-amber-500"
                        data-testid="folder-icon"
                      />
                    </td>
                    <td className="px-2 py-2.5 align-middle">
                      <button
                        type="button"
                        className="truncate text-left font-medium hover:underline"
                        onClick={() => setPrefix(folder)}
                      >
                        {lastSegment(folder)}
                      </button>
                    </td>
                    <td className="hidden px-4 py-2.5 align-middle text-muted-foreground sm:table-cell" />
                    <td className="px-4 py-2.5 text-right align-middle text-muted-foreground">
                      —
                    </td>
                    <td className="px-4 py-2.5 text-right align-middle">
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button
                            variant="ghost"
                            size="sm"
                            className="h-8 w-8 p-0 text-muted-foreground"
                            aria-label={`Actions for ${lastSegment(folder)}`}
                          >
                            <MoreVertical className="h-4 w-4" />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          <DropdownMenuItem onClick={() => setPrefix(folder)}>
                            Open
                          </DropdownMenuItem>
                          <DropdownMenuItem
                            onClick={() =>
                              openMove({ kind: "folder", from: folder })
                            }
                          >
                            Move / Rename
                          </DropdownMenuItem>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem
                            className="text-destructive focus:text-destructive"
                            onClick={() =>
                              setDeleteTarget({ kind: "folder", path: folder })
                            }
                          >
                            Delete
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </td>
                  </tr>
                ))}
                {listing.data?.objects.map((obj) => {
                  const name = lastSegment(obj.key);
                  const Icon = fileIcon(name);
                  return (
                    <tr
                      key={obj.key}
                      className="border-b border-border last:border-0 hover:bg-muted/50"
                    >
                      <td className="px-4 py-2.5 align-middle">
                        <Icon
                          className="h-4 w-4 text-muted-foreground"
                          data-testid="file-icon"
                        />
                      </td>
                      <td className="px-2 py-2.5 align-middle">
                        <span className="truncate font-medium">{name}</span>
                      </td>
                      <td className="hidden px-4 py-2.5 align-middle text-muted-foreground sm:table-cell">
                        {formatDate(obj.last_modified)}
                      </td>
                      <td className="px-4 py-2.5 text-right align-middle text-muted-foreground">
                        {formatBytes(obj.size)}
                      </td>
                      <td className="px-4 py-2.5 text-right align-middle">
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button
                              variant="ghost"
                              size="sm"
                              className="h-8 w-8 p-0 text-muted-foreground"
                              aria-label={`Actions for ${name}`}
                            >
                              <MoreVertical className="h-4 w-4" />
                            </Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent align="end">
                            <DropdownMenuItem
                              onClick={() =>
                                downloadObject(ws, conn.id, obj.key)
                              }
                            >
                              Download
                            </DropdownMenuItem>
                            <DropdownMenuItem
                              onClick={() =>
                                openMove({ kind: "file", from: obj.key })
                              }
                            >
                              Move / Rename
                            </DropdownMenuItem>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem
                              className="text-destructive focus:text-destructive"
                              onClick={() =>
                                setDeleteTarget({ kind: "file", key: obj.key })
                              }
                            >
                              Delete
                            </DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </div>
      </Card>

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
