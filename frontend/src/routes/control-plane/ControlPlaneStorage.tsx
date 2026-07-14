import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { FolderOpen, HardDrive, Loader2, Pencil, Plus, Trash2 } from "lucide-react";

import {
  type Storage,
  type StorageInput,
  createStorage,
  updateStorage,
  deleteStorage,
  listSecrets,
  listStorage,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

const COLUMNS = ["Name", "Purpose", "Provider", "Bucket", "Endpoint", "Secret", ""];

export const storageQueryKey = ["control-plane", "storage"] as const;
export const secretsQueryKey = ["control-plane", "secrets"] as const;

/**
 * Control Plane → Storage
 *
 * Platform-admin registry of GLOBAL storage connections. Lists connections,
 * adds one (referencing or minting a global secret), and deletes them.
 */
export default function ControlPlaneStorage() {
  const conns = useQuery<Storage[], ApiError>({
    queryKey: storageQueryKey,
    queryFn: listStorage,
    retry: false,
  });
  const queryClient = useQueryClient();
  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: deleteStorage,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: storageQueryKey }),
  });

  const [addOpen, setAddOpen] = useState(false);
  const [editing, setEditing] = useState<Storage | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Storage | null>(null);

  const rows = conns.data ?? [];

  return (
    <Card data-testid="storage-frame">
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle className="flex items-center gap-2 text-lg">
              <HardDrive className="h-5 w-5 text-muted-foreground" />
              Storage
            </CardTitle>
            <CardDescription>
              Global storage connections (S3-compatible object stores) available
              to catalogs and workspaces across the platform.
            </CardDescription>
          </div>
          <Button
            onClick={() => {
              setEditing(null);
              setAddOpen(true);
            }}
          >
            <Plus className="h-4 w-4" />
            Add connection
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        <SummaryStrip items={[{ label: "connections", value: rows.length }]} />
        {conns.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              Failed to load storage connections. Please try again.
            </AlertDescription>
          </Alert>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                {COLUMNS.map((col) => (
                  <TableHead
                    key={col || "actions"}
                    className={col === "" ? "text-right" : undefined}
                  >
                    {col || <span className="sr-only">Actions</span>}
                  </TableHead>
                ))}
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.length === 0 ? (
                <TableRow className="hover:bg-transparent">
                  <TableCell
                    colSpan={COLUMNS.length}
                    className="py-10 text-center text-muted-foreground"
                  >
                    {conns.isLoading
                      ? "Loading…"
                      : "No storage connections yet — add one to get started."}
                  </TableCell>
                </TableRow>
              ) : (
                rows.map((conn) => (
                  <TableRow key={conn.id}>
                    <TableCell className="font-medium">{conn.name}</TableCell>
                    <TableCell className="text-muted-foreground">
                      {conn.purpose ?? "—"}
                    </TableCell>
                    <TableCell>
                      <Badge variant="secondary">{conn.provider}</Badge>
                    </TableCell>
                    <TableCell className="font-mono text-muted-foreground">
                      {conn.bucket}
                    </TableCell>
                    <TableCell className="text-muted-foreground">
                      {conn.endpoint ?? "AWS default"}
                    </TableCell>
                    <TableCell>
                      <Badge variant={conn.secret_ref ? "secondary" : "outline"}>
                        {conn.secret_ref ? "Configured" : "Not set"}
                      </Badge>
                    </TableCell>
                    <TableCell className="text-right">
                      <Button variant="ghost" size="sm" asChild>
                        <Link to={`/control-plane/storage/${conn.id}/browse`} aria-label={`Browse ${conn.name}`}>
                          <FolderOpen className="h-4 w-4" />
                          Browse
                        </Link>
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => {
                          setEditing(conn);
                          setAddOpen(true);
                        }}
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
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        )}
      </CardContent>

      <AddStorageDialog open={addOpen} onOpenChange={setAddOpen} editing={editing} />

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={
          deleteTarget ? `Delete «${deleteTarget.name}»?` : "Delete storage connection?"
        }
        description="This storage connection will be removed. Any catalogs or workspaces that reference it may break."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget.id);
        }}
      />
    </Card>
  );
}

// Sentinel <Select> values that must not collide with a real secret id.
const NO_SECRET = "__none__";
const NEW_SECRET = "__new__";

