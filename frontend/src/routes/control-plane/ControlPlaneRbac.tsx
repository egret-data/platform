import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Check,
  Copy,
  KeyRound,
  Loader2,
  Plus,
  ShieldCheck,
  Trash2,
  Users,
} from "lucide-react";

import {
  type Catalog,
  type GrantBody,
  type Principal,
  type PrincipalWithCreds,
  type PolarisRole,
  addGrant,
  assignCatalogRole,
  assignPrincipalRole,
  createCatalogRole,
  createPrincipal,
  createPrincipalRole,
  deletePrincipal,
  listCatalogRoles,
  listCatalogs,
  listPrincipalRoles,
  listPrincipals,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription } from "@/components/ui/alert";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

export const principalsQueryKey = ["control-plane", "polaris", "principals"] as const;
export const principalRolesQueryKey = [
  "control-plane",
  "polaris",
  "principal-roles",
] as const;
export const catalogsQueryKey = ["control-plane", "polaris", "catalogs"] as const;
export const catalogRolesQueryKey = (catalog: string) =>
  ["control-plane", "polaris", "catalog-roles", catalog] as const;

/**
 * Control Plane → RBAC
 *
 * Platform-admin management of the external Apache Polaris catalog's RBAC:
 * principals, principal-roles, catalog-roles and the grants that bind them.
 * Rendered under RequireGlobalAdmin (route wiring lives elsewhere).
 */
export default function ControlPlaneRbac() {
  return (
    <Tabs defaultValue="principals" className="space-y-6">
      <TabsList>
        <TabsTrigger value="principals">Principals</TabsTrigger>
        <TabsTrigger value="principal-roles">Principal roles</TabsTrigger>
        <TabsTrigger value="catalog-roles">Catalog roles</TabsTrigger>
        <TabsTrigger value="grants">Grants</TabsTrigger>
      </TabsList>

      <TabsContent value="principals">
        <PrincipalsCard />
      </TabsContent>
      <TabsContent value="principal-roles">
        <PrincipalRolesCard />
      </TabsContent>
      <TabsContent value="catalog-roles">
        <CatalogRolesCard />
      </TabsContent>
      <TabsContent value="grants">
        <GrantsCard />
      </TabsContent>
    </Tabs>
  );
}

// ── Principals ─────────────────────────────────────────────────────────────

