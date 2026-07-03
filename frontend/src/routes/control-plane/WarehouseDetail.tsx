import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  Boxes,
  Building2,
  Check,
  ChevronDown,
  ChevronRight,
  Columns3,
  Copy,
  Database,
  FileText,
  HardDrive,
  History,
  KeyRound,
  Loader2,
  Plus,
  ShieldCheck,
  Table2,
  Trash2,
} from "lucide-react";

import {
  type CatalogClient,
  type CatalogClientCreated,
  type CatalogRole,
  type CreateClientInput,
  type CreateRoleInput,
  type Grant,
  type GrantInput,
  type Namespace,
  type TableDetail,
  type Tabular,
  type Warehouse,
  addGrant,
  createClient,
  createRole,
  deleteRole,
  getWarehouse,
  listClients,
  listGrants,
  listNamespaces,
  listRoles,
  listStorage,
  listTabulars,
  loadTableDetail,
  revokeClient,
  revokeGrant,
} from "@/api/controlPlane";
import { useMyWorkspaces } from "@/api/hooks";
import { ApiError } from "@/api/client";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

const CONSUMER_TYPES = ["engine", "notebook", "external"] as const;

export function rolesKey(whId: string) {
  return ["control-plane", "warehouses", whId, "roles"] as const;
}
export function clientsKey(whId: string) {
  return ["control-plane", "warehouses", whId, "clients"] as const;
}
function grantsKey(roleId: string) {
  return ["control-plane", "roles", roleId, "grants"] as const;
}

/**
 * Control Plane → Warehouse detail
 *
 * Catalog roles (keyed by consumer-type) with their grants, and the per-consumer
 * Catalog Clients that machines authenticate with.
 */
export default function WarehouseDetail() {
  const { id = "" } = useParams();
  const warehouse = useQuery<Warehouse, ApiError>({
    queryKey: ["control-plane", "warehouses", id],
    queryFn: () => getWarehouse(id),
    retry: false,
  });

  return (
    <div className="space-y-5">
      <Link
        to="/control-plane/warehouses"
        className="inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
      >
        <ArrowLeft className="h-4 w-4" />
        Warehouses
      </Link>

      <WarehouseSummary warehouseId={id} warehouse={warehouse.data} />

      <Tabs defaultValue="roles">
        <TabsList>
          <TabsTrigger value="roles">
            <ShieldCheck className="h-4 w-4" />
            Roles &amp; grants
          </TabsTrigger>
          <TabsTrigger value="clients">
            <KeyRound className="h-4 w-4" />
            Catalog Clients
          </TabsTrigger>
          <TabsTrigger value="browse">
            <Database className="h-4 w-4" />
            Browse
          </TabsTrigger>
        </TabsList>
        <TabsContent value="roles" className="mt-4">
          <RolesTab warehouseId={id} />
        </TabsContent>
        <TabsContent value="clients" className="mt-4">
          <ClientsTab warehouseId={id} />
        </TabsContent>
        <TabsContent value="browse" className="mt-4">
          <BrowseTab warehouseId={id} />
        </TabsContent>
      </Tabs>
    </div>
  );
}

/** engine → teal, notebook → violet, external → amber. */
function consumerDot(type: string): string {
  if (type === "engine") return "bg-teal-500";
  if (type === "notebook") return "bg-violet-500";
  if (type === "external") return "bg-amber-500";
  return "bg-muted-foreground";
}

function CopyButton({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      aria-label="Copy"
      className="text-muted-foreground transition-colors hover:text-foreground"
      onClick={() => {
        void navigator.clipboard?.writeText(value);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      }}
    >
      {copied ? <Check className="h-3.5 w-3.5 text-teal-600" /> : <Copy className="h-3.5 w-3.5" />}
    </button>
  );
}

