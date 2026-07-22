import { useState, type ReactNode } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  Boxes,
  Building2,
  Cable,
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
  Power,
  ListChecks,
  RotateCcw,
  Search,
  Settings2,
  Shield,
  ShieldCheck,
  Table2,
  Tag as TagIcon,
  Trash2,
} from "lucide-react";

import {
  type CatalogClient,
  type Column,
  type CatalogClientCreated,
  type CatalogRole,
  type CreateClientInput,
  type CreateRoleInput,
  type Grant,
  type GrantInput,
  type Namespace,
  type TableDetail,
  type WarehouseConfig,
  type SearchResult,
  type WarehouseTask,
  type DeletedTabular,
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
  loadViewColumns,
  listDeletedTabulars,
  undropTabular,
  setProtection,
  getWarehouseConfig,
  setWarehouseStatus,
  setDeleteProfile,
  setFormatPolicy,
  searchTabulars,
  listWarehouseTasks,
  revokeClient,
  revokeGrant,
} from "@/api/controlPlane";
import { useMyWorkspaces } from "@/api/hooks";
import { ApiError } from "@/api/client";
import { CatalogTagDialog } from "@/components/access/CatalogTagEditor";
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
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Skeleton } from "@/components/ui/skeleton";
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
  // Active tab lives in the URL (`?tab=`) so the view is deep-linkable and
  // survives a refresh. Default preserved: "roles".
  const [sp, setSp] = useSearchParams();
  const tab = sp.get("tab") ?? "roles";
  const setTab = (v: string) =>
    setSp(
      (prev) => {
        const n = new URLSearchParams(prev);
        n.set("tab", v);
        return n;
      },
      { replace: true },
    );
  const warehouse = useQuery<Warehouse, ApiError>({
    queryKey: ["control-plane", "warehouses", id],
    queryFn: () => getWarehouse(id),
    retry: false,
  });

  const backLink = (
    <Link
      to="/control-plane/warehouses"
      className="inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
    >
      <ArrowLeft className="h-4 w-4" />
      Warehouses
    </Link>
  );

  if (warehouse.isError) {
    return (
      <div className="space-y-5">
        {backLink}
        <Alert variant="destructive">
          <AlertDescription>
            {warehouse.error?.message ?? "Couldn't load this warehouse."}
          </AlertDescription>
        </Alert>
      </div>
    );
  }

  return (
    <div className="space-y-5">
      {backLink}

      <WarehouseSummary
        warehouseId={id}
        warehouse={warehouse.data}
        isLoading={warehouse.isLoading}
      />

      <Tabs value={tab} onValueChange={setTab}>
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
          <TabsTrigger value="connect">
            <Cable className="h-4 w-4" />
            Connect
          </TabsTrigger>
          <TabsTrigger value="settings">
            <Settings2 className="h-4 w-4" />
            Settings
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
        <TabsContent value="connect" className="mt-4">
          <ConnectTab warehouse={warehouse.data} />
        </TabsContent>
        <TabsContent value="settings" className="mt-4">
          <SettingsTab warehouseId={id} />
        </TabsContent>
      </Tabs>
    </div>
  );
}

/** engine → primary (teal), notebook → success (green), external → highlight (amber). */
function consumerDot(type: string): string {
  if (type === "engine") return "bg-primary";
  if (type === "notebook") return "bg-success";
  if (type === "external") return "bg-highlight";
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
      {copied ? <Check className="h-3.5 w-3.5 text-primary" /> : <Copy className="h-3.5 w-3.5" />}
    </button>
  );
}