function PrincipalsCard() {
  const principals = useQuery<Principal[], ApiError>({
    queryKey: principalsQueryKey,
    queryFn: listPrincipals,
    retry: false,
  });
  const queryClient = useQueryClient();
  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: deletePrincipal,
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: principalsQueryKey }),
  });

  const [addOpen, setAddOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Principal | null>(null);
  const [creds, setCreds] = useState<PrincipalWithCreds | null>(null);

  const rows = principals.data ?? [];

  return (
    <Card data-testid="principals-frame">
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle className="flex items-center gap-2 text-lg">
              <Users className="h-5 w-5 text-muted-foreground" />
              Principals
            </CardTitle>
            <CardDescription>
              Polaris service identities. A new principal's credentials are
              shown only once, at creation.
            </CardDescription>
          </div>
          <Button onClick={() => setAddOpen(true)}>
            <Plus className="h-4 w-4" />
            Add principal
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        {principals.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              Failed to load principals. Please try again.
            </AlertDescription>
          </Alert>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead className="text-right">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.length === 0 ? (
                <TableRow className="hover:bg-transparent">
                  <TableCell
                    colSpan={2}
                    className="py-10 text-center text-muted-foreground"
                  >
                    {principals.isLoading
                      ? "Loading…"
                      : "No principals yet — add one to get started."}
                  </TableCell>
                </TableRow>
              ) : (
                rows.map((p) => (
                  <TableRow key={p.name}>
                    <TableCell className="font-medium">{p.name}</TableCell>
                    <TableCell className="text-right">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-muted-foreground hover:text-destructive"
                        onClick={() => setDeleteTarget(p)}
                        aria-label={`Delete ${p.name}`}
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

      <AddPrincipalDialog
        open={addOpen}
        onOpenChange={setAddOpen}
        onCreated={setCreds}
      />

      <PrincipalCredsDialog
        creds={creds}
        onOpenChange={(o) => {
          if (!o) setCreds(null);
        }}
      />

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={
          deleteTarget ? `Delete «${deleteTarget.name}»?` : "Delete principal?"
        }
        description="This principal will be removed from Polaris. Any credentials it holds stop working immediately."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget.name);
        }}
      />
    </Card>
  );
}

function AddPrincipalDialog({
  open,
  onOpenChange,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated: (creds: PrincipalWithCreds) => void;
}) {
  const queryClient = useQueryClient();
  const createMutation = useMutation<PrincipalWithCreds, ApiError, string>({
    mutationFn: (name) => createPrincipal(name),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: principalsQueryKey }),
  });

  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setName("");
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
    try {
      const creds = await createMutation.mutateAsync(name.trim());
      handleOpenChange(false);
      onCreated(creds);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to create the principal.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Add principal</DialogTitle>
            <DialogDescription>
              Create a new Polaris service identity. Its client credentials are
              shown only once, immediately after creation.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cpr-principal-name">Name</Label>
              <Input
                id="cpr-principal-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. egret-ws-analytics"
                required
                autoFocus
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
            <Button type="submit" disabled={createMutation.isPending}>
              {createMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Add principal
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function PrincipalCredsDialog({
  creds,
  onOpenChange,
}: {
  creds: PrincipalWithCreds | null;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={creds !== null} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <KeyRound className="h-5 w-5 text-muted-foreground" />
            Credentials for «{creds?.name ?? ""}»
          </DialogTitle>
          <DialogDescription>
            These credentials are shown only once — save the secret now. You
            will not be able to retrieve it again.
          </DialogDescription>
        </DialogHeader>

        <div className="my-4 space-y-4">
          <Alert variant="destructive">
            <AlertDescription>
              Shown only once — copy and store the client secret before closing
              this dialog. It cannot be recovered.
            </AlertDescription>
          </Alert>

          <CopyableField label="Client ID" value={creds?.clientId ?? ""} />
          <CopyableField
            label="Client secret"
            value={creds?.clientSecret ?? ""}
          />
        </div>

        <DialogFooter>
          <Button onClick={() => onOpenChange(false)}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function CopyableField({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard unavailable — leave the value on screen for manual copy.
    }
  }

  return (
    <div className="space-y-1.5">
      <Label>{label}</Label>
      <div className="flex items-center gap-2">
        <code className="min-w-0 flex-1 truncate rounded-md border bg-muted px-3 py-2 font-mono text-sm">
          {value}
        </code>
        <Button
          type="button"
          variant="outline"
          size="icon"
          onClick={copy}
          aria-label={`Copy ${label}`}
        >
          {copied ? (
            <Check className="h-4 w-4" />
          ) : (
            <Copy className="h-4 w-4" />
          )}
        </Button>
      </div>
    </div>
  );
}

// ── Principal roles ──────────────────────────────────────────────────────────

function PrincipalRolesCard() {
  const roles = useQuery<PolarisRole[], ApiError>({
    queryKey: principalRolesQueryKey,
    queryFn: listPrincipalRoles,
    retry: false,
  });

  const [addOpen, setAddOpen] = useState(false);
  const [assignTarget, setAssignTarget] = useState<PolarisRole | null>(null);

  const rows = roles.data ?? [];

  return (
    <Card data-testid="principal-roles-frame">
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle className="flex items-center gap-2 text-lg">
              <ShieldCheck className="h-5 w-5 text-muted-foreground" />
              Principal roles
            </CardTitle>
            <CardDescription>
              Roles granted to principals. Assign a role to a principal to give
              it the role's catalog access.
            </CardDescription>
          </div>
          <Button onClick={() => setAddOpen(true)}>
            <Plus className="h-4 w-4" />
            Add role
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        {roles.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              Failed to load principal roles. Please try again.
            </AlertDescription>
          </Alert>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead className="text-right">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.length === 0 ? (
                <TableRow className="hover:bg-transparent">
                  <TableCell
                    colSpan={2}
                    className="py-10 text-center text-muted-foreground"
                  >
                    {roles.isLoading
                      ? "Loading…"
                      : "No principal roles yet — add one to get started."}
                  </TableCell>
                </TableRow>
              ) : (
                rows.map((r) => (
                  <TableRow key={r.name}>
                    <TableCell className="font-medium">{r.name}</TableCell>
                    <TableCell className="text-right">
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => setAssignTarget(r)}
                        aria-label={`Assign ${r.name} to a principal`}
                      >
                        Assign to principal…
                      </Button>
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        )}
      </CardContent>

      <AddPrincipalRoleDialog open={addOpen} onOpenChange={setAddOpen} />

      <AssignPrincipalRoleDialog
        role={assignTarget}
        onOpenChange={(o) => {
          if (!o) setAssignTarget(null);
        }}
      />
    </Card>
  );
}

function AddPrincipalRoleDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const createMutation = useMutation<void, ApiError, string>({
    mutationFn: createPrincipalRole,
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: principalRolesQueryKey }),
  });

  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setName("");
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
    try {
      await createMutation.mutateAsync(name.trim());
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to create the role.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Add principal role</DialogTitle>
            <DialogDescription>
              Create a new principal role in Polaris.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cpr-role-name">Name</Label>
              <Input
                id="cpr-role-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. data-engineer"
                required
                autoFocus
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
            <Button type="submit" disabled={createMutation.isPending}>
              {createMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Add role
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function AssignPrincipalRoleDialog({
  role,
  onOpenChange,
}: {
  role: PolarisRole | null;
  onOpenChange: (open: boolean) => void;
}) {
  const open = role !== null;
  const principals = useQuery<Principal[], ApiError>({
    queryKey: principalsQueryKey,
    queryFn: listPrincipals,
    enabled: open,
    retry: false,
  });
  const assignMutation = useMutation<void, ApiError, string>({
    mutationFn: (principal) => assignPrincipalRole(principal, role!.name),
  });

  const [principal, setPrincipal] = useState("");
  const [error, setError] = useState<string | null>(null);

  function handleOpenChange(next: boolean) {
    if (!next) {
      setPrincipal("");
      setError(null);
      assignMutation.reset();
    }
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    if (!principal) {
      setError("Select a principal.");
      return;
    }
    try {
      await assignMutation.mutateAsync(principal);
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to assign the role.");
    }
  }

  const items = principals.data ?? [];

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Assign «{role?.name ?? ""}» to a principal</DialogTitle>
            <DialogDescription>
              Grant this principal role to a Polaris principal.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cpr-assign-principal">Principal</Label>
              <Select value={principal} onValueChange={setPrincipal}>
                <SelectTrigger id="cpr-assign-principal" className="w-full">
                  <SelectValue placeholder="Select a principal" />
                </SelectTrigger>
                <SelectContent>
                  {items.map((p) => (
                    <SelectItem key={p.name} value={p.name}>
                      {p.name}
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
            <Button type="submit" disabled={assignMutation.isPending}>
              {assignMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Assign
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ── Catalog roles ────────────────────────────────────────────────────────────

function CatalogRolesCard() {
  const catalogs = useQuery<Catalog[], ApiError>({
    queryKey: catalogsQueryKey,
    queryFn: listCatalogs,
    retry: false,
  });
  const [catalog, setCatalog] = useState("");

  const roles = useQuery<PolarisRole[], ApiError>({
    queryKey: catalogRolesQueryKey(catalog),
    queryFn: () => listCatalogRoles(catalog),
    enabled: catalog !== "",
    retry: false,
  });

  const [addOpen, setAddOpen] = useState(false);
  const [assignTarget, setAssignTarget] = useState<PolarisRole | null>(null);

  const catalogItems = catalogs.data ?? [];
  const rows = roles.data ?? [];

  return (
    <Card data-testid="catalog-roles-frame">
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle className="flex items-center gap-2 text-lg">
              <ShieldCheck className="h-5 w-5 text-muted-foreground" />
              Catalog roles
            </CardTitle>
            <CardDescription>
              Roles scoped to a catalog. Grants attach to catalog roles; assign
              a catalog role to a principal role to propagate its access.
            </CardDescription>
          </div>
          <Button onClick={() => setAddOpen(true)} disabled={catalog === ""}>
            <Plus className="h-4 w-4" />
            Add catalog role
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="space-y-1.5">
          <Label htmlFor="cpr-cr-catalog">Catalog</Label>
          <Select value={catalog} onValueChange={setCatalog}>
            <SelectTrigger id="cpr-cr-catalog" className="w-full sm:w-72">
              <SelectValue placeholder="Select a catalog" />
            </SelectTrigger>
            <SelectContent>
              {catalogItems.map((c) => (
                <SelectItem key={c.name} value={c.name}>
                  {c.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        {catalog === "" ? (
          <p className="py-6 text-center text-muted-foreground">
            Select a catalog to view its roles.
          </p>
        ) : roles.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              Failed to load catalog roles. Please try again.
            </AlertDescription>
          </Alert>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead className="text-right">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.length === 0 ? (
                <TableRow className="hover:bg-transparent">
                  <TableCell
                    colSpan={2}
                    className="py-10 text-center text-muted-foreground"
                  >
                    {roles.isLoading
                      ? "Loading…"
                      : "No catalog roles yet — add one to get started."}
                  </TableCell>
                </TableRow>
              ) : (
                rows.map((r) => (
                  <TableRow key={r.name}>
                    <TableCell className="font-medium">{r.name}</TableCell>
                    <TableCell className="text-right">
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => setAssignTarget(r)}
                        aria-label={`Assign ${r.name} to a principal role`}
                      >
                        Assign to principal-role…
                      </Button>
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        )}
      </CardContent>

      <AddCatalogRoleDialog
        catalog={catalog}
        open={addOpen}
        onOpenChange={setAddOpen}
      />

      <AssignCatalogRoleDialog
        catalog={catalog}
        catalogRole={assignTarget}
        onOpenChange={(o) => {
          if (!o) setAssignTarget(null);
        }}
      />
    </Card>
  );
}

function AddCatalogRoleDialog({
  catalog,
  open,
  onOpenChange,
}: {
  catalog: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const createMutation = useMutation<void, ApiError, string>({
    mutationFn: (name) => createCatalogRole(catalog, name),
    onSuccess: () =>
      queryClient.invalidateQueries({
        queryKey: catalogRolesQueryKey(catalog),
      }),
  });

  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setName("");
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
    try {
      await createMutation.mutateAsync(name.trim());
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to create the catalog role.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Add catalog role</DialogTitle>
            <DialogDescription>
              Create a new role in catalog «{catalog}».
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cpr-cr-name">Name</Label>
              <Input
                id="cpr-cr-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. reader"
                required
                autoFocus
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
            <Button type="submit" disabled={createMutation.isPending}>
              {createMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Add catalog role
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function AssignCatalogRoleDialog({
  catalog,
  catalogRole,
  onOpenChange,
}: {
  catalog: string;
  catalogRole: PolarisRole | null;
  onOpenChange: (open: boolean) => void;
}) {
  const open = catalogRole !== null;
  const principalRoles = useQuery<PolarisRole[], ApiError>({
    queryKey: principalRolesQueryKey,
    queryFn: listPrincipalRoles,
    enabled: open,
    retry: false,
  });
  const assignMutation = useMutation<void, ApiError, string>({
    mutationFn: (principalRole) =>
      assignCatalogRole(principalRole, catalog, catalogRole!.name),
  });

  const [principalRole, setPrincipalRole] = useState("");
  const [error, setError] = useState<string | null>(null);

  function handleOpenChange(next: boolean) {
    if (!next) {
      setPrincipalRole("");
      setError(null);
      assignMutation.reset();
    }
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    if (!principalRole) {
      setError("Select a principal role.");
      return;
    }
    try {
      await assignMutation.mutateAsync(principalRole);
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to assign the catalog role.");
    }
  }

  const items = principalRoles.data ?? [];

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>
              Assign «{catalogRole?.name ?? ""}» to a principal role
            </DialogTitle>
            <DialogDescription>
              Bind this catalog role to a principal role so principals holding
              it inherit the catalog access.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cpr-assign-pr">Principal role</Label>
              <Select value={principalRole} onValueChange={setPrincipalRole}>
                <SelectTrigger id="cpr-assign-pr" className="w-full">
                  <SelectValue placeholder="Select a principal role" />
                </SelectTrigger>
                <SelectContent>
                  {items.map((r) => (
                    <SelectItem key={r.name} value={r.name}>
                      {r.name}
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
            <Button type="submit" disabled={assignMutation.isPending}>
              {assignMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Assign
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ── Grants ───────────────────────────────────────────────────────────────────

const GRANT_KINDS: GrantBody["kind"][] = [
  "catalog",
  "namespace",
  "table",
  "view",
];

function GrantsCard() {
  const catalogs = useQuery<Catalog[], ApiError>({
    queryKey: catalogsQueryKey,
    queryFn: listCatalogs,
    retry: false,
  });
  const [catalog, setCatalog] = useState("");

  const roles = useQuery<PolarisRole[], ApiError>({
    queryKey: catalogRolesQueryKey(catalog),
    queryFn: () => listCatalogRoles(catalog),
    enabled: catalog !== "",
    retry: false,
  });
  const [catalogRole, setCatalogRole] = useState("");

  const addMutation = useMutation<void, ApiError, GrantBody>({
    mutationFn: (grant) => addGrant(catalog, catalogRole, grant),
  });

  const [kind, setKind] = useState<GrantBody["kind"]>("catalog");
  const [privilege, setPrivilege] = useState("");
  const [namespace, setNamespace] = useState("");
  const [table, setTable] = useState("");
  const [view, setView] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [ok, setOk] = useState(false);

  const catalogItems = catalogs.data ?? [];
  const roleItems = roles.data ?? [];
  const showNamespace = kind !== "catalog";

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setOk(false);

    if (!catalog) {
      setError("Select a catalog.");
      return;
    }
    if (!catalogRole) {
      setError("Select a catalog role.");
      return;
    }

    const grant: GrantBody = { kind, privilege: privilege.trim() };
    if (showNamespace) {
      const parts = namespace
        .split(",")
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      if (parts.length > 0) grant.namespace = parts;
    }
    if (kind === "table") grant.table = table.trim();
    if (kind === "view") grant.view = view.trim();

    try {
      await addMutation.mutateAsync(grant);
      setOk(true);
      setPrivilege("");
      setNamespace("");
      setTable("");
      setView("");
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to add the grant.");
    }
  }

  return (
    <Card data-testid="grants-frame">
      <CardHeader>
        <div className="space-y-1.5">
          <CardTitle className="flex items-center gap-2 text-lg">
            <KeyRound className="h-5 w-5 text-muted-foreground" />
            Grants
          </CardTitle>
          <CardDescription>
            Attach a privilege on a securable to a catalog role. Polaris exposes
            no grant-listing endpoint, so grants are write-only here.
          </CardDescription>
        </div>
      </CardHeader>
      <CardContent>
        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="grid gap-4 sm:grid-cols-2">
            <div className="space-y-1.5">
              <Label htmlFor="cpr-grant-catalog">Catalog</Label>
              <Select
                value={catalog}
                onValueChange={(v) => {
                  setCatalog(v);
                  setCatalogRole("");
                }}
              >
                <SelectTrigger id="cpr-grant-catalog" className="w-full">
                  <SelectValue placeholder="Select a catalog" />
                </SelectTrigger>
                <SelectContent>
                  {catalogItems.map((c) => (
                    <SelectItem key={c.name} value={c.name}>
                      {c.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cpr-grant-role">Catalog role</Label>
              <Select
                value={catalogRole}
                onValueChange={setCatalogRole}
                disabled={catalog === ""}
              >
                <SelectTrigger id="cpr-grant-role" className="w-full">
                  <SelectValue
                    placeholder={
                      catalog === ""
                        ? "Select a catalog first"
                        : "Select a catalog role"
                    }
                  />
                </SelectTrigger>
                <SelectContent>
                  {roleItems.map((r) => (
                    <SelectItem key={r.name} value={r.name}>
                      {r.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cpr-grant-kind">Securable kind</Label>
              <Select
                value={kind}
                onValueChange={(v) => setKind(v as GrantBody["kind"])}
              >
                <SelectTrigger id="cpr-grant-kind" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {GRANT_KINDS.map((k) => (
                    <SelectItem key={k} value={k}>
                      {k}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cpr-grant-privilege">Privilege</Label>
              <Input
                id="cpr-grant-privilege"
                value={privilege}
                onChange={(e) => setPrivilege(e.target.value)}
                placeholder="e.g. TABLE_READ_DATA"
                required
              />
            </div>

            {showNamespace && (
              <div className="space-y-1.5">
                <Label htmlFor="cpr-grant-namespace">Namespace</Label>
                <Input
                  id="cpr-grant-namespace"
                  value={namespace}
                  onChange={(e) => setNamespace(e.target.value)}
                  placeholder="Comma-separated, e.g. sales, orders"
                />
              </div>
            )}

            {kind === "table" && (
              <div className="space-y-1.5">
                <Label htmlFor="cpr-grant-table">Table</Label>
                <Input
                  id="cpr-grant-table"
                  value={table}
                  onChange={(e) => setTable(e.target.value)}
                  placeholder="e.g. line_items"
                />
              </div>
            )}

            {kind === "view" && (
              <div className="space-y-1.5">
                <Label htmlFor="cpr-grant-view">View</Label>
                <Input
                  id="cpr-grant-view"
                  value={view}
                  onChange={(e) => setView(e.target.value)}
                  placeholder="e.g. daily_totals"
                />
              </div>
            )}
          </div>

          {error && (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          {ok && (
            <Alert>
              <AlertDescription>Grant added.</AlertDescription>
            </Alert>
          )}

          <div className="flex justify-end">
            <Button type="submit" disabled={addMutation.isPending}>
              {addMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              <Plus className="h-4 w-4" />
              Add grant
            </Button>
          </div>
        </form>
      </CardContent>
    </Card>
  );
}
