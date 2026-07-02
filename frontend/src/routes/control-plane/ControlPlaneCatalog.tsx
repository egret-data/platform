import { useState } from "react";
import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import {
  Boxes,
  CheckCircle2,
  Database,
  Loader2,
  Pencil,
  Plus,
  Trash2,
  XCircle,
} from "lucide-react";

import {
  type Assignment,
  type Catalog,
  type CreateCatalogInput,
  type PolarisConnection,
  type PolarisConnectionInput,
  type PolarisHealth,
  assignCatalog,
  createCatalog,
  deleteCatalog,
  getPolarisConnection,
  listAssignments,
  listCatalogs,
  listStorage,
  polarisHealth,
  putPolarisConnection,
  unassignCatalog,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { type WorkspaceSummary, useMyWorkspaces } from "@/api/hooks";
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

const COLUMNS = ["Name", "Type", ""];

export const polarisConnectionQueryKey = ["control-plane", "polaris", "connection"] as const;
export const polarisHealthQueryKey = ["control-plane", "polaris", "health"] as const;
export const catalogsQueryKey = ["control-plane", "polaris", "catalogs"] as const;
export const storageQueryKey = ["control-plane", "storage"] as const;

/**
 * Control Plane → Catalog
 *
 * Platform-admin management of the external Apache Polaris catalog: the Polaris
 * connection (with a live health badge), the catalogs registered in it, and
 * per-workspace catalog assignments.
 */
export default function ControlPlaneCatalog() {
  return (
    <div className="space-y-6">
      <ConnectionCard />
      <CatalogsCard />
    </div>
  );
}

// ── Connection card ──────────────────────────────────────────────────────────

function ConnectionCard() {
  const conn = useQuery<PolarisConnection | null, ApiError>({
    queryKey: polarisConnectionQueryKey,
    queryFn: getPolarisConnection,
    retry: false,
  });
  const [formOpen, setFormOpen] = useState(false);

  const connection = conn.data ?? null;

  return (
    <Card data-testid="polaris-connection-frame">
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle className="flex items-center gap-2 text-lg">
              <Database className="h-5 w-5 text-muted-foreground" />
              Polaris connection
            </CardTitle>
            <CardDescription>
              The external Apache Polaris REST catalog the platform manages.
            </CardDescription>
          </div>
          {connection && (
            <Button variant="outline" onClick={() => setFormOpen(true)}>
              <Pencil className="h-4 w-4" />
              Edit
            </Button>
          )}
        </div>
      </CardHeader>
      <CardContent>
        {conn.isLoading ? (
          <p className="text-muted-foreground">Loading…</p>
        ) : conn.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              Failed to load the Polaris connection. Please try again.
            </AlertDescription>
          </Alert>
        ) : connection ? (
          <div className="flex items-center justify-between gap-4">
            <div className="space-y-1">
              <p className="text-sm text-muted-foreground">Base URL</p>
              <p className="font-mono text-sm">{connection.baseUrl}</p>
            </div>
            <HealthBadge />
          </div>
        ) : (
          <div className="flex flex-col items-center gap-4 py-6 text-center">
            <p className="text-muted-foreground">
              No Polaris connection configured yet.
            </p>
            <Button onClick={() => setFormOpen(true)}>
              <Plus className="h-4 w-4" />
              Connect Polaris
            </Button>
          </div>
        )}
      </CardContent>

      <ConnectionDialog
        open={formOpen}
        onOpenChange={setFormOpen}
        current={connection}
      />
    </Card>
  );
}

function HealthBadge() {
  const health = useQuery<PolarisHealth, ApiError>({
    queryKey: polarisHealthQueryKey,
    queryFn: polarisHealth,
    retry: false,
  });

  if (health.isLoading) {
    return (
      <Badge variant="outline">
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
        Checking…
      </Badge>
    );
  }

  const ok = health.data?.ok === true;
  if (ok) {
    return (
      <Badge className="bg-emerald-600 text-white hover:bg-emerald-600">
        <CheckCircle2 className="h-3.5 w-3.5" />
        Healthy
      </Badge>
    );
  }

  const error =
    health.data?.error ?? (health.isError ? "Unreachable" : "Unhealthy");
  return (
    <Badge variant="destructive" title={error}>
      <XCircle className="h-3.5 w-3.5" />
      {error}
    </Badge>
  );
}