/** Warehouse summary band: identity + status + key facts + quick counts. */
function WarehouseSummary({
  warehouseId,
  warehouse,
  isLoading,
}: {
  warehouseId: string;
  warehouse: Warehouse | undefined;
  isLoading?: boolean;
}) {
  const roles = useQuery({ queryKey: rolesKey(warehouseId), queryFn: () => listRoles(warehouseId), retry: false });
  const clients = useQuery({ queryKey: clientsKey(warehouseId), queryFn: () => listClients(warehouseId), retry: false });
  const namespaces = useQuery({ queryKey: ["cp", "browse", warehouseId, "ns"], queryFn: () => listNamespaces(warehouseId), retry: false });
  const workspaces = useMyWorkspaces(true);
  const storage = useQuery({ queryKey: ["control-plane", "storage"], queryFn: listStorage, retry: false });

  const wsName = workspaces.data?.find((w) => w.id === warehouse?.workspaceId)?.name;
  const storageName = storage.data?.find((s) => s.id === warehouse?.storageConnId)?.name;
  const provisioned = !!warehouse?.lakekeeperWarehouseId;

  if (isLoading && !warehouse) {
    return (
      <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0 space-y-3">
          <div className="flex items-center gap-3">
            <Skeleton className="h-11 w-11 shrink-0 rounded-lg" />
            <div className="space-y-2">
              <Skeleton className="h-7 w-48" />
              <Skeleton className="h-5 w-24" />
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
            <Skeleton className="h-4 w-40" />
            <Skeleton className="h-4 w-40" />
          </div>
        </div>
        <div className="grid grid-cols-3 gap-2 sm:gap-3">
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-[70px] w-[76px] rounded-lg" />
          ))}
        </div>
      </div>
    );
  }

  const stats = [
    { icon: ShieldCheck, label: "roles", value: roles.data?.length ?? 0 },
    { icon: KeyRound, label: "clients", value: clients.data?.length ?? 0 },
    { icon: Database, label: "namespaces", value: namespaces.data?.length ?? 0 },
  ];

  return (
    <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
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
                    ? "mt-1 gap-1 border-primary/30 bg-primary/10 text-primary"
                    : "mt-1 gap-1 border-highlight/30 bg-highlight/10 text-highlight"
                }
              >
                <span className={`h-1.5 w-1.5 rounded-full ${provisioned ? "bg-primary" : "bg-highlight"}`} />
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
              <span className="mt-1 text-lg font-semibold leading-none tabular-nums">{s.value}</span>
              <span className="text-xs text-muted-foreground">{s.label}</span>
            </div>
          ))}
        </div>
    </div>
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
                      {roles.error?.message ?? "Couldn't load roles."}
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
  const [error, setError] = useState<string | null>(null);
  const [revokeTarget, setRevokeTarget] = useState<CatalogClient | null>(null);
  const revoke = useMutation<void, ApiError, string>({
    mutationFn: revokeClient,
    onSuccess: () => {
      setError(null);
      queryClient.invalidateQueries({ queryKey: clientsKey(warehouseId) });
    },
    onError: (e) => setError(e.message ?? "Failed to revoke client."),
  });
  const [addOpen, setAddOpen] = useState(false);
  const rows = clients.data ?? [];
  const roleName = (id: string) => roles.data?.find((r) => r.id === id)?.name ?? id.slice(0, 8);

  return (
    <section className="space-y-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="space-y-1">
          <h2 className="flex items-center gap-2 text-base font-semibold">
            <KeyRound className="h-4 w-4 text-muted-foreground" />
            Catalog Clients
          </h2>
          <p className="text-sm text-muted-foreground">
            Machine credentials (client id + secret) an engine or notebook uses to authenticate.
          </p>
        </div>
        <Button className="shrink-0" onClick={() => setAddOpen(true)} disabled={(roles.data ?? []).length === 0}>
          <Plus className="h-4 w-4" />
          Add client
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
                  {clients.isError ? (
                    <span className="text-destructive">
                      {clients.error?.message ?? "Couldn't load clients."}
                    </span>
                  ) : clients.isLoading ? (
                    <ListSkeleton rows={3} cols={4} />
                  ) : (roles.data ?? []).length === 0 ? (
                    "Add a role first, then create a client for it."
                  ) : (
                    "No clients yet — add one to issue credentials."
                  )}
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
                      onClick={() => setRevokeTarget(c)}
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
      </div>

      <AddClientDialog
        warehouseId={warehouseId}
        roles={roles.data ?? []}
        open={addOpen}
        onOpenChange={setAddOpen}
      />

      <ConfirmDeleteDialog
        open={revokeTarget !== null}
        onOpenChange={(o) => {
          if (!o) setRevokeTarget(null);
        }}
        title={revokeTarget ? `Revoke client «${revokeTarget.name}»?` : "Revoke client?"}
        description="Any engine or notebook authenticating with this client will immediately lose catalog access. This cannot be undone."
        confirmText="revoke"
        onConfirm={() => {
          if (revokeTarget) revoke.mutate(revokeTarget.id);
          setRevokeTarget(null);
        }}
      />
    </section>
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
  const [selectedTabular, setSelectedTabular] = useState<Tabular | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [deletedOpen, setDeletedOpen] = useState(false);
  const [protectError, setProtectError] = useState<string | null>(null);

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
  const [search, setSearch] = useState("");
  const q = search.trim();
  const results = useQuery<SearchResult[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "search", q],
    queryFn: () => searchTabulars(warehouseId, q),
    enabled: q.length > 0,
    retry: false,
  });

  return (
    <div className="space-y-4">
      {protectError && (
        <Alert variant="destructive">
          <AlertDescription>{protectError}</AlertDescription>
        </Alert>
      )}
      <div className="grid gap-4 lg:grid-cols-[300px_minmax(0,1fr)]">
      <Card className="overflow-hidden">
        <div className="flex items-center gap-2 border-b bg-muted/30 px-4 py-2.5">
          <Database className="h-4 w-4 text-muted-foreground" />
          <span className="text-sm font-medium">Namespaces</span>
          <span className="ml-auto text-xs text-muted-foreground">{list.length}</span>
          <Button
            variant="ghost"
            size="sm"
            className="h-6 gap-1 px-1.5 text-xs text-muted-foreground"
            onClick={() => setDeletedOpen(true)}
            title="Recover recently-dropped tables"
          >
            <Trash2 className="h-3.5 w-3.5" />
            Recycle bin
          </Button>
        </div>
        <div className="border-b p-2">
          <div className="relative">
            <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              className="h-8 pl-7 text-sm"
              placeholder="Search tables & views…"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </div>
        </div>
        <div className="max-h-[520px] overflow-y-auto py-1">
          {q ? (
            results.isLoading ? (
              <div className="p-2">
                <ListSkeleton rows={5} cols={1} />
              </div>
            ) : (results.data ?? []).length === 0 ? (
              <BrowseEmpty icon={Search}>No matches for “{q}”.</BrowseEmpty>
            ) : (
              (results.data ?? []).map((r, i) => {
                const nsLabel = r.namespace.join(".");
                return (
                  <button
                    key={`${nsLabel}.${r.name}.${i}`}
                    type="button"
                    onClick={() => {
                      setSelectedNs(nsLabel);
                      setSelectedTabular({ name: r.name, kind: r.kind, id: null, protected: false });
                    }}
                    className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm hover:bg-muted"
                  >
                    <Table2 className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                    <span className="flex-1 truncate">
                      <span className="text-muted-foreground">{nsLabel}.</span>
                      {r.name}
                    </span>
                    {r.kind === "view" && (
                      <Badge variant="outline" className="text-[10px] uppercase">view</Badge>
                    )}
                  </button>
                );
              })
            )
          ) : namespaces.isError ? (
            <BrowseEmpty icon={Database}>
              <span className="text-destructive">
                {namespaces.error?.message ?? "Couldn't load namespaces."}
              </span>
            </BrowseEmpty>
          ) : namespaces.isLoading ? (
            <div className="p-2">
              <ListSkeleton rows={6} cols={1} />
            </div>
          ) : list.length === 0 ? (
            <BrowseEmpty icon={Database}>This warehouse has no namespaces yet.</BrowseEmpty>
          ) : (
            list.map((ns) => {
              const label = ns.parts.join(".");
              return (
                <NsNode
                  key={label}
                  warehouseId={warehouseId}
                  ns={ns}
                  label={label}
                  open={expanded.has(label)}
                  onToggle={() => toggle(label)}
                  selectedTable={selectedNs === label ? (selectedTabular?.name ?? null) : null}
                  onSelectTable={(t) => {
                    setSelectedNs(label);
                    setSelectedTabular(t);
                  }}
                  onProtectError={setProtectError}
                />
              );
            })
          )}
        </div>
      </Card>

      <Card className="overflow-hidden">
        {selectedNs && selectedTabular ? (
          <TableDetailView
            warehouseId={warehouseId}
            namespace={selectedNs}
            tabular={selectedTabular}
            onProtectError={setProtectError}
          />
        ) : (
          <div className="flex min-h-[320px] flex-col items-center justify-center gap-2 p-6 text-sm text-muted-foreground">
            <Table2 className="h-8 w-8 opacity-30" />
            <span>Select a table or view to see its details.</span>
          </div>
        )}
      </Card>

      <DeletedTablesDialog warehouseId={warehouseId} open={deletedOpen} onOpenChange={setDeletedOpen} />
      </div>
    </div>
  );
}

