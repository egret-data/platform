import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { KeyRound, Loader2, Plus, Trash2 } from "lucide-react";

import {
  type CatalogClient,
  type CatalogClientCreated,
  type CatalogRole,
  type CreateClientInput,
  createClient,
  listClients,
  listRoles,
  revokeClient,
} from "@/api/controlPlane";
import { useMyWorkspaces } from "@/api/hooks";
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
import { clientsKey, rolesKey } from "./shared";

// ── Catalog Clients ────────────────────────────────────────────────────────────

export function ClientsTab({ warehouseId }: { warehouseId: string }) {
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
