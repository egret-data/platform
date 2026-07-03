import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, KeyRound, Loader2, Plus, ShieldCheck, Trash2 } from "lucide-react";

import {
  type CatalogClient,
  type CatalogClientCreated,
  type CatalogRole,
  type CreateClientInput,
  type CreateRoleInput,
  type Grant,
  type GrantInput,
  type Warehouse,
  addGrant,
  createClient,
  createRole,
  getWarehouse,
  listClients,
  listGrants,
  listRoles,
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
    <div className="space-y-4">
      <div>
        <Link
          to="/control-plane/warehouses"
          className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
        >
          <ArrowLeft className="h-4 w-4" />
          Warehouses
        </Link>
        <h1 className="mt-1 text-2xl font-semibold">{warehouse.data?.name ?? "Warehouse"}</h1>
        {warehouse.data?.lakekeeperWarehouseId && (
          <p className="font-mono text-xs text-muted-foreground">
            lakekeeper id: {warehouse.data.lakekeeperWarehouseId}
          </p>
        )}
      </div>

      <Tabs defaultValue="roles">
        <TabsList>
          <TabsTrigger value="roles">Roles &amp; grants</TabsTrigger>
          <TabsTrigger value="clients">Catalog Clients</TabsTrigger>
        </TabsList>
        <TabsContent value="roles">
          <RolesTab warehouseId={id} />
        </TabsContent>
        <TabsContent value="clients">
          <ClientsTab warehouseId={id} />
        </TabsContent>
      </Tabs>
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
          items.map((role) => <RoleGrants key={role.id} role={role} />)
        )}
      </CardContent>

      <AddRoleDialog warehouseId={warehouseId} open={addOpen} onOpenChange={setAddOpen} />
    </Card>
  );
}

function RoleGrants({ role }: { role: CatalogRole }) {
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
        <div className="flex items-center gap-2">
          <span className="font-medium">{role.name}</span>
          <Badge variant="secondary">{role.consumerType}</Badge>
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
      <AddGrantDialog roleId={role.id} open={addOpen} onOpenChange={setAddOpen} />
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

function AddGrantDialog({
  roleId,
  open,
  onOpenChange,
}: {
  roleId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const create = useMutation<void, ApiError, GrantInput>({
    mutationFn: (input) => addGrant(roleId, input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: grantsKey(roleId) }),
  });
  const [securableType, setSecurableType] = useState("catalog");
  const [privilege, setPrivilege] = useState("read");
  const [namespace, setNamespace] = useState("");
  const [tableName, setTableName] = useState("");
  const [error, setError] = useState<string | null>(null);

  function handleOpenChange(next: boolean) {
    if (!next) {
      setSecurableType("catalog");
      setPrivilege("read");
      setNamespace("");
      setTableName("");
      setError(null);
      create.reset();
    }
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    const ns = namespace.trim();
    try {
      await create.mutateAsync({
        securableType,
        privilege,
        namespace: ns.length > 0 ? ns.split(".").map((s) => s.trim()) : null,
        tableName: tableName.trim().length > 0 ? tableName.trim() : null,
      });
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
            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-1.5">
                <Label htmlFor="cpg-securable">Securable</Label>
                <Select value={securableType} onValueChange={setSecurableType}>
                  <SelectTrigger id="cpg-securable" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="catalog">catalog</SelectItem>
                    <SelectItem value="namespace">namespace</SelectItem>
                    <SelectItem value="table">table</SelectItem>
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
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="cpg-namespace">Namespace</Label>
              <Input
                id="cpg-namespace"
                value={namespace}
                onChange={(e) => setNamespace(e.target.value)}
                placeholder="Optional — dotted, e.g. analytics.events"
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="cpg-table">Table</Label>
              <Input
                id="cpg-table"
                value={tableName}
                onChange={(e) => setTableName(e.target.value)}
                placeholder="Optional"
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