/** Shield toggle that protects/unprotects a namespace/table/view from drops. */
function ProtectToggle({
  warehouseId,
  target,
  id,
  isProtected,
  className,
  onError,
}: {
  warehouseId: string;
  target: "table" | "view" | "namespace";
  id: string | null;
  isProtected: boolean;
  className?: string;
  onError?: (m: string | null) => void;
}) {
  const queryClient = useQueryClient();
  const mut = useMutation<{ protected: boolean }, ApiError, boolean>({
    mutationFn: (next) => setProtection(warehouseId, target, id!, next),
    onSuccess: () => {
      onError?.(null);
      queryClient.invalidateQueries({ queryKey: ["cp", "browse", warehouseId] });
    },
    onError: (e) => onError?.(e.message ?? "Failed to change protection."),
  });
  if (!id) return null;
  return (
    <button
      type="button"
      disabled={mut.isPending}
      onClick={(e) => {
        e.stopPropagation();
        mut.mutate(!isProtected);
      }}
      title={isProtected ? "Protected from drops — click to unprotect" : "Protect from drops"}
      aria-label={isProtected ? "Unprotect" : "Protect"}
      className={`shrink-0 rounded p-1 transition-colors ${
        isProtected
          ? "text-highlight hover:text-highlight/80"
          : "text-muted-foreground/40 opacity-0 hover:text-foreground group-hover:opacity-100"
      } ${className ?? ""}`}
    >
      {isProtected ? <ShieldCheck className="h-4 w-4" /> : <Shield className="h-4 w-4" />}
    </button>
  );
}

