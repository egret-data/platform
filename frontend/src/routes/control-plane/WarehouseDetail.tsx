import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  Boxes,
  Building2,
  Check,
  ChevronRight,
  Columns3,
  Copy,
  Database,
  HardDrive,
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
  type Schema,
  type Tabular,
  type Warehouse,
  addGrant,
  createClient,
  createRole,
  getWarehouse,
  listClients,
  listGrants,
  listNamespaces,
  listRoles,
  listStorage,
  listTabulars,
  loadSchema,
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
  const [addOpen, setAddOpen] = useState(false);
  const items = roles.data ?? [];

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
              Roles are keyed by consumer-type; their grants gate reads/writes on the warehouse.
            </CardDescription>
          </div>
          <Button onClick={() => setAddOpen(true)}>
            <Plus className="h-4 w-4" />
            Add role
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-6">
        {items.length === 0 ? (
          <p className="py-6 text-center text-muted-foreground">
            {roles.isLoading ? "Loading…" : "No roles yet — add one to grant catalog access."}
          </p>
        ) : (
          items.map((role) => <RoleGrants key={role.id} role={role} warehouseId={warehouseId} />)
        )}
      </CardContent>

      <AddRoleDialog warehouseId={warehouseId} open={addOpen} onOpenChange={setAddOpen} />
    </Card>
  );
}

