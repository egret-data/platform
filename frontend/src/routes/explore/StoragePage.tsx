import { useState } from "react";
import { useParams, useSearchParams } from "react-router-dom";
import { FolderOpen, HardDrive, Loader2, Pencil, Plus, Trash2 } from "lucide-react";

import {
  type StorageConn,
  type StorageConnInput,
  useCreateStorageConn,
  useDeleteStorageConn,
  useStorageConns,
  useUpdateStorageConn,
} from "@/api/storage";
import { ApiError } from "@/api/client";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import FileBrowser from "@/routes/explore/storage/FileBrowser";

export default function StoragePage() {
  const ws = useParams().workspace!;
  const conns = useStorageConns(ws);
  const deleteMutation = useDeleteStorageConn(ws);

  const [addOpen, setAddOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<StorageConn | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<StorageConn | null>(null);

  // Browse state lives in the URL so it's deep-linkable and the back button
  // walks folder history: `?conn=<id>&path=<prefix>`.
  const [searchParams, setSearchParams] = useSearchParams();
  const browseConnId = searchParams.get("conn");
  const browsePath = searchParams.get("path") ?? "";

  function openBrowse(conn: StorageConn) {
    setSearchParams({ conn: conn.id });
  }
  function closeBrowse() {
    setSearchParams({});
  }
  function changeBrowsePath(path: string) {
    if (!browseConnId) return;
    setSearchParams(path ? { conn: browseConnId, path } : { conn: browseConnId });
  }

  if (conns.isLoading) {
    return (
      <section>
        <SectionHeader onAdd={() => setAddOpen(true)} />
        <div className="mt-4">
          <ListSkeleton rows={3} cols={5} />
        </div>
      </section>
    );
  }

  if (conns.isError) {
    const status = (conns.error as ApiError | null)?.status;
    if (status === 403) {
      return (
        <Alert>
          <HardDrive className="h-4 w-4" />
          <AlertTitle>Insufficient permissions</AlertTitle>
          <AlertDescription>
            Requires workspace-admin to manage storage.
          </AlertDescription>
        </Alert>
      );
    }
    return (
      <Alert variant="destructive">
        <AlertTitle>Failed to load storage connections</AlertTitle>
        <AlertDescription>Something went wrong. Please try again.</AlertDescription>
      </Alert>
    );
  }

  const rows = conns.data ?? [];

  // Master/detail: a `?conn=<id>` in the URL selects a connection to browse.
  // Resolved against the loaded list so deep links / refresh work; an unknown
  // id (e.g. a deleted connection) falls back to the registry list.
  const browseTarget = browseConnId
    ? (rows.find((c) => c.id === browseConnId) ?? null)
    : null;
  if (browseTarget) {
    return (
      <FileBrowser
        ws={ws}
        conn={browseTarget}
        prefix={browsePath}
        onPrefixChange={changeBrowsePath}
        onClose={closeBrowse}
      />
    );
  }

  return (
    <section>
      <SectionHeader onAdd={() => setAddOpen(true)} />

      <div className="mt-4">
        {rows.length === 0 ? (
          <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
            <p>No storage connections yet.</p>
          </div>
        ) : (
          <table className="w-full text-sm">
            <thead className="text-left text-muted-foreground">
              <tr className="border-b border-border">
                <th className="py-2 pr-4 font-medium">Name</th>
                <th className="py-2 pr-4 font-medium">Purpose</th>
                <th className="py-2 pr-4 font-medium">Provider</th>
                <th className="py-2 pr-4 font-medium">Bucket</th>
                <th className="py-2 pr-4 font-medium">Endpoint</th>
                <th className="py-2 pr-4 font-medium">Secret</th>
                <th className="py-2 font-medium sr-only">Actions</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((conn) => (
                <tr key={conn.id} className="border-b border-border last:border-b-0">
                  <td className="py-2 pr-4 font-medium">{conn.name}</td>
                  <td className="py-2 pr-4 text-muted-foreground">
                    {conn.purpose ?? <span className="text-muted-foreground/50">—</span>}
                  </td>
                  <td className="py-2 pr-4">
                    <Badge variant="secondary">{conn.provider}</Badge>
                  </td>
                  <td className="py-2 pr-4 font-mono text-muted-foreground">{conn.bucket}</td>
                  <td className="py-2 pr-4 text-muted-foreground">
                    {conn.endpoint ?? "AWS default"}
                  </td>
                  <td className="py-2 pr-4">
                    <Badge variant={conn.has_secret ? "secondary" : "outline"}>
                      {conn.has_secret ? "Configured" : "Not set"}
                    </Badge>
                  </td>
                  <td className="py-2 text-right">
                    <div className="flex items-center justify-end gap-1">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-muted-foreground"
                        onClick={() => openBrowse(conn)}
                        aria-label={`Browse files for ${conn.name}`}
                      >
                        <FolderOpen className="h-4 w-4" />
                        Browse
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-muted-foreground"
                        onClick={() => setEditTarget(conn)}
                        aria-label={`Edit ${conn.name}`}
                      >
                        <Pencil className="h-4 w-4" />
                        Edit
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-muted-foreground hover:text-destructive"
                        onClick={() => setDeleteTarget(conn)}
                        aria-label={`Delete ${conn.name}`}
                      >
                        <Trash2 className="h-4 w-4" />
                        Delete
                      </Button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {/* Add dialog */}
      <StorageConnDialog
        ws={ws}
        open={addOpen}
        onOpenChange={setAddOpen}
        editConn={null}
      />

      {/* Edit dialog — key forces remount when target changes so useState initializers re-run */}
      <StorageConnDialog
        key={editTarget?.id ?? "__edit-closed"}
        ws={ws}
        open={editTarget !== null}
        onOpenChange={(o) => {
          if (!o) setEditTarget(null);
        }}
        editConn={editTarget}
      />

      {/* Delete confirm */}
      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete «${deleteTarget.name}»?` : "Delete storage connection?"}
        description="This storage connection will be removed. Any catalogs or notebooks that reference it may break."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget.id);
        }}
      />
    </section>
  );
}

function SectionHeader({ onAdd }: { onAdd: () => void }) {
  return (
    <div className="flex items-start justify-between gap-4">
      <div>
        <h2 className="text-base font-semibold">Storage</h2>
        <p className="text-sm text-muted-foreground">
          Storage connections this workspace can use (catalogs, file storage&hellip;).
        </p>
      </div>
      <Button onClick={onAdd}>
        <Plus className="h-4 w-4" />
        Add connection
      </Button>
    </div>
  );
}

function StorageConnDialog({
  ws,
  open,
  onOpenChange,
  editConn,
}: {
  ws: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editConn: StorageConn | null;
}) {
  const isEdit = editConn !== null;
  const createMutation = useCreateStorageConn(ws);
  const updateMutation = useUpdateStorageConn(ws);

  const [name, setName] = useState(editConn?.name ?? "");
  const [purpose, setPurpose] = useState(editConn?.purpose ?? "");
  const [provider, setProvider] = useState(editConn?.provider ?? "s3");
  const [endpoint, setEndpoint] = useState(editConn?.endpoint ?? "");
  const [region, setRegion] = useState(editConn?.region ?? "");
  const [bucket, setBucket] = useState(editConn?.bucket ?? "");
  const [keyPrefix, setKeyPrefix] = useState(editConn?.key_prefix ?? "");
  const [accessKey, setAccessKey] = useState(editConn?.access_key ?? "");
  const [secretKey, setSecretKey] = useState("");
  const [pathStyle, setPathStyle] = useState(editConn?.path_style ?? true);
  const [error, setError] = useState<string | null>(null);

  // When the dialog opens with a different target, reset fields
  function resetToConn(conn: StorageConn | null) {
    setName(conn?.name ?? "");
    setPurpose(conn?.purpose ?? "");
    setProvider(conn?.provider ?? "s3");
    setEndpoint(conn?.endpoint ?? "");
    setRegion(conn?.region ?? "");
    setBucket(conn?.bucket ?? "");
    setKeyPrefix(conn?.key_prefix ?? "");
    setAccessKey(conn?.access_key ?? "");
    setSecretKey("");
    setPathStyle(conn?.path_style ?? true);
    setError(null);
    createMutation.reset();
    updateMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) resetToConn(null);
    onOpenChange(next);
  }

  const isPending = createMutation.isPending || updateMutation.isPending;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);

    const input: StorageConnInput = {
      name: name.trim(),
      purpose: purpose.trim().length > 0 ? purpose.trim() : null,
      provider,
      endpoint: endpoint.trim().length > 0 ? endpoint.trim() : null,
      region: region.trim(),
      bucket: bucket.trim(),
      key_prefix: keyPrefix.trim().length > 0 ? keyPrefix.trim() : null,
      access_key: accessKey.trim(),
      // On create: always send secret_key. On edit: only if typed.
      secret_key: secretKey.length > 0 ? secretKey : null,
      path_style: pathStyle,
    };

    try {
      if (isEdit && editConn) {
        await updateMutation.mutateAsync({ id: editConn.id, input });
      } else {
        await createMutation.mutateAsync(input);
      }
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to save storage connection.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>
              {isEdit ? "Edit storage connection" : "Add storage connection"}
            </DialogTitle>
            <DialogDescription>
              {isEdit
                ? "Update the connection details. Leave secret key blank to keep the current secret."
                : "Connect an S3-compatible object store. Egret validates reachability on save."}
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="sc-name">Name</Label>
              <Input
                id="sc-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. prod-lakehouse"
                required
                autoFocus={!isEdit}
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="sc-purpose">Purpose</Label>
              <Input
                id="sc-purpose"
                value={purpose}
                onChange={(e) => setPurpose(e.target.value)}
                placeholder="Optional (e.g. catalog, notebooks)"
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="sc-provider">Provider</Label>
              <Select value={provider} onValueChange={setProvider}>
                <SelectTrigger id="sc-provider" className="w-full">
                  <SelectValue placeholder="Select a provider" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="s3">S3 / MinIO</SelectItem>
                  <SelectItem value="gcs" disabled>
                    Google Cloud Storage — coming soon
                  </SelectItem>
                  <SelectItem value="azure" disabled>
                    Azure Blob — coming soon
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="sc-endpoint">Endpoint</Label>
              <Input
                id="sc-endpoint"
                value={endpoint}
                onChange={(e) => setEndpoint(e.target.value)}
                placeholder="Leave blank for AWS (e.g. https://minio.corp.local)"
              />
            </div>

            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-1.5">
                <Label htmlFor="sc-region">Region</Label>
                <Input
                  id="sc-region"
                  value={region}
                  onChange={(e) => setRegion(e.target.value)}
                  placeholder="e.g. us-east-1"
                  required
                />
              </div>

              <div className="space-y-1.5">
                <Label htmlFor="sc-bucket">Bucket</Label>
                <Input
                  id="sc-bucket"
                  value={bucket}
                  onChange={(e) => setBucket(e.target.value)}
                  placeholder="e.g. my-data-lake"
                  required
                />
              </div>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="sc-key-prefix">Key prefix</Label>
              <Input
                id="sc-key-prefix"
                value={keyPrefix}
                onChange={(e) => setKeyPrefix(e.target.value)}
                placeholder="Optional — e.g. workspaces/eng"
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="sc-access-key">Access key</Label>
              <Input
                id="sc-access-key"
                value={accessKey}
                onChange={(e) => setAccessKey(e.target.value)}
                placeholder="Access key ID"
                required
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="sc-secret-key">
                Secret key{isEdit ? "" : " *"}
              </Label>
              <Input
                id="sc-secret-key"
                type="password"
                value={secretKey}
                onChange={(e) => setSecretKey(e.target.value)}
                placeholder={
                  isEdit ? "Leave blank to keep current" : "Secret access key"
                }
                required={!isEdit}
                autoComplete="new-password"
              />
            </div>

            <div className="flex items-center gap-3">
              <input
                type="checkbox"
                id="sc-path-style"
                checked={pathStyle}
                onChange={(e) => setPathStyle(e.target.checked)}
                className="h-4 w-4 rounded border-border accent-primary cursor-pointer"
              />
              <Label htmlFor="sc-path-style">
                Force path-style URL (s3.region.amazonaws.com/bucket)
              </Label>
            </div>

            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
          </div>

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => handleOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={isPending}>
              {isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              {isEdit ? "Save changes" : "Add connection"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
