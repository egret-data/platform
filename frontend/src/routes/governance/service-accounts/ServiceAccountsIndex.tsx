import { useId, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Bot, Plus } from "lucide-react";
import { useAdminWorkspacesListWs } from "@/api/generated/admin-workspaces/admin-workspaces";
import {
  createServiceAccount,
  deleteServiceAccount,
  disableServiceAccount,
  enableServiceAccount,
  listServiceAccounts,
  serviceAccountsQueryKey,
  type ControlPlaneAccessListServiceAccountsParams,
  type CreateServiceAccountBody,
  type ServiceAccountDto,
} from "@/api/serviceAccounts";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { ENTITY_LINK_CLASS } from "@/components/ui/entity-link";
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
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { StatusBadge } from "@/components/ui/status-badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Textarea } from "@/components/ui/textarea";
import { TabError } from "@/routes/governance/tabParts";
import { serviceAccountUrl, type SaScope } from "@/routes/governance/service-accounts/serviceAccountsUrl";

const ALL = "all";

const SCOPE_LABEL: Record<SaScope, string> = { platform: "Platform", workspace: "Workspace" };

function errorText(e: unknown): string {
  return e instanceof Error && e.message ? e.message : "The request failed.";
}

/**
 * `/governance/service-accounts` (SA3): the service accounts, with a scope
 * filter (`?scope=platform|workspace`) and, for workspace scope, a workspace
 * filter (`?workspace=<id>`), both in the URL (FE-PAGE-2). The server list is
 * not paged, so there is no `?page=`.
 */