function RoleGrants({ role, warehouseId }: { role: CatalogRole; warehouseId: string }) {
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
  const [addOpen, setAddOpen] = useState(false);
  const rows = grants.data ?? [];

  return (
    <div className="rounded-lg border p-4">
      <div className="mb-3 flex items-center justify-between gap-2">
        <div className="flex items-center gap-2.5">
          <span
            className={`h-2.5 w-2.5 shrink-0 rounded-full ${consumerDot(role.consumerType)}`}
            title={role.consumerType}
          />
          <span className="font-medium">{role.name}</span>
          <Badge variant="secondary">{role.consumerType}</Badge>
          <span className="text-xs text-muted-foreground">
            {rows.length} grant{rows.length === 1 ? "" : "s"}
          </span>
        </div>
        <Button variant="outline" size="sm" onClick={() => setAddOpen(true)}>
          <Plus className="h-4 w-4" />
          Add grant
        </Button>
      </div>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Securable</TableHead>
            <TableHead>Target</TableHead>
            <TableHead>Privilege</TableHead>
            <TableHead className="text-right">
              <span className="sr-only">Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.length === 0 ? (
            <TableRow className="hover:bg-transparent">
              <TableCell colSpan={4} className="py-4 text-center text-sm text-muted-foreground">
                No grants — this role has no catalog access.
              </TableCell>
            </TableRow>
          ) : (
            rows.map((g) => (
              <TableRow key={g.id}>
                <TableCell>{g.securableType}</TableCell>
                <TableCell className="font-mono text-muted-foreground">
                  {g.namespace ? g.namespace.join(".") : "—"}
                  {g.tableName ? `.${g.tableName}` : ""}
                </TableCell>
                <TableCell>
                  <Badge variant="outline">{g.privilege}</Badge>
                </TableCell>
                <TableCell className="text-right">
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-muted-foreground hover:text-destructive"
                    onClick={() => revoke.mutate(g.id)}
                    aria-label="Revoke grant"
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </TableCell>
              </TableRow>
            ))
          )}
        </TableBody>
      </Table>
      <AddGrantDialog roleId={role.id} warehouseId={warehouseId} open={addOpen} onOpenChange={setAddOpen} />
    </div>
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
  const [error, setError] = useState<string | null>(null);

  function handleOpenChange(next: boolean) {
    if (!next) {
      setName("");
      setConsumerType("engine");
      setError(null);
      create.reset();
    }
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await create.mutateAsync({ name: name.trim(), consumerType });
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
                  <SelectItem value="manage">manage</SelectItem>
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

  const namespaces = useQuery<Namespace[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "ns"],
    queryFn: () => listNamespaces(warehouseId),
    retry: false,
  });

  const tabulars = useQuery<Tabular[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "tables", selectedNs],
    queryFn: () => listTabulars(warehouseId, selectedNs!),
    enabled: !!selectedNs,
    retry: false,
  });

  const schema = useQuery<Schema, ApiError>({
    queryKey: ["cp", "browse", warehouseId, "schema", selectedNs, selectedTable],
    queryFn: () => loadSchema(warehouseId, selectedNs!, selectedTable!),
    enabled: !!selectedNs && !!selectedTable,
    retry: false,
  });

  function handleNsSelect(ns: string) {
    setSelectedNs(ns);
    setSelectedTable(null);
  }

  const rowClass = (active: boolean) =>
    `flex w-full items-center gap-2 border-l-2 px-4 py-2 text-left text-sm transition-colors ${
      active
        ? "border-primary bg-primary/10 font-medium text-primary"
        : "border-transparent hover:bg-muted"
    }`;

  return (
    <div className="grid gap-4 md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1.5fr)]">
      {/* Namespaces */}
      <Card className="overflow-hidden">
        <ColHeader icon={Database} title="Namespaces" count={(namespaces.data ?? []).length} show />
        <div className="max-h-[440px] overflow-y-auto">
          {namespaces.isLoading ? (
            <BrowseEmpty>Loading…</BrowseEmpty>
          ) : (namespaces.data ?? []).length === 0 ? (
            <BrowseEmpty icon={Database}>This warehouse has no namespaces yet.</BrowseEmpty>
          ) : (
            (namespaces.data ?? []).map((ns) => {
              const label = ns.parts.join(".");
              const active = selectedNs === label;
              return (
                <button key={label} onClick={() => handleNsSelect(label)} className={rowClass(active)}>
                  <Database className={`h-4 w-4 shrink-0 ${active ? "text-primary" : "text-muted-foreground"}`} />
                  <span className="truncate">{label}</span>
                </button>
              );
            })
          )}
        </div>
      </Card>

      {/* Tables */}
      <Card className="overflow-hidden">
        <ColHeader icon={Table2} title="Tables" count={(tabulars.data ?? []).length} show={!!selectedNs} />
        <div className="max-h-[440px] overflow-y-auto">
          {!selectedNs ? (
            <BrowseEmpty icon={Table2}>Select a namespace.</BrowseEmpty>
          ) : tabulars.isLoading ? (
            <BrowseEmpty>Loading…</BrowseEmpty>
          ) : (tabulars.data ?? []).length === 0 ? (
            <BrowseEmpty icon={Table2}>No tables in this namespace.</BrowseEmpty>
          ) : (
            (tabulars.data ?? []).map((t) => {
              const active = selectedTable === t.name;
              return (
                <button key={t.name} onClick={() => setSelectedTable(t.name)} className={rowClass(active)}>
                  <Table2 className={`h-4 w-4 shrink-0 ${active ? "text-primary" : "text-muted-foreground"}`} />
                  <span className="flex-1 truncate">{t.name}</span>
                  <Badge variant="outline" className="text-[10px] uppercase">{t.kind}</Badge>
                </button>
              );
            })
          )}
        </div>
      </Card>

      {/* Schema */}
      <Card className="overflow-hidden">
        <ColHeader icon={Columns3} title="Schema" count={0} show={false} />
        <CardContent className="pt-4">
          {!selectedTable ? (
            <BrowseEmpty icon={Columns3}>Select a table to view its schema.</BrowseEmpty>
          ) : schema.isLoading ? (
            <BrowseEmpty>Loading…</BrowseEmpty>
          ) : schema.data ? (
            <div className="space-y-4">
              <div className="flex items-center gap-1 text-xs text-muted-foreground">
                <span>{selectedNs}</span>
                <ChevronRight className="h-3 w-3" />
                <span className="font-medium text-foreground">{selectedTable}</span>
              </div>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Type</TableHead>
                    <TableHead className="text-right">Required</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {schema.data.columns.map((col) => (
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
              {schema.data.partitionBy.length > 0 && (
                <div className="flex flex-wrap items-center gap-2 text-sm">
                  <span className="text-muted-foreground">Partitioned by:</span>
                  {schema.data.partitionBy.map((p) => (
                    <Badge key={p} variant="outline">{p}</Badge>
                  ))}
                </div>
              )}
              {schema.data.sortBy.length > 0 && (
                <div className="flex flex-wrap items-center gap-2 text-sm">
                  <span className="text-muted-foreground">Sorted by:</span>
                  {schema.data.sortBy.map((srt) => (
                    <Badge key={srt} variant="outline">{srt}</Badge>
                  ))}
                </div>
              )}
            </div>
          ) : null}
        </CardContent>
      </Card>
    </div>
  );
}

function ColHeader({
  icon: Icon,
  title,
  count,
  show,
}: {
  icon: typeof Database;
  title: string;
  count: number;
  show: boolean;
}) {
  return (
    <div className="flex items-center gap-2 border-b bg-muted/30 px-4 py-2.5">
      <Icon className="h-4 w-4 text-muted-foreground" />
      <span className="text-sm font-medium">{title}</span>
      {show && <span className="ml-auto text-xs text-muted-foreground">{count}</span>}
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