function NsNode({
  warehouseId,
  ns,
  label,
  open,
  onToggle,
  selectedTable,
  onSelectTable,
  onProtectError,
}: {
  warehouseId: string;
  ns: Namespace;
  label: string;
  open: boolean;
  onToggle: () => void;
  selectedTable: string | null;
  onSelectTable: (t: Tabular) => void;
  onProtectError?: (m: string | null) => void;
}) {
  const tabulars = useQuery<Tabular[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "tables", label],
    queryFn: () => listTabulars(warehouseId, label),
    enabled: open,
    retry: false,
  });
  return (
    <div>
      <div className="group flex w-full items-center pr-2 transition-colors hover:bg-muted">
        <button
          type="button"
          onClick={onToggle}
          className="flex min-w-0 flex-1 items-center gap-1.5 px-3 py-2 text-left text-sm"
        >
          {open ? (
            <ChevronDown className="h-4 w-4 shrink-0 text-muted-foreground" />
          ) : (
            <ChevronRight className="h-4 w-4 shrink-0 text-muted-foreground" />
          )}
          <Database className="h-4 w-4 shrink-0 text-muted-foreground" />
          <span className="truncate">{label}</span>
        </button>
        <ProtectToggle warehouseId={warehouseId} target="namespace" id={ns.id} isProtected={ns.protected} onError={onProtectError} />
      </div>
      {open && (
        <div className="pb-1">
          {tabulars.isLoading ? (
            <div className="px-9 py-1.5">
              <ListSkeleton rows={2} cols={1} />
            </div>
          ) : (tabulars.data ?? []).length === 0 ? (
            <p className="px-9 py-1.5 text-xs text-muted-foreground">No tables.</p>
          ) : (
            (tabulars.data ?? []).map((t) => {
              const active = selectedTable === t.name;
              return (
                <button
                  key={t.name}
                  type="button"
                  onClick={() => onSelectTable(t)}
                  className={`flex w-full items-center gap-2 border-l-2 py-1.5 pl-9 pr-3 text-left text-sm transition-colors ${
                    active
                      ? "border-primary bg-primary/10 font-medium text-primary"
                      : "border-transparent hover:bg-muted"
                  }`}
                >
                  <Table2 className={`h-3.5 w-3.5 shrink-0 ${active ? "text-primary" : "text-muted-foreground"}`} />
                  <span className="flex-1 truncate">{t.name}</span>
                  {t.protected && (
                    <ShieldCheck className="h-3.5 w-3.5 shrink-0 text-highlight" aria-label="protected" />
                  )}
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
  tabular,
  onProtectError,
}: {
  warehouseId: string;
  namespace: string;
  tabular: Tabular;
  onProtectError?: (m: string | null) => void;
}) {
  const isView = tabular.kind === "view";
  const table = tabular.name;
  const detail = useQuery<TableDetail, ApiError>({
    queryKey: ["cp", "browse", warehouseId, "detail", namespace, table],
    queryFn: () => loadTableDetail(warehouseId, namespace, table),
    enabled: !isView,
    retry: false,
  });
  const viewCols = useQuery<Column[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "view", namespace, table],
    queryFn: () => loadViewColumns(warehouseId, namespace, table),
    enabled: isView,
    retry: false,
  });
  const d = detail.data;
  const transform = (t: string, col: string) => (t === "identity" ? col : `${t}(${col})`);
  const [tagsDialog, setTagsDialog] = useState<{ open: boolean; column?: string }>({ open: false });

  return (
    <div>
      <div className="border-b p-5">
        <div className="flex items-center gap-1 text-xs text-muted-foreground">
          <span>{namespace}</span>
          <ChevronRight className="h-3 w-3" />
          <span className="font-medium text-foreground">{table}</span>
        </div>
        <div className="mt-1 flex items-center gap-2">
          <h2 className="flex items-center gap-2 text-lg font-semibold">
            <Table2 className="h-5 w-5 text-primary" />
            {table}
          </h2>
          {isView && (
            <Badge variant="outline" className="uppercase">view</Badge>
          )}
          <div className="group">
            <ProtectToggle
              warehouseId={warehouseId}
              target={tabular.kind}
              id={tabular.id}
              isProtected={tabular.protected}
              className="opacity-100"
              onError={onProtectError}
            />
          </div>
          {tabular.protected && <span className="text-xs text-highlight">Protected from drops</span>}
          <Button
            variant="ghost"
            size="sm"
            className="h-7 gap-1 px-2 text-muted-foreground"
            onClick={() => setTagsDialog({ open: true, column: undefined })}
          >
            <TagIcon className="h-3.5 w-3.5" />
            Tags
          </Button>
        </div>
        {!isView && d && (
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

      {isView ? (
        <div className="p-5">
          {viewCols.isLoading ? (
            <ListSkeleton rows={5} cols={3} />
          ) : viewCols.data ? (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead>Type</TableHead>
                  <TableHead className="text-right">Required</TableHead>
                  <TableHead className="text-right">
                    <span className="sr-only">Tags</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {viewCols.data.map((col) => (
                  <TableRow key={col.name}>
                    <TableCell className="font-mono">{col.name}</TableCell>
                    <TableCell>
                      <Badge variant="secondary" className="font-mono font-normal">{col.type}</Badge>
                    </TableCell>
                    <TableCell className="text-right">
                      {col.required ? (
                        <Check className="ml-auto h-4 w-4 text-primary" />
                      ) : (
                        <span className="text-muted-foreground">—</span>
                      )}
                    </TableCell>
                    <TableCell className="text-right">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="h-7 gap-1 px-2 text-muted-foreground"
                        onClick={() => setTagsDialog({ open: true, column: col.name })}
                        aria-label={`Tags for ${col.name}`}
                      >
                        <TagIcon className="h-3.5 w-3.5" />
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          ) : (
            <p className="text-center text-sm text-destructive">Failed to load view.</p>
          )}
        </div>
      ) : detail.isLoading ? (
        <div className="p-5">
          <ListSkeleton rows={6} cols={3} />
        </div>
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
                  <TableHead className="text-right">
                    <span className="sr-only">Tags</span>
                  </TableHead>
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
                        <Check className="ml-auto h-4 w-4 text-primary" />
                      ) : (
                        <span className="text-muted-foreground">—</span>
                      )}
                    </TableCell>
                    <TableCell className="text-right">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="h-7 gap-1 px-2 text-muted-foreground"
                        onClick={() => setTagsDialog({ open: true, column: col.name })}
                        aria-label={`Tags for ${col.name}`}
                      >
                        <TagIcon className="h-3.5 w-3.5" />
                      </Button>
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
                      <TableCell className="text-right font-mono text-xs tabular-nums">
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

      <CatalogTagDialog
        warehouseId={warehouseId}
        schema={namespace}
        table={table}
        column={tagsDialog.column}
        open={tagsDialog.open}
        onOpenChange={(open) => setTagsDialog((prev) => ({ ...prev, open }))}
      />
    </div>
  );
}

function DeletedTablesDialog({
  warehouseId,
  open,
  onOpenChange,
}: {
  warehouseId: string;
  open: boolean;
  onOpenChange: (o: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const deleted = useQuery<DeletedTabular[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "deleted"],
    queryFn: () => listDeletedTabulars(warehouseId),
    enabled: open,
    retry: false,
  });
  const undrop = useMutation<void, ApiError, DeletedTabular>({
    mutationFn: (d) => undropTabular(warehouseId, d.kind, d.id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["cp", "browse", warehouseId] }),
  });
  const rows = deleted.data ?? [];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>Recycle bin</DialogTitle>
          <DialogDescription>
            Dropped tables and views are soft-deleted and purged after a retention window. Recover
            one before it expires.
          </DialogDescription>
        </DialogHeader>
        <div className="max-h-[420px] overflow-y-auto">
          {deleted.isLoading ? (
            <ListSkeleton rows={4} cols={3} />
          ) : rows.length === 0 ? (
            <p className="py-8 text-center text-sm text-muted-foreground">Nothing to recover.</p>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead>Dropped</TableHead>
                  <TableHead>Purges</TableHead>
                  <TableHead className="text-right">
                    <span className="sr-only">Recover</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map((d) => (
                  <TableRow key={d.id}>
                    <TableCell>
                      <div className="flex items-center gap-2">
                        <Table2 className="h-4 w-4 text-muted-foreground" />
                        <span className="font-mono text-sm">
                          {d.namespace.join(".")}.{d.name}
                        </span>
                        {d.kind === "view" && (
                          <Badge variant="outline" className="text-[10px] uppercase">view</Badge>
                        )}
                      </div>
                    </TableCell>
                    <TableCell className="text-sm text-muted-foreground">
                      {new Date(d.deletedAt).toLocaleString()}
                    </TableCell>
                    <TableCell className="text-sm text-muted-foreground">
                      {new Date(d.expiration).toLocaleDateString()}
                    </TableCell>
                    <TableCell className="text-right">
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={undrop.isPending}
                        onClick={() => undrop.mutate(d)}
                      >
                        <RotateCcw className="h-4 w-4" />
                        Undrop
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </div>
      </DialogContent>
    </Dialog>
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

function SettingsTab({ warehouseId }: { warehouseId: string }) {
  const queryClient = useQueryClient();
  const config = useQuery<WarehouseConfig, ApiError>({
    queryKey: ["cp", "warehouse-config", warehouseId],
    queryFn: () => getWarehouseConfig(warehouseId),
    retry: false,
  });
  const c = config.data;
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["cp", "warehouse-config", warehouseId] });

  const statusMut = useMutation<void, ApiError, boolean>({
    mutationFn: (active) => setWarehouseStatus(warehouseId, active),
    onSuccess: invalidate,
  });
  const ttlMut = useMutation<void, ApiError, number | null>({
    mutationFn: (seconds) => setDeleteProfile(warehouseId, seconds),
    onSuccess: invalidate,
  });
  const fmtMut = useMutation<void, ApiError, number[]>({
    mutationFn: (versions) => setFormatPolicy(warehouseId, versions),
    onSuccess: invalidate,
  });

  const [ttlDays, setTtlDays] = useState("");
  const [fmt, setFmt] = useState("1,2");
  const [hardDeleteOpen, setHardDeleteOpen] = useState(false);

  const currentDays =
    c?.deleteProfileSeconds != null ? Math.round(c.deleteProfileSeconds / 86400) : null;

  return (
    <div className="space-y-8">
      <div className="max-w-4xl divide-y">
        <SettingRow icon={Power} title="Status & statistics">
          {config.isLoading ? (
            <ListSkeleton rows={2} cols={2} />
          ) : (
            <div className="space-y-3">
              <div className="flex items-center gap-2">
                <Badge
                  variant="outline"
                  className={
                    c?.active
                      ? "gap-1 border-primary/30 bg-primary/10 text-primary"
                      : "gap-1 border-muted-foreground/30 bg-muted text-muted-foreground"
                  }
                >
                  <span className={`h-1.5 w-1.5 rounded-full ${c?.active ? "bg-primary" : "bg-muted-foreground"}`} />
                  {c?.active ? "Active" : "Inactive"}
                </Badge>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={statusMut.isPending || !c}
                  onClick={() => statusMut.mutate(!c?.active)}
                >
                  {c?.active ? "Deactivate" : "Activate"}
                </Button>
              </div>
              <div className="flex gap-6 text-sm">
                <div>
                  <div className="text-xs uppercase tracking-wide text-muted-foreground">Tables</div>
                  <div className="text-lg font-semibold tabular-nums">{c?.numTables ?? "—"}</div>
                </div>
                <div>
                  <div className="text-xs uppercase tracking-wide text-muted-foreground">Views</div>
                  <div className="text-lg font-semibold tabular-nums">{c?.numViews ?? "—"}</div>
                </div>
              </div>
              {c?.statsUpdatedMs ? (
                <p className="text-xs text-muted-foreground">
                  Statistics as of {new Date(c.statsUpdatedMs).toLocaleString()}
                </p>
              ) : null}
            </div>
          )}
        </SettingRow>

        <SettingRow
          icon={Trash2}
          title="Soft-delete retention"
          description={
            currentDays != null
              ? `Dropped tables are recoverable for ${currentDays} day${currentDays === 1 ? "" : "s"}.`
              : "Hard delete — dropped tables are purged immediately (no recovery)."
          }
        >
          <div className="space-y-2">
            <div className="flex items-end gap-2">
              <div className="space-y-1.5">
                <Label htmlFor="ttl">Retention (days)</Label>
                <Input
                  id="ttl"
                  type="number"
                  min={7}
                  className="w-32"
                  placeholder={currentDays != null ? String(currentDays) : "7"}
                  value={ttlDays}
                  onChange={(e) => setTtlDays(e.target.value)}
                />
              </div>
              <Button
                size="sm"
                disabled={ttlMut.isPending}
                onClick={() => {
                  const days = Number(ttlDays || currentDays || 7);
                  ttlMut.mutate(Math.max(7, days) * 86400);
                }}
              >
                Save
              </Button>
              <Button
                variant="destructive"
                size="sm"
                disabled={ttlMut.isPending}
                onClick={() => setHardDeleteOpen(true)}
              >
                Hard delete
              </Button>
            </div>
            <p className="text-xs text-muted-foreground">Minimum ~7 days (enforced by the catalog).</p>
            {ttlMut.isError && (
              <p className="text-xs text-destructive">Retention rejected — try a longer window.</p>
            )}
            <ConfirmDeleteDialog
              open={hardDeleteOpen}
              onOpenChange={setHardDeleteOpen}
              title="Switch to hard delete?"
              description="Dropped tables will be purged immediately with no recovery window — the recycle bin will no longer hold them. This cannot be undone."
              confirmText="hard delete"
              onConfirm={() => ttlMut.mutate(null)}
            />
          </div>
        </SettingRow>

        <SettingRow
          icon={ShieldCheck}
          title="Format-version policy"
          description="Which Iceberg table format versions clients may create."
        >
          <div className="flex items-end gap-2">
            <div className="space-y-1.5">
              <Label htmlFor="fmt">Allowed versions</Label>
              <Select value={fmt} onValueChange={setFmt}>
                <SelectTrigger id="fmt" className="w-56">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="1,2">Allow v1 and v2</SelectItem>
                  <SelectItem value="2">Require v2</SelectItem>
                  <SelectItem value="2,3">Allow v2 and v3</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <Button
              size="sm"
              disabled={fmtMut.isPending}
              onClick={() => fmtMut.mutate(fmt.split(",").map(Number))}
            >
              Save
            </Button>
          </div>
        </SettingRow>

        <SettingRow
          icon={HardDrive}
          title="Storage credential"
          description={
            <>
              The S3 access key comes from this warehouse&apos;s{" "}
              <strong>storage connection</strong>. Rotate it by editing that connection in{" "}
              <em>Control Plane → Storage</em> — the new key is pushed to every warehouse using it.
            </>
          }
        >
          <Link
            to="/control-plane/storage"
            className="inline-flex items-center gap-1 whitespace-nowrap text-sm text-primary hover:underline"
          >
            <HardDrive className="h-4 w-4" />
            Manage storage connections
          </Link>
        </SettingRow>
      </div>

      <WarehouseTasksCard warehouseId={warehouseId} />
    </div>
  );
}

/** A single card-less settings row: label + description on the left, its control
 * on the right (stacking on mobile). Rows are separated by the parent's divide-y. */
function SettingRow({
  icon: Icon,
  title,
  description,
  children,
}: {
  icon: typeof Power;
  title: string;
  description?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-3 py-6 first:pt-0 last:pb-0 sm:flex-row sm:items-start sm:justify-between sm:gap-8">
      <div className="space-y-1 sm:max-w-sm">
        <h2 className="flex items-center gap-2 text-base font-semibold">
          <Icon className="h-4 w-4 text-muted-foreground" />
          {title}
        </h2>
        {description && <p className="text-sm text-muted-foreground">{description}</p>}
      </div>
      <div className="sm:shrink-0">{children}</div>
    </div>
  );
}

function WarehouseTasksCard({ warehouseId }: { warehouseId: string }) {
  const tasks = useQuery<WarehouseTask[], ApiError>({
    queryKey: ["cp", "warehouse-tasks", warehouseId],
    queryFn: () => listWarehouseTasks(warehouseId),
    retry: false,
  });
  const rows = tasks.data ?? [];
  return (
    <section className="space-y-3">
      <div className="space-y-1">
        <h2 className="flex items-center gap-2 text-base font-semibold">
          <ListChecks className="h-4 w-4 text-muted-foreground" />
          Background tasks
        </h2>
        <p className="text-sm text-muted-foreground">
          The catalog&apos;s GC queues (tabular expiry / purge / task-log cleanup) that reclaim
          storage from dropped tables.
        </p>
      </div>
      {tasks.isError ? (
        <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-destructive">
          {tasks.error?.message ?? "Couldn't load background tasks."}
        </p>
      ) : tasks.isLoading ? (
        <ListSkeleton rows={3} cols={3} />
      ) : rows.length === 0 ? (
        <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
          No pending tasks.
        </p>
      ) : (
        <div className="overflow-x-auto rounded-md border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Queue</TableHead>
                <TableHead>Entity</TableHead>
                <TableHead>Status</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((t, i) => (
                <TableRow key={i}>
                  <TableCell className="font-mono text-xs">{t.queue}</TableCell>
                  <TableCell className="font-mono text-xs">{t.entity ?? "—"}</TableCell>
                  <TableCell>
                    <Badge variant="outline">{t.status}</Badge>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}
    </section>
  );
}

function ConnectTab({ warehouse }: { warehouse: Warehouse | undefined }) {
  const storage = useQuery({
    queryKey: ["control-plane", "storage"],
    queryFn: listStorage,
    retry: false,
  });
  const sc = storage.data?.find((s) => s.id === warehouse?.storageConnId);
  const server = window.location.origin;
  const s3 = sc?.endpoint ?? "http://localhost:9000";
  // Iceberg REST clients append `/v1/...` themselves, so the catalog base URI is
  // `/catalog` (egret serves the endpoints under `/catalog/v1`).
  const catalogUri = `${server}/catalog`;
  const tokenUri = `${server}/oauth/token`;

  if (!warehouse?.lakekeeperWarehouseId) {
    return (
      <Card>
        <CardContent className="py-10 text-center text-sm text-muted-foreground">
          This warehouse isn&apos;t provisioned yet — no catalog endpoint to connect to.
        </CardContent>
      </Card>
    );
  }
  // Clients pass the warehouse NAME (the token is scoped to it); egret resolves it
  // to the lakekeeper prefix internally.
  const wh = warehouse.name;

  const pyiceberg = `from pyiceberg.catalog.rest import RestCatalog

catalog = RestCatalog(
    "egret",
    uri="${catalogUri}",
    warehouse="${wh}",
    credential="<CLIENT_ID>:<CLIENT_SECRET>",
    **{
        "oauth2-server-uri": "${tokenUri}",
        # ask egret to vend short-lived, table-scoped S3 creds:
        "header.X-Iceberg-Access-Delegation": "vended-credentials",
        "s3.endpoint": "${s3}",
    },
)

print(catalog.list_namespaces())
table = catalog.load_table("demo_ns.events")
print(table.scan().to_arrow())`;

  const trino = `# etc/catalog/egret.properties
connector.name=iceberg
iceberg.catalog.type=rest
iceberg.rest-catalog.uri=${catalogUri}
iceberg.rest-catalog.warehouse=${wh}
iceberg.rest-catalog.security=OAUTH2
iceberg.rest-catalog.oauth2.credential=<CLIENT_ID>:<CLIENT_SECRET>
iceberg.rest-catalog.oauth2.server-uri=${tokenUri}
# use egret's vended (short-lived, table-scoped) credentials:
iceberg.rest-catalog.vended-credentials-enabled=true
fs.native-s3.enabled=true
s3.endpoint=${s3}
s3.region=us-east-1
s3.path-style-access=true

-- then, in the Trino CLI:
SELECT * FROM egret.demo_ns.events;`;

  const starrocks = `-- StarRocks 4.0+
CREATE EXTERNAL CATALOG egret
PROPERTIES (
    "type" = "iceberg",
    "iceberg.catalog.type" = "rest",
    "iceberg.catalog.uri" = "${catalogUri}",
    "iceberg.catalog.warehouse" = "${wh}",
    "iceberg.catalog.security" = "OAUTH2",
    "iceberg.catalog.oauth2-server-uri" = "${tokenUri}",
    "iceberg.catalog.credential" = "<CLIENT_ID>:<CLIENT_SECRET>",
    "iceberg.catalog.vended-credentials-enabled" = "true",
    "aws.s3.endpoint" = "${s3}",
    "aws.s3.region" = "us-east-1",
    "aws.s3.enable_path_style_access" = "true"
);

SET CATALOG egret;
SELECT * FROM demo_ns.events;`;

  const engines = [
    { key: "pyiceberg", label: "PyIceberg", code: pyiceberg },
    { key: "trino", label: "Trino", code: trino },
    { key: "starrocks", label: "StarRocks", code: starrocks },
  ];

  return (
    <Card>
      <CardHeader>
        <CardTitle aria-level={2} className="flex items-center gap-2 text-lg">
          <Cable className="h-5 w-5 text-muted-foreground" />
          Connect an engine
        </CardTitle>
        <CardDescription>
          egret exposes this warehouse as an Iceberg REST catalog. Point any Iceberg-compatible
          engine at it using a Catalog Client&apos;s credentials.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-5">
        <div className="grid gap-3 sm:grid-cols-3">
          <Coord label="Catalog URI" value={catalogUri} />
          <Coord label="Warehouse" value={wh} />
          <Coord label="OAuth token URL" value={tokenUri} />
        </div>

        <Alert>
          <AlertDescription>
            The <code className="font-mono text-xs">credential</code> is{" "}
            <code className="font-mono text-xs">client_id:client_secret</code> from a{" "}
            <strong>Catalog Client</strong> (create one in the <em>Catalog Clients</em> tab); its
            role grants decide which namespaces/tables it may read or write. On a{" "}
            <code className="font-mono text-xs">vended-credentials</code> load egret hands back{" "}
            <strong>short-lived S3 credentials scoped to just that table</strong> — the configs below
            opt into them, so no static object-store keys are needed.
          </AlertDescription>
        </Alert>

        <Tabs defaultValue="pyiceberg">
          <TabsList>
            {engines.map((e) => (
              <TabsTrigger key={e.key} value={e.key}>
                {e.label}
              </TabsTrigger>
            ))}
          </TabsList>
          {engines.map((e) => (
            <TabsContent key={e.key} value={e.key} className="mt-4 space-y-2">
              <CodeBlock code={e.code} />
              <p className="text-xs text-muted-foreground">
                Replace the <code className="font-mono">&lt;…&gt;</code> placeholders with a Catalog
                Client&apos;s id/secret. If the engine runs in Docker, use the egret host reachable
                from that container instead of <code className="font-mono">{server}</code>.
              </p>
            </TabsContent>
          ))}
        </Tabs>
      </CardContent>
    </Card>
  );
}

function Coord({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border bg-muted/20 p-3">
      <div className="text-xs uppercase tracking-wide text-muted-foreground">{label}</div>
      <div className="mt-1 flex items-center gap-1.5">
        <code className="min-w-0 flex-1 truncate font-mono text-xs">{value}</code>
        <CopyButton value={value} />
      </div>
    </div>
  );
}

function CodeBlock({ code }: { code: string }) {
  return (
    <div className="relative">
      <div className="absolute right-2 top-2 z-10">
        <CopyButton value={code} />
      </div>
      <pre className="overflow-x-auto rounded-lg border bg-muted/40 p-4 pr-10 text-xs leading-relaxed">
        <code className="font-mono">{code}</code>
      </pre>
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