export default function ServiceAccountsIndex() {
  const [search, setSearch] = useSearchParams();
  const askedScope = search.get("scope");
  const scope: SaScope | null = askedScope === "platform" || askedScope === "workspace" ? askedScope : null;
  const workspace = scope === "workspace" ? search.get("workspace") : null;
  const scopeId = useId();
  const workspaceFilterId = useId();
  const [creating, setCreating] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const workspaces = useAdminWorkspacesListWs(undefined, { query: { retry: false } });
  const workspaceRows = workspaces.data?.status === 200 ? workspaces.data.data : [];
  const workspaceName = (id: string | null | undefined) =>
    id ? (workspaceRows.find((w) => w.id === id)?.name ?? id) : null;

  const params: ControlPlaneAccessListServiceAccountsParams = {
    ...(scope ? { scope } : {}),
    ...(workspace ? { workspace_id: workspace } : {}),
  };
  const list = useQuery({
    queryKey: serviceAccountsQueryKey(params),
    queryFn: () => listServiceAccounts(params),
  });

  const change = (changes: Record<string, string | null>) =>
    setSearch((prev) => {
      const next = new URLSearchParams(prev);
      for (const [k, v] of Object.entries(changes)) {
        if (v === null || v === "") next.delete(k);
        else next.set(k, v);
      }
      return next;
    });

  const qc = useQueryClient();
  const invalidate = () => qc.invalidateQueries({ queryKey: serviceAccountsQueryKey() });
  const toggle = useMutation({
    mutationFn: (sa: ServiceAccountDto) =>
      sa.status === "active" ? disableServiceAccount(sa.user_id) : enableServiceAccount(sa.user_id),
    onMutate: () => setActionError(null),
    onSuccess: invalidate,
    onError: (e) => setActionError(errorText(e)),
  });
  const remove = useMutation({
    mutationFn: (sa: ServiceAccountDto) => deleteServiceAccount(sa.user_id),
    onMutate: () => setActionError(null),
    onSuccess: invalidate,
    onError: (e) => setActionError(errorText(e)),
  });
  const [deleting, setDeleting] = useState<ServiceAccountDto | null>(null);

  const rows = Array.isArray(list.data) ? list.data : null;

  return (
    <ListPageShell
      title="Service accounts"
      description="Non-human principals that run jobs and call the API, with their own credentials."
      action={
        <Button onClick={() => setCreating(true)}>
          <Plus aria-hidden />
          Create service account
        </Button>
      }
    >
      <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
        <div className="flex items-center gap-2">
          <Label htmlFor={scopeId}>Scope</Label>
          <Select
            value={scope ?? ALL}
            onValueChange={(v) => change({ scope: v === ALL ? null : v, workspace: null })}
          >
            <SelectTrigger id={scopeId} className="w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL}>All scopes</SelectItem>
              <SelectItem value="platform">Platform</SelectItem>
              <SelectItem value="workspace">Workspace</SelectItem>
            </SelectContent>
          </Select>
        </div>
        {scope === "workspace" ? (
          <div className="flex items-center gap-2">
            <Label htmlFor={workspaceFilterId}>Workspace</Label>
            <Select value={workspace ?? ALL} onValueChange={(v) => change({ workspace: v === ALL ? null : v })}>
              <SelectTrigger id={workspaceFilterId} className="w-48">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL}>All workspaces</SelectItem>
                {workspaceRows.map((w) => (
                  <SelectItem key={w.id} value={w.id}>
                    {w.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        ) : null}
      </div>

      {actionError ? (
        <Alert variant="destructive" className="mt-4" role="alert">
          <AlertDescription>{actionError}</AlertDescription>
        </Alert>
      ) : null}

      <div className="mt-4">
        {list.isError ? (
          <TabError what="service accounts" onRetry={() => void list.refetch()} />
        ) : rows === null ? (
          <ListSkeleton rows={6} cols={5} />
        ) : rows.length === 0 ? (
          <EmptyState
            icon={Bot}
            title="No service accounts."
            description={scope ? "None match this filter." : "Create one to give a job or an integration its own identity."}
          />
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead>Scope</TableHead>
                <TableHead>Workspace</TableHead>
                <TableHead>Status</TableHead>
                <TableHead>Created</TableHead>
                <TableHead>
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((sa) => (
                <TableRow key={sa.user_id}>
                  <TableCell>
                    <Link to={serviceAccountUrl(sa.user_id)} className={ENTITY_LINK_CLASS}>
                      {sa.name}
                    </Link>
                  </TableCell>
                  <TableCell>
                    <Badge variant="outline">{SCOPE_LABEL[sa.scope as SaScope] ?? sa.scope}</Badge>
                  </TableCell>
                  <TableCell>{workspaceName(sa.workspace_id)}</TableCell>
                  <TableCell>
                    <StatusBadge tone={sa.status === "active" ? "success" : "inactive"}>{sa.status}</StatusBadge>
                  </TableCell>
                  <TableCell className="tabular-nums">{new Date(sa.created_at).toLocaleDateString()}</TableCell>
                  <TableCell>
                    <div className="flex justify-end gap-2">
                      <Button
                        variant="outline"
                        size="sm"
                        aria-label={`${sa.status === "active" ? "Disable" : "Enable"} ${sa.name}`}
                        disabled={toggle.isPending}
                        onClick={() => toggle.mutate(sa)}
                      >
                        {sa.status === "active" ? "Disable" : "Enable"}
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        aria-label={`Delete ${sa.name}`}
                        onClick={() => setDeleting(sa)}
                      >
                        Delete
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </div>

      <CreateDialog
        open={creating}
        onOpenChange={setCreating}
        workspaces={workspaceRows.map((w) => ({ id: w.id, name: w.name }))}
        initialScope={scope ?? "platform"}
      />
      {deleting ? (
        <ConfirmDeleteDialog
          open
          onOpenChange={(o) => {
            if (!o) setDeleting(null);
          }}
          title={`Delete ${deleting.name}?`}
          description="Its credentials stop working and it leaves every group and role. This cannot be undone."
          onConfirm={() => remove.mutate(deleting)}
        />
      ) : null}
    </ListPageShell>
  );
}

function CreateDialog({
  open,
  onOpenChange,
  workspaces,
  initialScope,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  workspaces: { id: string; name: string }[];
  initialScope: SaScope;
}) {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const ids = { scope: useId(), workspace: useId(), name: useId(), description: useId() };
  const [scope, setScope] = useState<SaScope>(initialScope);
  const [workspaceId, setWorkspaceId] = useState("");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [submitted, setSubmitted] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: (body: CreateServiceAccountBody) => createServiceAccount(body),
    onSuccess: async (sa) => {
      await qc.invalidateQueries({ queryKey: serviceAccountsQueryKey() });
      onOpenChange(false);
      if (sa && "user_id" in sa) navigate(serviceAccountUrl(sa.user_id));
    },
    onError: (e) => setServerError(errorText(e)),
  });

  const nameMissing = name.trim() === "";
  const workspaceMissing = scope === "workspace" && workspaceId === "";

  const submit = () => {
    setSubmitted(true);
    setServerError(null);
    if (nameMissing || workspaceMissing) return;
    create.mutate({
      scope,
      name: name.trim(),
      description: description.trim() === "" ? null : description.trim(),
      workspace_id: scope === "workspace" ? workspaceId : null,
    });
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Create service account</DialogTitle>
          <DialogDescription>A platform account is global; a workspace account belongs to one workspace.</DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor={ids.scope}>Account scope</Label>
            <Select value={scope} onValueChange={(v) => setScope(v as SaScope)}>
              <SelectTrigger id={ids.scope}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="platform">Platform</SelectItem>
                <SelectItem value="workspace">Workspace</SelectItem>
              </SelectContent>
            </Select>
          </div>
          {scope === "workspace" ? (
            <div className="space-y-2">
              <Label htmlFor={ids.workspace}>Account workspace</Label>
              <Select value={workspaceId} onValueChange={setWorkspaceId}>
                <SelectTrigger id={ids.workspace}>
                  <SelectValue placeholder="Choose a workspace" />
                </SelectTrigger>
                <SelectContent>
                  {workspaces.map((w) => (
                    <SelectItem key={w.id} value={w.id}>
                      {w.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {submitted && workspaceMissing ? (
                <p className="text-sm text-destructive">Choose the workspace this account belongs to.</p>
              ) : null}
            </div>
          ) : null}
          <div className="space-y-2">
            <Label htmlFor={ids.name}>Name</Label>
            <Input id={ids.name} value={name} onChange={(e) => setName(e.target.value)} />
            {submitted && nameMissing ? <p className="text-sm text-destructive">Enter a name.</p> : null}
          </div>
          <div className="space-y-2">
            <Label htmlFor={ids.description}>Description</Label>
            <Textarea id={ids.description} value={description} onChange={(e) => setDescription(e.target.value)} />
          </div>
          {serverError ? (
            <Alert variant="destructive" role="alert">
              <AlertDescription>{serverError}</AlertDescription>
            </Alert>
          ) : null}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={submit} disabled={create.isPending}>
            Create
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