/** Warehouse summary band: identity + status + key facts + quick counts. */
function WarehouseSummary({
  warehouseId,
  warehouse,
}: {
  warehouseId: string;
  warehouse: Warehouse | undefined;
}) {
  const roles = useQuery({ queryKey: rolesKey(warehouseId), queryFn: () => listRoles(warehouseId), retry: false });
  const clients = useQuery({ queryKey: clientsKey(warehouseId), queryFn: () => listClients(warehouseId), retry: false });
  const namespaces = useQuery({ queryKey: ["cp", "browse", warehouseId, "ns"], queryFn: () => listNamespaces(warehouseId), retry: false });
  const workspaces = useMyWorkspaces(true);
  const storage = useQuery({ queryKey: ["control-plane", "storage"], queryFn: listStorage, retry: false });

  const wsName = workspaces.data?.find((w) => w.id === warehouse?.workspaceId)?.name;
  const storageName = storage.data?.find((s) => s.id === warehouse?.storageConnId)?.name;
  const provisioned = !!warehouse?.lakekeeperWarehouseId;

  const stats = [
    { icon: ShieldCheck, label: "roles", value: roles.data?.length ?? 0 },
    { icon: KeyRound, label: "clients", value: clients.data?.length ?? 0 },
    { icon: Database, label: "namespaces", value: namespaces.data?.length ?? 0 },
  ];

  return (
    <Card className="overflow-hidden">
      <div className="flex flex-col gap-5 p-6 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0 space-y-3">
          <div className="flex items-center gap-3">
            <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
              <Boxes className="h-6 w-6" />
            </div>
            <div className="min-w-0">
              <h1 className="truncate text-2xl font-semibold leading-tight">{warehouse?.name ?? "Warehouse"}</h1>
              <Badge
                variant="outline"
                className={
                  provisioned
                    ? "mt-1 gap-1 border-teal-500/30 bg-teal-500/10 text-teal-700"
                    : "mt-1 gap-1 border-amber-500/30 bg-amber-500/10 text-amber-700"
                }
              >
                <span className={`h-1.5 w-1.5 rounded-full ${provisioned ? "bg-teal-500" : "bg-amber-500"}`} />
                {provisioned ? "Provisioned" : "Pending"}
              </Badge>
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
            <Fact icon={Building2} label="Workspace" value={wsName ?? "—"} />
            <Fact icon={HardDrive} label="Storage" value={storageName ?? "—"} />
            {warehouse?.lakekeeperWarehouseId && (
              <div className="flex items-center gap-1.5 text-muted-foreground">
                <span className="text-xs uppercase tracking-wide">Lakekeeper</span>
                <code className="font-mono text-xs text-foreground">
                  {warehouse.lakekeeperWarehouseId.slice(0, 8)}…
                </code>
                <CopyButton value={warehouse.lakekeeperWarehouseId} />
              </div>
            )}
          </div>
        </div>

        <div className="grid grid-cols-3 gap-2 sm:gap-3">
          {stats.map((s) => (
            <div
              key={s.label}
              className="flex min-w-[76px] flex-col items-center rounded-lg border bg-muted/30 px-3 py-2"
            >
              <s.icon className="h-4 w-4 text-muted-foreground" />
              <span className="mt-1 text-lg font-semibold leading-none">{s.value}</span>
              <span className="text-xs text-muted-foreground">{s.label}</span>
            </div>
          ))}
        </div>
      </div>
    </Card>
  );
}

function Fact({
  icon: Icon,
  label,
  value,
}: {
  icon: typeof Building2;
  label: string;
  value: string;
}) {
  return (
    <div className="flex items-center gap-1.5 text-muted-foreground">
      <Icon className="h-4 w-4" />
      <span className="text-xs uppercase tracking-wide">{label}</span>
      <span className="text-foreground">{value}</span>
    </div>
  );
}

// ── Roles & grants ────────────────────────────────────────────────────────────

function RolesTab({ warehouseId }: { warehouseId: string }) {
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
    <Card>
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle className="flex items-center gap-2 text-lg">
              <ShieldCheck className="h-5 w-5 text-muted-foreground" />
              Catalog roles
            </CardTitle>
            <CardDescription>
              Keyed by consumer-type
              <span className="mx-1 inline-flex items-center gap-1"><span className="h-1.5 w-1.5 rounded-full bg-teal-500" />engine</span>
              <span className="mx-1 inline-flex items-center gap-1"><span className="h-1.5 w-1.5 rounded-full bg-violet-500" />notebook</span>
              <span className="mx-1 inline-flex items-center gap-1"><span className="h-1.5 w-1.5 rounded-full bg-amber-500" />external</span>
              — their grants gate reads/writes on the warehouse.
            </CardDescription>
          </div>
          <Button onClick={() => setAddOpen(true)}>
            <Plus className="h-4 w-4" />
            Add role
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-3">
        {error && (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}
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
                  {roles.isLoading ? "Loading…" : "No roles yet — add one to grant catalog access."}
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
      </CardContent>

      <AddRoleDialog warehouseId={warehouseId} open={addOpen} onOpenChange={setAddOpen} />
    </Card>
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
    onSuccess: () => queryClient.invalidateQueries({ queryKey: grantsKey(role.id) }),
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
                  aria-label="Revoke grant"
                  onClick={() => revoke.mutate(g.id)}
                  className="text-muted-foreground opacity-60 transition-opacity hover:text-destructive"
                >
                  <Trash2 className="h-3 w-3" />
                </button>
              </span>
            ))}
          </div>
        )}
      </TableCell>
      <TableCell className="text-center text-sm text-muted-foreground">{clientCount}</TableCell>
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
              A `write`/`manage` privilege permits writes; any grant permits reads.
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

