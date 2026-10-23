import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { HardDrive, Loader2, Plus, Trash2 } from "lucide-react";

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
import { pickableSecrets } from "@/lib/systemSecrets";
import { EmptyState } from "@/components/explore/EmptyState";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
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
import { PageContainer } from "@/components/layout/PageContainer";
import { PageTitle } from "@/components/layout/PageTitle";
import { ENTITY_LINK_CLASS } from "@/components/ui/entity-link";

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
  const [deleteTarget, setDeleteTarget] = useState<Storage | null>(null);

  const rows = conns.data ?? [];

  const columns: Column<Storage>[] = [
    {
      key: "name",
      label: "Name",
      sortable: true,
      className: "font-medium",
      value: (c) => c.name,
      cell: (c) => (
        <Link
          to={`/control-plane/storage/${c.id}`}
          className={ENTITY_LINK_CLASS}
        >
          {c.name}
        </Link>
      ),
    },
    {
      key: "purpose",
      label: "Purpose",
      sortable: true,
      value: (c) => c.purpose ?? "",
      cell: (c) => <span className="text-muted-foreground">{c.purpose ?? "—"}</span>,
    },
    {
      key: "provider",
      label: "Provider",
      sortable: true,
      value: (c) => c.provider,
      cell: (c) => <Badge variant="secondary">{c.provider}</Badge>,
    },
    {
      key: "bucket",
      label: "Bucket",
      sortable: true,
      value: (c) => c.bucket,
      cell: (c) => <span className="font-mono text-muted-foreground">{c.bucket}</span>,
    },
    {
      key: "endpoint",
      label: "Endpoint",
      value: (c) => c.endpoint ?? "",
      cell: (c) => <span className="text-muted-foreground">{c.endpoint ?? "AWS default"}</span>,
    },
    {
      key: "secret",
      label: "Secret",
      sortable: true,
      value: (c) => (c.secret_ref ? "Configured" : "Not set"),
      cell: (c) => (
        <Badge variant={c.secret_ref ? "secondary" : "outline"}>
          {c.secret_ref ? "Configured" : "Not set"}
        </Badge>
      ),
    },
    {
      key: "actions",
      label: "",
      className: "w-10 text-right",
      cell: (c) => (
        <div className="flex justify-end">
          <Button
            variant="ghost"
            size="sm"
            className="text-muted-foreground hover:text-destructive"
            onClick={() => setDeleteTarget(c)}
            aria-label={`Delete ${c.name}`}
          >
            <Trash2 className="h-4 w-4" />
            Delete
          </Button>
        </div>
      ),
    },
  ];

  return (
    <PageContainer>
      <div className="space-y-4" data-testid="storage-frame">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="max-w-3xl">
          <PageTitle>Storage</PageTitle>
          <p className="text-sm text-muted-foreground">
            Global storage connections (S3-compatible object stores) available to catalogs and
            workspaces across the platform.
          </p>
        </div>
        <Button className="shrink-0" onClick={() => setAddOpen(true)}>
          <Plus className="h-4 w-4" />
          Add connection
        </Button>
      </div>

      <SummaryStrip items={[{ label: "connections", value: rows.length }]} />

      {conns.isError ? (
        <Alert variant="destructive">
          <AlertDescription>Failed to load storage connections. Please try again.</AlertDescription>
        </Alert>
      ) : conns.isLoading ? (
        <ListSkeleton rows={5} cols={7} />
      ) : (
        <FilterableTable
          rows={rows}
          columns={columns}
          getId={(c) => c.id}
          searchPlaceholder="Search connections…"
          initialSort={{ key: "name", dir: "asc" }}
          pageSize={25}
          bordered
          emptyText={
            <EmptyState
              icon={HardDrive}
              title="No storage connections yet"
              description="Register an S3-compatible object store so catalogs and workspaces can use it."
              action={
                <Button size="sm" onClick={() => setAddOpen(true)}>
                  <Plus className="mr-1 h-4 w-4" />
                  Add connection
                </Button>
              }
            />
          }
        />
      )}

      <AddStorageDialog open={addOpen} onOpenChange={setAddOpen} editing={null} />

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
      </div>
    </PageContainer>
  );
}

// Sentinel <Select> values that must not collide with a real secret id.
const NO_SECRET = "__none__";
const NEW_SECRET = "__new__";

export function AddStorageDialog({
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

  const items = pickableSecrets(secrets.data ?? []);

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
                  <SelectItem value="s3">S3 / RustFS</SelectItem>
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
                placeholder="Leave blank for AWS (e.g. https://s3.corp.local)"
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

            {/* THREE OF THIS CONTROL'S FOUR CLASSES WERE DEAD, which is worse than them being
                wrong. It carried `h-4 w-4 rounded border-border accent-primary`, and measured on
                the kind deployment against `main`'s bundle, `getComputedStyle` returned
                `appearance: auto`, `border-width: 0px`, `border-radius: 0px`. Nothing ever reset
                `appearance`, so the UA painted the whole control itself and `rounded` and
                `border-border` produced no pixels at all — only `accent-color` got through, and
                only to tint the tick. What was on screen was the PLATFORM's checkbox: its shape,
                its tick, its focus ring, different on macOS, Windows and Linux, beside an app that
                draws everything else itself.

                So this was never a case of #443's radius fix being overridden — it is a case of it
                never applying, along with the transparent 24×24 pointer target SC 2.5.8 asks for
                (measured: 16×16 before, 24×24 after) and the indeterminate mark, which a native
                input cannot draw from JSX at all.

                The row is `gap-3` with the label 12px to the right and nothing within 16px above or
                below, so the 4px bleed of the hit area collides with nothing — measured. Clicking
                the label toggles the control: verified live, twice, as two separate interactions
                rather than one tick, and note the default here is `useState(true)`, which is what
                made the first check look like a failure. */}
            <div className="flex items-center gap-3">
              <Checkbox
                id="cps-path-style"
                checked={pathStyle}
                onCheckedChange={(v) => setPathStyle(v === true)}
                className="cursor-pointer"
              />
              <Label htmlFor="cps-path-style" className="cursor-pointer">
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