function ConnectionDialog({
  open,
  onOpenChange,
  current,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  current: PolarisConnection | null;
}) {
  const queryClient = useQueryClient();
  const mutation = useMutation<PolarisConnection, ApiError, PolarisConnectionInput>({
    mutationFn: putPolarisConnection,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: polarisConnectionQueryKey });
      queryClient.invalidateQueries({ queryKey: polarisHealthQueryKey });
    },
  });

  const [baseUrl, setBaseUrl] = useState(current?.baseUrl ?? "");
  const [kcTokenUrl, setKcTokenUrl] = useState(current?.kcTokenUrl ?? "");
  const [kcClientId, setKcClientId] = useState(current?.kcClientId ?? "");
  const [kcClientSecret, setKcClientSecret] = useState("");
  const [kcScope, setKcScope] = useState(current?.kcScope ?? "");
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setBaseUrl(current?.baseUrl ?? "");
    setKcTokenUrl(current?.kcTokenUrl ?? "");
    setKcClientId(current?.kcClientId ?? "");
    setKcClientSecret("");
    setKcScope(current?.kcScope ?? "");
    setError(null);
    mutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);

    const input: PolarisConnectionInput = {
      baseUrl: baseUrl.trim(),
      kcTokenUrl: kcTokenUrl.trim(),
      kcClientId: kcClientId.trim(),
      kcClientSecret,
    };
    if (kcScope.trim().length > 0) input.kcScope = kcScope.trim();

    try {
      await mutation.mutateAsync(input);
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to save the connection.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>
              {current ? "Edit Polaris connection" : "Connect Polaris"}
            </DialogTitle>
            <DialogDescription>
              Point the platform at your Apache Polaris REST catalog and its
              Keycloak OAuth client.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cpc-base-url">Base URL</Label>
              <Input
                id="cpc-base-url"
                value={baseUrl}
                onChange={(e) => setBaseUrl(e.target.value)}
                placeholder="e.g. https://polaris.corp.local/api/catalog"
                required
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cpc-token-url">Keycloak token URL</Label>
              <Input
                id="cpc-token-url"
                value={kcTokenUrl}
                onChange={(e) => setKcTokenUrl(e.target.value)}
                placeholder="e.g. https://kc.corp.local/realms/main/protocol/openid-connect/token"
                required
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cpc-client-id">Keycloak client ID</Label>
              <Input
                id="cpc-client-id"
                value={kcClientId}
                onChange={(e) => setKcClientId(e.target.value)}
                placeholder="e.g. polaris-admin"
                required
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cpc-client-secret">Keycloak client secret</Label>
              <Input
                id="cpc-client-secret"
                type="password"
                value={kcClientSecret}
                onChange={(e) => setKcClientSecret(e.target.value)}
                placeholder={current ? "Leave blank to keep existing" : "Client secret"}
                autoComplete="new-password"
                required={!current}
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cpc-scope">Scope</Label>
              <Input
                id="cpc-scope"
                value={kcScope}
                onChange={(e) => setKcScope(e.target.value)}
                placeholder="Optional (e.g. PRINCIPAL_ROLE:ALL)"
              />
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
            <Button type="submit" disabled={mutation.isPending}>
              {mutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              {current ? "Save" : "Connect"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ── Catalogs card ────────────────────────────────────────────────────────────

function CatalogsCard() {
  const catalogs = useQuery<Catalog[], ApiError>({
    queryKey: catalogsQueryKey,
    queryFn: listCatalogs,
    retry: false,
  });
  const queryClient = useQueryClient();
  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: deleteCatalog,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: catalogsQueryKey }),
  });

  const [addOpen, setAddOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Catalog | null>(null);
  const [assignTarget, setAssignTarget] = useState<Catalog | null>(null);

  const rows = catalogs.data ?? [];

  return (
    <Card data-testid="catalogs-frame">
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle className="flex items-center gap-2 text-lg">
              <Boxes className="h-5 w-5 text-muted-foreground" />
              Catalogs
            </CardTitle>
            <CardDescription>
              Catalogs registered in Polaris, backed by a global storage
              connection and assignable to workspaces.
            </CardDescription>
          </div>
          <Button onClick={() => setAddOpen(true)}>
            <Plus className="h-4 w-4" />
            Add catalog
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        {catalogs.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              Failed to load catalogs. Please try again.
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
                    {catalogs.isLoading
                      ? "Loading…"
                      : "No catalogs yet — add one to get started."}
                  </TableCell>
                </TableRow>
              ) : (
                rows.map((cat) => (
                  <TableRow key={cat.name}>
                    <TableCell className="font-medium">{cat.name}</TableCell>
                    <TableCell>
                      <Badge variant="secondary">{cat.catalogType}</Badge>
                    </TableCell>
                    <TableCell className="text-right">
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => setAssignTarget(cat)}
                        aria-label={`Assign ${cat.name}`}
                      >
                        Assign…
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-muted-foreground hover:text-destructive"
                        onClick={() => setDeleteTarget(cat)}
                        aria-label={`Delete ${cat.name}`}
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

      <AddCatalogDialog open={addOpen} onOpenChange={setAddOpen} />

      <AssignCatalogDialog
        catalog={assignTarget}
        onOpenChange={(o) => {
          if (!o) setAssignTarget(null);
        }}
      />

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete «${deleteTarget.name}»?` : "Delete catalog?"}
        description="This catalog will be removed from Polaris. Workspaces assigned to it will lose access."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget.name);
        }}
      />
    </Card>
  );
}

function AddCatalogDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const storage = useQuery({
    queryKey: storageQueryKey,
    queryFn: listStorage,
    retry: false,
  });
  const createMutation = useMutation<Catalog, ApiError, CreateCatalogInput>({
    mutationFn: createCatalog,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: catalogsQueryKey }),
  });

  const [name, setName] = useState("");
  const [storageId, setStorageId] = useState("");
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setName("");
    setStorageId("");
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

    if (!storageId) {
      setError("Select a storage connection.");
      return;
    }

    try {
      await createMutation.mutateAsync({
        name: name.trim(),
        storageConnectionId: storageId,
      });
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to create the catalog.");
    }
  }

  const items = storage.data ?? [];

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Add catalog</DialogTitle>
            <DialogDescription>
              Register a new catalog in Polaris backed by a global storage
              connection.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cpc-cat-name">Name</Label>
              <Input
                id="cpc-cat-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. lakehouse"
                required
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cpc-cat-storage">Storage connection</Label>
              <Select value={storageId} onValueChange={setStorageId}>
                <SelectTrigger id="cpc-cat-storage" className="w-full">
                  <SelectValue placeholder="Select a storage connection" />
                </SelectTrigger>
                <SelectContent>
                  {items.map((s) => (
                    <SelectItem key={s.id} value={s.id}>
                      {s.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
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
            <Button type="submit" disabled={createMutation.isPending}>
              {createMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Add catalog
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ── Assign dialog ────────────────────────────────────────────────────────────

function AssignCatalogDialog({
  catalog,
  onOpenChange,
}: {
  catalog: Catalog | null;
  onOpenChange: (open: boolean) => void;
}) {
  const open = catalog !== null;
  const queryClient = useQueryClient();
  const workspaces = useMyWorkspaces(open);

  const wsList = workspaces.data ?? [];

  const assignmentQueries = useQueries({
    queries: wsList.map((ws) => ({
      queryKey: ["control-plane", "assignments", ws.id],
      queryFn: () => listAssignments(ws.id),
      enabled: open,
      retry: false,
    })),
  });

  const assignMutation = useMutation<void, ApiError, { wsId: string; assigned: boolean }>({
    mutationFn: ({ wsId, assigned }) =>
      assigned
        ? unassignCatalog(wsId, catalog!.name)
        : assignCatalog(wsId, catalog!.name),
    onSuccess: (_data, { wsId }) =>
      queryClient.invalidateQueries({
        queryKey: ["control-plane", "assignments", wsId],
      }),
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            Assign «{catalog?.name ?? ""}» to workspaces
          </DialogTitle>
          <DialogDescription>
            Grant or revoke access to this catalog per workspace.
          </DialogDescription>
        </DialogHeader>

        <div className="my-4 space-y-2">
          {workspaces.isLoading ? (
            <p className="text-muted-foreground">Loading workspaces…</p>
          ) : wsList.length === 0 ? (
            <p className="text-muted-foreground">No workspaces available.</p>
          ) : (
            wsList.map((ws: WorkspaceSummary, i) => {
              const q = assignmentQueries[i];
              const assignments = (q?.data as Assignment[] | undefined) ?? [];
              const assigned = assignments.some(
                (a) => a.catalogName === catalog?.name,
              );
              const pending =
                assignMutation.isPending &&
                assignMutation.variables?.wsId === ws.id;
              return (
                <div
                  key={ws.id}
                  className="flex items-center justify-between gap-4 rounded-md border px-3 py-2"
                >
                  <div className="min-w-0">
                    <p className="truncate font-medium">{ws.name}</p>
                    <p className="truncate text-sm text-muted-foreground">
                      {ws.slug}
                    </p>
                  </div>
                  <Button
                    variant={assigned ? "outline" : "default"}
                    size="sm"
                    disabled={pending || q?.isLoading}
                    onClick={() =>
                      assignMutation.mutate({ wsId: ws.id, assigned })
                    }
                    aria-label={`${assigned ? "Unassign" : "Assign"} ${ws.name}`}
                  >
                    {pending && <Loader2 className="h-4 w-4 animate-spin" />}
                    {assigned ? "Unassign" : "Assign"}
                  </Button>
                </div>
              );
            })
          )}
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