// ── Catalog Clients ────────────────────────────────────────────────────────────

function ClientsTab({ warehouseId }: { warehouseId: string }) {
  const queryClient = useQueryClient();
  const clients = useQuery<CatalogClient[], ApiError>({
    queryKey: clientsKey(warehouseId),
    queryFn: () => listClients(warehouseId),
    retry: false,
  });
  const roles = useQuery<CatalogRole[], ApiError>({
    queryKey: rolesKey(warehouseId),
    queryFn: () => listRoles(warehouseId),
    retry: false,
  });
  const revoke = useMutation<void, ApiError, string>({
    mutationFn: revokeClient,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: clientsKey(warehouseId) }),
  });
  const [addOpen, setAddOpen] = useState(false);
  const rows = clients.data ?? [];
  const roleName = (id: string) => roles.data?.find((r) => r.id === id)?.name ?? id.slice(0, 8);

  return (
    <Card>
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle className="flex items-center gap-2 text-lg">
              <KeyRound className="h-5 w-5 text-muted-foreground" />
              Catalog Clients
            </CardTitle>
            <CardDescription>
              Machine credentials (client id + secret) an engine or notebook uses to authenticate.
            </CardDescription>
          </div>
          <Button onClick={() => setAddOpen(true)} disabled={(roles.data ?? []).length === 0}>
            <Plus className="h-4 w-4" />
            Add client
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
              <TableHead>Consumer</TableHead>
              <TableHead>Role</TableHead>
              <TableHead>Client ID</TableHead>
              <TableHead className="text-right">
                <span className="sr-only">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.length === 0 ? (
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={5} className="py-10 text-center text-muted-foreground">
                  {clients.isLoading
                    ? "Loading…"
                    : (roles.data ?? []).length === 0
                      ? "Add a role first, then create a client for it."
                      : "No clients yet — add one to issue credentials."}
                </TableCell>
              </TableRow>
            ) : (
              rows.map((c) => (
                <TableRow key={c.id}>
                  <TableCell className="font-medium">{c.name}</TableCell>
                  <TableCell>
                    <Badge variant="secondary">{c.consumerType}</Badge>
                  </TableCell>
                  <TableCell className="text-muted-foreground">{roleName(c.catalogRoleId)}</TableCell>
                  <TableCell className="font-mono text-muted-foreground">{c.clientId}</TableCell>
                  <TableCell className="text-right">
                    <Button
                      variant="ghost"
                      size="sm"
                      className="text-muted-foreground hover:text-destructive"
                      onClick={() => revoke.mutate(c.id)}
                      aria-label={`Revoke ${c.name}`}
                    >
                      <Trash2 className="h-4 w-4" />
                      Revoke
                    </Button>
                  </TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </CardContent>

      <AddClientDialog
        warehouseId={warehouseId}
        roles={roles.data ?? []}
        open={addOpen}
        onOpenChange={setAddOpen}
      />
    </Card>
  );
}

function AddClientDialog({
  warehouseId,
  roles,
  open,
  onOpenChange,
}: {
  warehouseId: string;
  roles: CatalogRole[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const workspaces = useMyWorkspaces(open);
  const create = useMutation<CatalogClientCreated, ApiError, CreateClientInput>({
    mutationFn: (input) => createClient(warehouseId, input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: clientsKey(warehouseId) }),
  });
  const [name, setName] = useState("");
  const [roleId, setRoleId] = useState("");
  const [workspaceId, setWorkspaceId] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<CatalogClientCreated | null>(null);

  const selectedRole = roles.find((r) => r.id === roleId);

  function handleOpenChange(next: boolean) {
    if (!next) {
      setName("");
      setRoleId("");
      setWorkspaceId("");
      setError(null);
      setCreated(null);
      create.reset();
    }
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    if (!selectedRole) return;
    try {
      const result = await create.mutateAsync({
        name: name.trim(),
        consumerType: selectedRole.consumerType,
        workspaceId,
        catalogRoleId: roleId,
      });
      setCreated(result);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to create client.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        {created ? (
          <div>
            <DialogHeader>
              <DialogTitle>Client created</DialogTitle>
              <DialogDescription>
                Copy the client secret now — it is shown only once and cannot be retrieved later.
              </DialogDescription>
            </DialogHeader>
            <div className="my-4 space-y-3">
              <CredField label="Client ID" value={created.clientId} />
              <CredField label="Client secret" value={created.clientSecret} />
              <Alert>
                <AlertDescription>
                  Store these in the engine/notebook configuration. The secret will not be shown again.
                </AlertDescription>
              </Alert>
            </div>
            <DialogFooter>
              <Button onClick={() => handleOpenChange(false)}>Done</Button>
            </DialogFooter>
          </div>
        ) : (
          <form onSubmit={handleSubmit}>
            <DialogHeader>
              <DialogTitle>Add client</DialogTitle>
              <DialogDescription>Issue machine credentials bound to a catalog role.</DialogDescription>
            </DialogHeader>
            <div className="my-4 space-y-4">
              <div className="space-y-1.5">
                <Label htmlFor="cpc-name">Name</Label>
                <Input id="cpc-name" value={name} onChange={(e) => setName(e.target.value)} required autoFocus />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="cpc-role">Role</Label>
                <Select value={roleId} onValueChange={setRoleId}>
                  <SelectTrigger id="cpc-role" className="w-full">
                    <SelectValue placeholder="Select a role" />
                  </SelectTrigger>
                  <SelectContent>
                    {roles.map((r) => (
                      <SelectItem key={r.id} value={r.id}>
                        {r.name} ({r.consumerType})
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="cpc-workspace">Workspace</Label>
                <Select value={workspaceId} onValueChange={setWorkspaceId}>
                  <SelectTrigger id="cpc-workspace" className="w-full">
                    <SelectValue placeholder="Select a workspace" />
                  </SelectTrigger>
                  <SelectContent>
                    {(workspaces.data ?? []).map((w) => (
                      <SelectItem key={w.id} value={w.id}>
                        {w.name}
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
              <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={create.isPending || !roleId || !workspaceId}>
                {create.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
                Create client
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

// ── Browse (namespaces / tables / schema) ────────────────────────────────────

function BrowseTab({ warehouseId }: { warehouseId: string }) {
  const [selectedNs, setSelectedNs] = useState<string | null>(null);
  const [selectedTable, setSelectedTable] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  const namespaces = useQuery<Namespace[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "ns"],
    queryFn: () => listNamespaces(warehouseId),
    retry: false,
  });

  function toggle(label: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(label)) next.delete(label);
      else next.add(label);
      return next;
    });
  }

  const list = namespaces.data ?? [];

  return (
    <div className="grid gap-4 lg:grid-cols-[300px_minmax(0,1fr)]">
      <Card className="overflow-hidden">
        <div className="flex items-center gap-2 border-b bg-muted/30 px-4 py-2.5">
          <Database className="h-4 w-4 text-muted-foreground" />
          <span className="text-sm font-medium">Namespaces</span>
          <span className="ml-auto text-xs text-muted-foreground">{list.length}</span>
        </div>
        <div className="max-h-[560px] overflow-y-auto py-1">
          {namespaces.isLoading ? (
            <BrowseEmpty>Loading…</BrowseEmpty>
          ) : list.length === 0 ? (
            <BrowseEmpty icon={Database}>This warehouse has no namespaces yet.</BrowseEmpty>
          ) : (
            list.map((ns) => {
              const label = ns.parts.join(".");
              return (
                <NsNode
                  key={label}
                  warehouseId={warehouseId}
                  label={label}
                  open={expanded.has(label)}
                  onToggle={() => toggle(label)}
                  selectedTable={selectedNs === label ? selectedTable : null}
                  onSelectTable={(t) => {
                    setSelectedNs(label);
                    setSelectedTable(t);
                  }}
                />
              );
            })
          )}
        </div>
      </Card>

      <Card className="overflow-hidden">
        {selectedNs && selectedTable ? (
          <TableDetailView warehouseId={warehouseId} namespace={selectedNs} table={selectedTable} />
        ) : (
          <div className="flex min-h-[320px] flex-col items-center justify-center gap-2 p-6 text-sm text-muted-foreground">
            <Table2 className="h-8 w-8 opacity-30" />
            <span>Select a table to view its details.</span>
          </div>
        )}
      </Card>
    </div>
  );
}

function NsNode({
  warehouseId,
  label,
  open,
  onToggle,
  selectedTable,
  onSelectTable,
}: {
  warehouseId: string;
  label: string;
  open: boolean;
  onToggle: () => void;
  selectedTable: string | null;
  onSelectTable: (t: string) => void;
}) {
  const tabulars = useQuery<Tabular[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "tables", label],
    queryFn: () => listTabulars(warehouseId, label),
    enabled: open,
    retry: false,
  });
  return (
    <div>
      <button
        type="button"
        onClick={onToggle}
        className="flex w-full items-center gap-1.5 px-3 py-2 text-left text-sm transition-colors hover:bg-muted"
      >
        {open ? (
          <ChevronDown className="h-4 w-4 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-4 w-4 shrink-0 text-muted-foreground" />
        )}
        <Database className="h-4 w-4 shrink-0 text-muted-foreground" />
        <span className="truncate">{label}</span>
      </button>
      {open && (
        <div className="pb-1">
          {tabulars.isLoading ? (
            <p className="px-9 py-1.5 text-xs text-muted-foreground">Loading…</p>
          ) : (tabulars.data ?? []).length === 0 ? (
            <p className="px-9 py-1.5 text-xs text-muted-foreground">No tables.</p>
          ) : (
            (tabulars.data ?? []).map((t) => {
              const active = selectedTable === t.name;
              return (
                <button
                  key={t.name}
                  type="button"
                  onClick={() => onSelectTable(t.name)}
                  className={`flex w-full items-center gap-2 border-l-2 py-1.5 pl-9 pr-3 text-left text-sm transition-colors ${
                    active
                      ? "border-primary bg-primary/10 font-medium text-primary"
                      : "border-transparent hover:bg-muted"
                  }`}
                >
                  <Table2 className={`h-3.5 w-3.5 shrink-0 ${active ? "text-primary" : "text-muted-foreground"}`} />
                  <span className="flex-1 truncate">{t.name}</span>
                  {t.kind === "view" && (
                    <Badge variant="outline" className="text-[10px] uppercase">view</Badge>
                  )}
                </button>
              );
            })
          )}
        </div>
      )}
    </div>
  );
}

function TableDetailView({
  warehouseId,
  namespace,
  table,
}: {
  warehouseId: string;
  namespace: string;
  table: string;
}) {
  const detail = useQuery<TableDetail, ApiError>({
    queryKey: ["cp", "browse", warehouseId, "detail", namespace, table],
    queryFn: () => loadTableDetail(warehouseId, namespace, table),
    retry: false,
  });
  const d = detail.data;
  const transform = (t: string, col: string) => (t === "identity" ? col : `${t}(${col})`);

  return (
    <div>
      <div className="border-b p-5">
        <div className="flex items-center gap-1 text-xs text-muted-foreground">
          <span>{namespace}</span>
          <ChevronRight className="h-3 w-3" />
          <span className="font-medium text-foreground">{table}</span>
        </div>
        <h3 className="mt-1 flex items-center gap-2 text-lg font-semibold">
          <Table2 className="h-5 w-5 text-primary" />
          {table}
        </h3>
        {d && (
          <div className="mt-3 flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
            <Fact2 label="Format" value={`v${d.formatVersion}`} />
            <Fact2 label="Columns" value={String(d.columns.length)} />
            <Fact2 label="Snapshots" value={String(d.snapshotCount)} />
            <Fact2 label="Updated" value={new Date(d.lastUpdatedMs).toLocaleString()} />
            <div className="flex min-w-0 items-center gap-1.5 text-muted-foreground">
              <span className="text-xs uppercase tracking-wide">Location</span>
              <code className="max-w-[260px] truncate font-mono text-xs text-foreground">{d.location}</code>
              <CopyButton value={d.location} />
            </div>
          </div>
        )}
      </div>

      {detail.isLoading ? (
        <p className="p-6 text-center text-sm text-muted-foreground">Loading…</p>
      ) : d ? (
        <Tabs defaultValue="schema" className="p-5">
          <TabsList>
            <TabsTrigger value="schema">
              <Columns3 className="h-4 w-4" />
              Schema
            </TabsTrigger>
            <TabsTrigger value="properties">
              <FileText className="h-4 w-4" />
              Properties
            </TabsTrigger>
            <TabsTrigger value="snapshots">
              <History className="h-4 w-4" />
              Snapshots
            </TabsTrigger>
          </TabsList>

          <TabsContent value="schema" className="mt-4 space-y-4">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead>Type</TableHead>
                  <TableHead className="text-right">Required</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {d.columns.map((col) => (
                  <TableRow key={col.name}>
                    <TableCell className="font-mono">{col.name}</TableCell>
                    <TableCell>
                      <Badge variant="secondary" className="font-mono font-normal">{col.type}</Badge>
                    </TableCell>
                    <TableCell className="text-right">
                      {col.required ? (
                        <Check className="ml-auto h-4 w-4 text-teal-600" />
                      ) : (
                        <span className="text-muted-foreground">—</span>
                      )}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
            {d.partitionFields.length > 0 && (
              <div className="flex flex-wrap items-center gap-2 text-sm">
                <span className="text-muted-foreground">Partitioned by:</span>
                {d.partitionFields.map((pf) => (
                  <Badge key={pf.name} variant="outline">{transform(pf.transform, pf.sourceColumn)}</Badge>
                ))}
              </div>
            )}
            {d.sortFields.length > 0 && (
              <div className="flex flex-wrap items-center gap-2 text-sm">
                <span className="text-muted-foreground">Sorted by:</span>
                {d.sortFields.map((sf, i) => (
                  <Badge key={i} variant="outline">
                    {transform(sf.transform, sf.column)} {sf.direction}
                  </Badge>
                ))}
              </div>
            )}
          </TabsContent>

          <TabsContent value="properties" className="mt-4">
            {Object.keys(d.properties).length === 0 ? (
              <p className="py-6 text-center text-sm text-muted-foreground">No table properties.</p>
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Key</TableHead>
                    <TableHead>Value</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {Object.entries(d.properties).map(([k, v]) => (
                    <TableRow key={k}>
                      <TableCell className="font-mono text-muted-foreground">{k}</TableCell>
                      <TableCell className="break-all font-mono">{v}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </TabsContent>

          <TabsContent value="snapshots" className="mt-4">
            {d.snapshots.length === 0 ? (
              <p className="py-6 text-center text-sm text-muted-foreground">No snapshots yet.</p>
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Snapshot</TableHead>
                    <TableHead>Operation</TableHead>
                    <TableHead>When</TableHead>
                    <TableHead className="text-right">Records</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {d.snapshots.map((snap) => (
                    <TableRow key={snap.snapshotId}>
                      <TableCell className="font-mono text-xs">
                        {snap.snapshotId}
                        {snap.snapshotId === d.currentSnapshotId && (
                          <Badge variant="secondary" className="ml-2 text-[10px]">current</Badge>
                        )}
                      </TableCell>
                      <TableCell>
                        <Badge variant="outline">{snap.operation}</Badge>
                      </TableCell>
                      <TableCell className="text-sm text-muted-foreground">
                        {new Date(snap.timestampMs).toLocaleString()}
                      </TableCell>
                      <TableCell className="text-right font-mono text-xs">
                        {snap.summary["added-records"] ?? snap.summary["total-records"] ?? "—"}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </TabsContent>
        </Tabs>
      ) : (
        <p className="p-6 text-center text-sm text-destructive">Failed to load table detail.</p>
      )}
    </div>
  );
}

function Fact2({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center gap-1.5 text-muted-foreground">
      <span className="text-xs uppercase tracking-wide">{label}</span>
      <span className="text-foreground">{value}</span>
    </div>
  );
}

function BrowseEmpty({ icon: Icon, children }: { icon?: typeof Database; children: React.ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-2 px-4 py-10 text-center text-sm text-muted-foreground">
      {Icon && <Icon className="h-6 w-6 opacity-40" />}
      <span>{children}</span>
    </div>
  );
}

function CredField({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="space-y-1.5">
      <Label>{label}</Label>
      <div className="flex items-center gap-2">
        <Input readOnly value={value} className="font-mono text-xs" />
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => {
            void navigator.clipboard?.writeText(value);
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1500);
          }}
        >
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
    </div>
  );
}
