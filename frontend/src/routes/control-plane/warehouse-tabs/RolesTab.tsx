import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Plus, ShieldCheck, Trash2 } from "lucide-react";

import {
  type CatalogClient,
  type CatalogRole,
  type CreateRoleInput,
  type Grant,
  type GrantInput,
  type Namespace,
  type Tabular,
  addGrant,
  createRole,
  deleteRole,
  listClients,
  listGrants,
  listNamespaces,
  listRoles,
  listTabulars,
  revokeGrant,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { clientsKey, consumerDot, rolesKey } from "./shared";

const CONSUMER_TYPES = ["engine", "notebook", "external"] as const;

function grantsKey(roleId: string) {
  return ["control-plane", "roles", roleId, "grants"] as const;
}

// ── Roles & grants ────────────────────────────────────────────────────────────

export function RolesTab({ warehouseId }: { warehouseId: string }) {
  const roles = useQuery<CatalogRole[], ApiError>({
    queryKey: rolesKey(warehouseId),
    queryFn: () => listRoles(warehouseId),
    retry: false,
  });
  const clients = useQuery<CatalogClient[], ApiError>({
    queryKey: clientsKey(warehouseId),
    queryFn: () => listClients(warehouseId),
    retry: false,
  });
  const [addOpen, setAddOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const items = roles.data ?? [];
  const clientCount = (roleId: string) =>
    (clients.data ?? []).filter((c) => c.catalogRoleId === roleId).length;

  return (
    <section className="space-y-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="space-y-1">
          <h2 className="flex items-center gap-2 text-base font-semibold">
            <ShieldCheck className="h-4 w-4 text-muted-foreground" />
            Catalog roles
          </h2>
          <p className="text-sm text-muted-foreground">
            Keyed by consumer-type
            <span className="mx-1 inline-flex items-center gap-1"><span className="h-1.5 w-1.5 rounded-full bg-primary" />engine</span>
            <span className="mx-1 inline-flex items-center gap-1"><span className="h-1.5 w-1.5 rounded-full bg-success" />notebook</span>
            <span className="mx-1 inline-flex items-center gap-1"><span className="h-1.5 w-1.5 rounded-full bg-highlight" />external</span>
            — their grants gate reads/writes on the warehouse.
          </p>
        </div>
        <Button className="shrink-0" onClick={() => setAddOpen(true)}>
          <Plus className="h-4 w-4" />
          Add role
        </Button>
      </div>
      {error && (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      <div className="overflow-x-auto rounded-md border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Role</TableHead>
              <TableHead>Consumer</TableHead>
              <TableHead>Grants</TableHead>
              <TableHead className="text-center">Clients</TableHead>
              <TableHead className="text-right">
                <span className="sr-only">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {items.length === 0 ? (
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={5} className="py-10 text-center text-muted-foreground">
                  {roles.isError ? (
                    <span className="text-destructive">
                      {/* `ApiError.message` for a 403 is `Error::Forbidden`'s Display, the
                          bare word "forbidden" — it names no permission and offers no way
                          forward. This route is now gated `catalog_registry:read` (SP3c-2,
                          Task 7 retired `warehouse:read`), and this tab renders unconditionally
                          on the persona holding only `catalog_registry:read` on the catalog
                          section, so this branch is the one that persona actually reaches. */}
                      {roles.error?.status === 403
                        ? "Couldn't load catalog roles: this needs the catalog_registry:read permission. A platform administrator can grant it."
                        : (roles.error?.message ?? "Couldn't load roles.")}
                    </span>
                  ) : roles.isLoading ? (
                    <ListSkeleton rows={3} cols={4} />
                  ) : (
                    "No roles yet — add one to grant catalog access."
                  )}
                </TableCell>
              </TableRow>
            ) : (
              items.map((role) => (
                <RoleRow
                  key={role.id}
                  role={role}
                  warehouseId={warehouseId}
                  clientCount={clientCount(role.id)}
                  onError={setError}
                />
              ))
            )}
          </TableBody>
        </Table>
      </div>

      <AddRoleDialog warehouseId={warehouseId} open={addOpen} onOpenChange={setAddOpen} />
    </section>
  );
}

function consumerHint(type: string): string {
  if (type === "engine") return "Query engines (Trino / StarRocks / Daft) authenticating to read/write.";
  if (type === "notebook") return "Notebook kernels accessing the catalog.";
  if (type === "external") return "External / third-party integrations.";
  return type;
}

function RoleRow({
  role,
  warehouseId,
  clientCount,
  onError,
}: {
  role: CatalogRole;
  warehouseId: string;
  clientCount: number;
  onError: (m: string | null) => void;
}) {
  const queryClient = useQueryClient();
  const grants = useQuery<Grant[], ApiError>({
    queryKey: grantsKey(role.id),
    queryFn: () => listGrants(role.id),
    retry: false,
  });
  const revoke = useMutation<void, ApiError, string>({
    mutationFn: revokeGrant,
    onSuccess: () => {
      onError(null);
      queryClient.invalidateQueries({ queryKey: grantsKey(role.id) });
    },
    onError: (e) => onError(e.message ?? "Failed to revoke grant."),
  });
  const del = useMutation<void, ApiError, void>({
    mutationFn: () => deleteRole(role.id),
    onSuccess: () => {
      onError(null);
      queryClient.invalidateQueries({ queryKey: rolesKey(warehouseId) });
    },
    onError: (e) => onError(e.message ?? "Failed to delete role."),
  });
  const [addOpen, setAddOpen] = useState(false);
  const [delOpen, setDelOpen] = useState(false);
  const rows = grants.data ?? [];
  const target = (g: Grant) =>
    g.namespace ? g.namespace.join(".") + (g.tableName ? `.${g.tableName}` : "") : g.securableType;

  return (
    <TableRow>
      <TableCell>
        <div className="flex items-center gap-2">
          <span
            className={`h-2 w-2 shrink-0 rounded-full ${consumerDot(role.consumerType)}`}
            title={role.consumerType}
          />
          <div className="min-w-0">
            <div className="font-medium">{role.name}</div>
            {role.description && (
              <div className="truncate text-xs text-muted-foreground">{role.description}</div>
            )}
          </div>
        </div>
      </TableCell>
      <TableCell>
        <Badge variant="secondary" title={consumerHint(role.consumerType)}>{role.consumerType}</Badge>
      </TableCell>
      <TableCell>
        {rows.length === 0 ? (
          <span className="text-sm text-muted-foreground">No grants</span>
        ) : (
          <div className="flex flex-wrap gap-1.5">
            {rows.map((g) => (
              <span
                key={g.id}
                className="group inline-flex items-center gap-1 rounded-md border bg-muted/40 px-1.5 py-0.5 text-xs"
              >
                <span className="font-mono">{target(g)}</span>
                <span className="text-muted-foreground">·</span>
                <span className="font-medium">{g.privilege}</span>
                <button
                  type="button"
                  aria-label={`Revoke ${g.privilege} on ${target(g)} from ${role.name}`}
                  onClick={() => revoke.mutate(g.id)}
                  className="-my-1 flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted-foreground opacity-60 transition-opacity hover:text-destructive"
                >
                  <Trash2 className="h-3 w-3" />
                </button>
              </span>
            ))}
          </div>
        )}
      </TableCell>
      <TableCell className="text-center text-sm tabular-nums text-muted-foreground">{clientCount}</TableCell>
      <TableCell className="text-right">
        <div className="flex items-center justify-end gap-1">
          <Button variant="outline" size="sm" onClick={() => setAddOpen(true)}>
            <Plus className="h-4 w-4" />
            Add grant
          </Button>
          <Button
            variant="ghost"
            size="sm"
            className="text-muted-foreground hover:text-destructive"
            onClick={() => setDelOpen(true)}
            aria-label={`Delete role ${role.name}`}
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
        <AddGrantDialog roleId={role.id} warehouseId={warehouseId} open={addOpen} onOpenChange={setAddOpen} />
        <ConfirmDeleteDialog
          open={delOpen}
          onOpenChange={setDelOpen}
          title={`Delete role «${role.name}»?`}
          description={
            clientCount > 0
              ? `This role is used by ${clientCount} Catalog Client${clientCount === 1 ? "" : "s"} — delete those clients first.`
              : "The role and its grants will be removed."
          }
          confirmText="delete"
          onConfirm={() => del.mutate()}
        />
      </TableCell>
    </TableRow>
  );
}

function AddRoleDialog({
  warehouseId,
  open,
  onOpenChange,
}: {
  warehouseId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const create = useMutation<CatalogRole, ApiError, CreateRoleInput>({
    mutationFn: (input) => createRole(warehouseId, input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: rolesKey(warehouseId) }),
  });
  const [name, setName] = useState("");
  const [consumerType, setConsumerType] = useState<string>("engine");
  const [description, setDescription] = useState("");
  const [error, setError] = useState<string | null>(null);

  function handleOpenChange(next: boolean) {
    if (!next) {
      setName("");
      setConsumerType("engine");
      setDescription("");
      setError(null);
      create.reset();
    }
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await create.mutateAsync({
        name: name.trim(),
        consumerType,
        description: description.trim() || null,
      });
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to create role.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Add role</DialogTitle>
            <DialogDescription>A catalog role for a consumer type.</DialogDescription>
          </DialogHeader>
          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cpr-name">Name</Label>
              <Input id="cpr-name" value={name} onChange={(e) => setName(e.target.value)} required autoFocus />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="cpr-consumer">Consumer type</Label>
              <Select value={consumerType} onValueChange={setConsumerType}>
                <SelectTrigger id="cpr-consumer" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {CONSUMER_TYPES.map((c) => (
                    <SelectItem key={c} value={c}>
                      {c}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="cpr-desc">Description</Label>
              <Input
                id="cpr-desc"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="Optional — what this role is for"
              />
            </div>
            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending}>
              {create.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Add role
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

const ALL_NS = "__all_ns__";
const ALL_TABLES = "__all_tables__";

function AddGrantDialog({
  roleId,
  warehouseId,
  open,
  onOpenChange,
}: {
  roleId: string;
  warehouseId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const create = useMutation<void, ApiError, GrantInput>({
    mutationFn: (input) => addGrant(roleId, input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: grantsKey(roleId) }),
  });
  const [namespaceSel, setNamespaceSel] = useState(ALL_NS);
  const [tableSel, setTableSel] = useState(ALL_TABLES);
  const [privilege, setPrivilege] = useState("read");
  const [error, setError] = useState<string | null>(null);

  const nsQuery = useQuery<Namespace[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "ns"],
    queryFn: () => listNamespaces(warehouseId),
    enabled: open,
  });

  const tablesQuery = useQuery<Tabular[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "tables", namespaceSel],
    queryFn: () => listTabulars(warehouseId, namespaceSel),
    enabled: open && namespaceSel !== ALL_NS,
  });

  function handleNamespaceChange(val: string) {
    setNamespaceSel(val);
    setTableSel(ALL_TABLES);
  }

  function handleOpenChange(next: boolean) {
    if (!next) {
      setNamespaceSel(ALL_NS);
      setTableSel(ALL_TABLES);
      setPrivilege("read");
      setError(null);
      create.reset();
    }
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    const securableType =
      tableSel !== ALL_TABLES ? "table" : namespaceSel !== ALL_NS ? "namespace" : "catalog";
    const namespace = namespaceSel !== ALL_NS ? namespaceSel.split(".") : null;
    const tableName = tableSel !== ALL_TABLES ? tableSel : null;
    try {
      await create.mutateAsync({ securableType, namespace, tableName, privilege });
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to add grant.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Add grant</DialogTitle>
            <DialogDescription>
              A <code className="font-mono text-xs">write</code>/
              <code className="font-mono text-xs">manage</code> privilege permits writes; any grant
              permits reads.
            </DialogDescription>
          </DialogHeader>
          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cpg-ns">Namespace</Label>
              <Select value={namespaceSel} onValueChange={handleNamespaceChange}>
                <SelectTrigger id="cpg-ns" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={ALL_NS}>All namespaces</SelectItem>
                  {(nsQuery.data ?? []).map((ns) => {
                    const label = ns.parts.join(".");
                    return (
                      <SelectItem key={label} value={label}>
                        {label}
                      </SelectItem>
                    );
                  })}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="cpg-table-sel">Table</Label>
              <Select
                value={tableSel}
                onValueChange={setTableSel}
                disabled={namespaceSel === ALL_NS}
              >
                <SelectTrigger id="cpg-table-sel" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={ALL_TABLES}>All tables</SelectItem>
                  {(tablesQuery.data ?? []).map((t) => (
                    <SelectItem key={t.name} value={t.name}>
                      {t.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="cpg-privilege">Privilege</Label>
              <Select value={privilege} onValueChange={setPrivilege}>
                <SelectTrigger id="cpg-privilege" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="read">read</SelectItem>
                  <SelectItem value="write">write</SelectItem>
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
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending}>
              {create.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Add grant
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