function AddStorageDialog({
  open,
  onOpenChange,
  editing,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editing: Storage | null;
}) {
  const queryClient = useQueryClient();
  const secrets = useQuery({
    queryKey: secretsQueryKey,
    queryFn: listSecrets,
    retry: false,
  });
  const createMutation = useMutation<Storage, ApiError, StorageInput>({
    mutationFn: createStorage,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: storageQueryKey }),
  });
  const updateMutation = useMutation<Storage, ApiError, StorageInput>({
    mutationFn: (input) => updateStorage(editing!.id, input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: storageQueryKey }),
  });

  const [name, setName] = useState("");
  const [purpose, setPurpose] = useState("");
  const [provider, setProvider] = useState("s3");
  const [endpoint, setEndpoint] = useState("");
  const [region, setRegion] = useState("");
  const [bucket, setBucket] = useState("");
  const [keyPrefix, setKeyPrefix] = useState("");
  const [accessKey, setAccessKey] = useState("");
  const [pathStyle, setPathStyle] = useState(true);
  // Secret selection: a real secret id, or a sentinel.
  const [secretChoice, setSecretChoice] = useState<string>(NO_SECRET);
  const [newSecretName, setNewSecretName] = useState("");
  const [newSecretValue, setNewSecretValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    if (editing) {
      setName(editing.name);
      setPurpose(editing.purpose ?? "");
      setProvider(editing.provider);
      setEndpoint(editing.endpoint ?? "");
      setRegion(editing.region);
      setBucket(editing.bucket);
      setKeyPrefix(editing.key_prefix ?? "");
      setAccessKey(editing.access_key);
      setPathStyle(editing.path_style);
      setSecretChoice(editing.secret_ref ?? NO_SECRET);
      setError(null);
    } else {
      reset();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, editing]);

  function reset() {
    setName("");
    setPurpose("");
    setProvider("s3");
    setEndpoint("");
    setRegion("");
    setBucket("");
    setKeyPrefix("");
    setAccessKey("");
    setPathStyle(true);
    setSecretChoice(NO_SECRET);
    setNewSecretName("");
    setNewSecretValue("");
    setError(null);
    createMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);

    const input: StorageInput = {
      name: name.trim(),
      purpose: purpose.trim().length > 0 ? purpose.trim() : null,
      provider,
      endpoint: endpoint.trim().length > 0 ? endpoint.trim() : null,
      region: region.trim(),
      bucket: bucket.trim(),
      key_prefix: keyPrefix.trim().length > 0 ? keyPrefix.trim() : null,
      access_key: accessKey.trim(),
      path_style: pathStyle,
    };

    if (secretChoice === NEW_SECRET) {
      input.new_secret = { name: newSecretName.trim(), value: newSecretValue };
    } else if (secretChoice !== NO_SECRET) {
      input.secret_ref = secretChoice;
    }

    try {
      if (editing) {
        await updateMutation.mutateAsync(input);
      } else {
        await createMutation.mutateAsync(input);
      }
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to save storage connection.");
    }
  }

  const items = secrets.data ?? [];

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>{editing ? "Edit storage connection" : "Add storage connection"}</DialogTitle>
            <DialogDescription>
              {editing
                ? "Update the connection. Changing its secret rotates the credential on every warehouse using it."
                : "Register a global S3-compatible object store for the platform."}
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cps-name">Name</Label>
              <Input
                id="cps-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. prod-lakehouse"
                required
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cps-purpose">Purpose</Label>
              <Input
                id="cps-purpose"
                value={purpose}
                onChange={(e) => setPurpose(e.target.value)}
                placeholder="Optional (e.g. catalog, notebooks)"
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cps-provider">Provider</Label>
              <Select value={provider} onValueChange={setProvider}>
                <SelectTrigger id="cps-provider" className="w-full">
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
              <Label htmlFor="cps-endpoint">Endpoint</Label>
              <Input
                id="cps-endpoint"
                value={endpoint}
                onChange={(e) => setEndpoint(e.target.value)}
                placeholder="Leave blank for AWS (e.g. https://minio.corp.local)"
              />
            </div>

            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-1.5">
                <Label htmlFor="cps-region">Region</Label>
                <Input
                  id="cps-region"
                  value={region}
                  onChange={(e) => setRegion(e.target.value)}
                  placeholder="e.g. us-east-1"
                  required
                />
              </div>

              <div className="space-y-1.5">
                <Label htmlFor="cps-bucket">Bucket</Label>
                <Input
                  id="cps-bucket"
                  value={bucket}
                  onChange={(e) => setBucket(e.target.value)}
                  placeholder="e.g. my-data-lake"
                  required
                />
              </div>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cps-key-prefix">Key prefix</Label>
              <Input
                id="cps-key-prefix"
                value={keyPrefix}
                onChange={(e) => setKeyPrefix(e.target.value)}
                placeholder="Optional — e.g. workspaces/eng"
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cps-access-key">Access key</Label>
              <Input
                id="cps-access-key"
                value={accessKey}
                onChange={(e) => setAccessKey(e.target.value)}
                placeholder="Access key ID"
                required
              />
            </div>

            <div className="space-y-2">
              <Label htmlFor="cps-secret">Secret</Label>
              <Select value={secretChoice} onValueChange={setSecretChoice}>
                <SelectTrigger id="cps-secret" className="w-full">
                  <SelectValue placeholder="Select a secret" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={NO_SECRET}>No secret</SelectItem>
                  {items.map((s) => (
                    <SelectItem key={s.id} value={s.id}>
                      {s.name}
                    </SelectItem>
                  ))}
                  <SelectItem value={NEW_SECRET}>＋ New secret</SelectItem>
                </SelectContent>
              </Select>

              {secretChoice === NEW_SECRET && (
                <div className="space-y-3 pt-1">
                  <div className="space-y-1.5">
                    <Label htmlFor="cps-new-secret-name">New secret name</Label>
                    <Input
                      id="cps-new-secret-name"
                      value={newSecretName}
                      onChange={(e) => setNewSecretName(e.target.value)}
                      placeholder="e.g. prod-s3-secret"
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="cps-new-secret-value">New secret value</Label>
                    <Input
                      id="cps-new-secret-value"
                      type="password"
                      value={newSecretValue}
                      onChange={(e) => setNewSecretValue(e.target.value)}
                      placeholder="Enter a value"
                      autoComplete="new-password"
                    />
                  </div>
                </div>
              )}
            </div>

            <div className="flex items-center gap-3">
              <input
                type="checkbox"
                id="cps-path-style"
                checked={pathStyle}
                onChange={(e) => setPathStyle(e.target.checked)}
                className="h-4 w-4 rounded border-border accent-primary cursor-pointer"
              />
              <Label htmlFor="cps-path-style">
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
            <Button
              type="submit"
              disabled={createMutation.isPending || updateMutation.isPending}
            >
              {(createMutation.isPending || updateMutation.isPending) && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              {editing ? "Save changes" : "Add connection"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
