import { useEffect, useMemo, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArchiveRestore,
  ArrowLeft,
  Boxes,
  Cpu,
  Pencil,
  Trash2,
  UserCog,
} from "lucide-react";
import { toast } from "sonner";

import {
  getAdminWorkspacesGetWsQueryKey,
  getAdminWorkspacesListWsQueryKey,
  getAdminWorkspacesWorkspaceStatsQueryKey,
  useAdminWorkspacesArchiveWs,
  useAdminWorkspacesGetWs,
  useAdminWorkspacesRestoreWs,
  useAdminWorkspacesTransferOwner,
  useAdminWorkspacesUpdateWs,
  useAdminWorkspacesWorkspaceStats,
} from "@/api/generated/admin-workspaces/admin-workspaces";
import { useControlPlaneAuditListAudit } from "@/api/generated/control-plane-audit/control-plane-audit";
import {
  getControlPlaneEnginesListWorkspaceEnginesQueryKey,
  useControlPlaneEnginesAssignWorkspaceEngine,
  useControlPlaneEnginesListEngines,
  useControlPlaneEnginesListWorkspaceEngines,
  useControlPlaneEnginesUnassignWorkspaceEngine,
} from "@/api/generated/control-plane-engines/control-plane-engines";
import type { EngineDto } from "@/api/generated/model/engineDto";
import { listUsers } from "@/api/access";
import { listStorage, listWarehouses } from "@/api/controlPlane";
import { useApproveRequest, useDenyRequest, useWorkspaceRequests } from "@/api/accessRequests";
import { MembersPanel } from "@/components/access/MembersPanel";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { fullTimestamp, timeAgo } from "@/lib/relativeTime";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

const TABS = ["overview", "members", "resources", "requests", "activity"] as const;
type TabKey = (typeof TABS)[number];

/**
 * Control Plane → Workspaces → detail.
 *
 * A per-workspace admin cockpit for platform admins. Tabs: Overview,
 * Members (reusing `MembersPanel`), Resources (deep-links into the CP resource
 * pages), Access requests, and Activity (the audit log filtered to this
 * workspace). `platform_admin` bypasses the per-workspace gates, so the
 * workspace-scoped APIs (members, access-requests) work here for any workspace.
 */
export default function WorkspaceDetail() {
  const id = useParams().id!;
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [searchParams, setSearchParams] = useSearchParams();

  const wsQ = useAdminWorkspacesGetWs(id, { query: { retry: false } });
  const ws = wsQ.data?.status === 200 ? wsQ.data.data : undefined;
  const statsQ = useAdminWorkspacesWorkspaceStats(id, { query: { retry: false } });
  const stats = statsQ.data?.status === 200 ? statsQ.data.data : undefined;

  const [renameOpen, setRenameOpen] = useState(false);
  const [archiveOpen, setArchiveOpen] = useState(false);
  const [transferOpen, setTransferOpen] = useState(false);

  const tabParam = searchParams.get("tab");
  const tab: TabKey = (TABS as readonly string[]).includes(tabParam ?? "")
    ? (tabParam as TabKey)
    : "overview";
  const setTab = (t: string) =>
    setSearchParams((p) => {
      p.set("tab", t);
      return p;
    });

  const invalidate = () => {
    qc.invalidateQueries({ queryKey: getAdminWorkspacesGetWsQueryKey(id) });
    qc.invalidateQueries({ queryKey: getAdminWorkspacesListWsQueryKey() });
    // Transfer adds the new owner as a member, so the Overview member stat must refresh too.
    qc.invalidateQueries({ queryKey: getAdminWorkspacesWorkspaceStatsQueryKey(id) });
  };

  const archiveMut = useAdminWorkspacesArchiveWs({
    mutation: {
      onSuccess: () => {
        toast.success("Workspace archived");
        invalidate();
      },
      onError: (e) => toast.error(String(e)),
    },
  });
  const restoreMut = useAdminWorkspacesRestoreWs({
    mutation: {
      onSuccess: () => {
        toast.success("Workspace restored");
        invalidate();
      },
      onError: (e) => toast.error(String(e)),
    },
  });

  if (wsQ.isLoading) {
    return <p className="p-6 text-sm text-muted-foreground">Loading workspace…</p>;
  }
  if (!ws) {
    return (
      <div className="space-y-4 p-6">
        <p className="text-sm text-destructive" role="alert">
          Workspace not found.
        </p>
        <Button variant="outline" size="sm" onClick={() => navigate("/control-plane/workspaces")}>
          <ArrowLeft className="mr-1 h-4 w-4" /> Back to workspaces
        </Button>
      </div>
    );
  }

  const isArchived = ws.status === "archived";

  return (
    <div className="space-y-5">
      <div>
        <Link
          to="/control-plane/workspaces"
          className="inline-flex items-center text-sm text-muted-foreground hover:text-foreground"
        >
          <ArrowLeft className="mr-1 h-4 w-4" /> Workspaces
        </Link>
        <div className="mt-2 flex flex-wrap items-start justify-between gap-3">
          <div className="flex items-center gap-3">
            <h1 className="text-2xl font-semibold leading-tight">{ws.name}</h1>
            <code className="rounded bg-muted px-2 py-0.5 font-mono text-sm text-muted-foreground">
              {ws.slug}
            </code>
            {isArchived ? (
              <Badge variant="secondary">archived</Badge>
            ) : (
              <Badge
                className="border-transparent"
                style={{ backgroundColor: "var(--success)", color: "var(--success-foreground)" }}
              >
                active
              </Badge>
            )}
          </div>
          <div className="flex items-center gap-2">
            <Button size="sm" variant="outline" onClick={() => setRenameOpen(true)}>
              <Pencil className="mr-1 h-4 w-4" /> Rename
            </Button>
            <Button size="sm" variant="outline" onClick={() => setTransferOpen(true)}>
              <UserCog className="mr-1 h-4 w-4" /> Transfer owner
            </Button>
            {isArchived ? (
              <Button size="sm" variant="outline" onClick={() => restoreMut.mutate({ id })}>
                <ArchiveRestore className="mr-1 h-4 w-4" /> Restore
              </Button>
            ) : (
              <Button
                size="sm"
                variant="outline"
                className="text-destructive hover:text-destructive"
                onClick={() => setArchiveOpen(true)}
              >
                <Trash2 className="mr-1 h-4 w-4" /> Archive
              </Button>
            )}
          </div>
        </div>
      </div>

      <Tabs value={tab} onValueChange={setTab}>
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="members">Members</TabsTrigger>
          <TabsTrigger value="resources">Resources</TabsTrigger>
          <TabsTrigger value="requests">Access requests</TabsTrigger>
          <TabsTrigger value="activity">Activity</TabsTrigger>
        </TabsList>

        <TabsContent value="overview" className="mt-4">
          <OverviewTab
            owner={ws.owner_name}
            createdAt={ws.created_at}
            status={ws.status}
            memberCount={stats?.member_count}
            queryCount={stats?.query_count}
          />
        </TabsContent>

        <TabsContent value="members" className="mt-4">
          <MembersPanel workspaceId={ws.slug} />
        </TabsContent>

        <TabsContent value="resources" className="mt-4">
          <ResourcesTab workspaceId={id} />
        </TabsContent>

        <TabsContent value="requests" className="mt-4">
          <RequestsTab slug={ws.slug} />
        </TabsContent>

        <TabsContent value="activity" className="mt-4">
          <ActivityTab workspaceId={id} />
        </TabsContent>
      </Tabs>

      <RenameDialog
        open={renameOpen}
        onOpenChange={setRenameOpen}
        id={id}
        slug={ws.slug}
        currentName={ws.name}
        onDone={invalidate}
      />

      <TransferOwnerDialog
        open={transferOpen}
        onOpenChange={setTransferOpen}
        id={id}
        onDone={invalidate}
      />

      <AlertDialog open={archiveOpen} onOpenChange={setArchiveOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Archive workspace?</AlertDialogTitle>
            <AlertDialogDescription>
              <span className="font-medium">{ws.name}</span> will be archived and hidden from active
              lookups. You can restore it later.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                archiveMut.mutate({ id });
                setArchiveOpen(false);
              }}
              className="bg-destructive text-white hover:bg-destructive/90"
            >
              Archive
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function OverviewTab({
  owner,
  createdAt,
  status,
  memberCount,
  queryCount,
}: {
  owner?: string | null;
  createdAt: string;
  status: string;
  memberCount?: number;
  queryCount?: number;
}) {
  return (
    <div className="space-y-4">
      <SummaryStrip
        items={[
          { label: "members", value: memberCount ?? 0 },
          { label: "queries run", value: queryCount ?? 0 },
        ]}
      />
      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Overview</CardTitle>
          <CardDescription>Ownership and lifecycle for this workspace.</CardDescription>
        </CardHeader>
        <CardContent>
          <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <div>
              <dt className="text-xs uppercase tracking-wide text-muted-foreground">Owner</dt>
              <dd className="text-sm">{owner ?? "—"}</dd>
            </div>
            <div>
              <dt className="text-xs uppercase tracking-wide text-muted-foreground">Created</dt>
              <dd className="text-sm" title={fullTimestamp(createdAt)}>
                {timeAgo(createdAt)}
              </dd>
            </div>
            <div>
              <dt className="text-xs uppercase tracking-wide text-muted-foreground">Status</dt>
              <dd className="text-sm capitalize">{status}</dd>
            </div>
          </dl>
        </CardContent>
      </Card>
    </div>
  );
}

function ResourcesTab({ workspaceId }: { workspaceId: string }) {
  // The resources that actually belong to this workspace: its warehouses and the
  // engines assigned to it. Storage connections and secrets are platform-global
  // (migrations 0050 / 0064), so they are not listed per-workspace here.
  return (
    <div className="space-y-6">
      <WarehousesSection workspaceId={workspaceId} />
      <AssignedEnginesSection workspaceId={workspaceId} />
      <p className="text-xs text-muted-foreground">
        Storage connections and secrets are managed platform-wide in the Infrastructure section.
      </p>
    </div>
  );
}

/** Iceberg warehouses owned by this workspace (read-only list; management lives on
 * the Warehouses page). */
function WarehousesSection({ workspaceId }: { workspaceId: string }) {
  const whQ = useQuery({
    queryKey: ["control-plane", "warehouses"],
    queryFn: listWarehouses,
    retry: false,
  });
  const storageQ = useQuery({
    queryKey: ["control-plane", "storage"],
    queryFn: listStorage,
    retry: false,
  });
  const warehouses = (whQ.data ?? []).filter((w) => w.workspaceId === workspaceId);
  const storageName = (id: string) => storageQ.data?.find((s) => s.id === id)?.name ?? "—";

  return (
    <Card>
      <CardHeader>
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 space-y-1.5">
            <CardTitle className="text-lg">Warehouses</CardTitle>
            <CardDescription>Iceberg catalogs owned by this workspace.</CardDescription>
          </div>
          <Button asChild size="sm" variant="outline">
            <Link to={`/control-plane/warehouses?workspace=${workspaceId}`}>Manage</Link>
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        {whQ.isLoading ? (
          <p className="text-sm text-muted-foreground">Loading warehouses…</p>
        ) : warehouses.length === 0 ? (
          <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
            No warehouses in this workspace yet.
          </p>
        ) : (
          <div className="rounded-md border">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead>Storage</TableHead>
                  <TableHead className="text-right">Status</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {warehouses.map((w) => (
                  <TableRow key={w.id}>
                    <TableCell>
                      <div className="flex min-w-0 items-center gap-2.5">
                        <Boxes className="h-4 w-4 shrink-0 text-muted-foreground" />
                        <Link
                          to={`/control-plane/warehouses/${w.id}`}
                          className="truncate font-mono font-medium text-primary hover:underline"
                        >
                          {w.name}
                        </Link>
                      </div>
                    </TableCell>
                    <TableCell className="text-muted-foreground">
                      {storageName(w.storageConnId)}
                    </TableCell>
                    <TableCell className="text-right">
                      <Badge variant={w.lakekeeperWarehouseId ? "secondary" : "outline"}>
                        {w.lakekeeperWarehouseId ? "Provisioned" : "Pending"}
                      </Badge>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
      </CardContent>
    </Card>
  );
}

/**
 * Assigned engines for this workspace (control-plane view). Lists the engines
 * assigned to the workspace with a Remove action, plus an "Assign engine" dialog
 * whose picker offers the platform engines not already assigned. Assign/remove
 * hit the CP workspace-engine endpoints; the backend 400s when the engine has no
 * JWT auth, surfaced here as a toast.
 */
function AssignedEnginesSection({ workspaceId }: { workspaceId: string }) {
  const qc = useQueryClient();
  const [assignOpen, setAssignOpen] = useState(false);

  const assignedQ = useControlPlaneEnginesListWorkspaceEngines(workspaceId, {
    query: { retry: false },
  });
  const assigned = assignedQ.data?.status === 200 ? assignedQ.data.data : [];

  const invalidate = () =>
    qc.invalidateQueries({
      queryKey: getControlPlaneEnginesListWorkspaceEnginesQueryKey(workspaceId),
    });

  const unassignMut = useControlPlaneEnginesUnassignWorkspaceEngine({
    mutation: {
      onSuccess: () => {
        toast.success("Engine removed");
        invalidate();
      },
      onError: (e) => toast.error(String(e)),
    },
  });

  return (
    <Card>
      <CardHeader>
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <CardTitle className="text-lg">Assigned engines</CardTitle>
            <CardDescription>
              Egret-provisioned engines this workspace can query. Only JWT-enabled
              engines can be assigned.
            </CardDescription>
          </div>
          <Button size="sm" onClick={() => setAssignOpen(true)}>
            <Cpu className="mr-1 h-4 w-4" /> Assign engine
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        {assignedQ.isLoading ? (
          <p className="text-sm text-muted-foreground">Loading engines…</p>
        ) : assigned.length === 0 ? (
          <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
            No engines assigned to this workspace yet.
          </p>
        ) : (
          <div className="rounded-md border">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead>Type</TableHead>
                  <TableHead>Status</TableHead>
                  <TableHead className="text-right">Actions</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {assigned.map((e) => (
                  <TableRow key={e.id}>
                    <TableCell>
                      <div className="flex min-w-0 items-center gap-2.5">
                        <Cpu className="h-4 w-4 shrink-0 text-muted-foreground" />
                        <span className="truncate font-mono font-medium">{e.name}</span>
                      </div>
                    </TableCell>
                    <TableCell>
                      <Badge variant="secondary">{e.type}</Badge>
                    </TableCell>
                    <TableCell>
                      <Badge variant="outline">{e.status}</Badge>
                    </TableCell>
                    <TableCell className="text-right">
                      <Button
                        size="sm"
                        variant="ghost"
                        className="text-muted-foreground hover:text-destructive"
                        disabled={unassignMut.isPending}
                        onClick={() => unassignMut.mutate({ id: workspaceId, engineId: e.id })}
                      >
                        <Trash2 className="mr-1 h-4 w-4" /> Remove
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
      </CardContent>

      <AssignEngineDialog
        open={assignOpen}
        onOpenChange={setAssignOpen}
        workspaceId={workspaceId}
        assignedIds={assigned.map((e) => e.id)}
        onDone={invalidate}
      />
    </Card>
  );
}

/** An engine is assignable only when its Trino spec enables JWT auth (mirrors the
 * backend guard on `spec.trinoConfig.auth.jwt`). */
function engineHasJwt(e: EngineDto): boolean {
  const auth = (e.spec.trinoConfig as Record<string, unknown> | undefined)?.["auth"] as
    | Record<string, unknown>
    | undefined;
  return typeof auth?.["jwt"] === "object" && auth?.["jwt"] !== null;
}

function AssignEngineDialog({
  open,
  onOpenChange,
  workspaceId,
  assignedIds,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  workspaceId: string;
  assignedIds: string[];
  onDone: () => void;
}) {
  const [engineId, setEngineId] = useState("");
  const enginesQ = useControlPlaneEnginesListEngines({ query: { retry: false } });
  const allEngines = enginesQ.data?.status === 200 ? enginesQ.data.data : [];
  // Only offer JWT-enabled engines — the backend rejects the rest with a 400, so
  // showing them here would just lead to a guaranteed error.
  const unassigned = allEngines.filter((e) => !assignedIds.includes(e.id));
  const candidates = unassigned.filter(engineHasJwt);

  useEffect(() => {
    if (open) setEngineId("");
  }, [open]);

  const assignMut = useControlPlaneEnginesAssignWorkspaceEngine({
    mutation: {
      onSuccess: () => {
        toast.success("Engine assigned");
        onDone();
        onOpenChange(false);
      },
      onError: (e) => toast.error(String(e)),
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Assign engine</DialogTitle>
          <DialogDescription>
            Assign an egret-provisioned engine to this workspace. Only JWT-enabled
            engines can be assigned.
          </DialogDescription>
        </DialogHeader>
        <div className="my-4 space-y-1.5">
          <Label htmlFor="assign-engine">Engine</Label>
          <Select value={engineId} onValueChange={setEngineId}>
            <SelectTrigger id="assign-engine" className="w-full">
              <SelectValue placeholder="Select an engine…" />
            </SelectTrigger>
            <SelectContent>
              {candidates.map((e) => (
                <SelectItem key={e.id} value={e.id}>
                  {e.name} ({e.type})
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {candidates.length === 0 && (
            <p className="text-xs text-muted-foreground">
              {unassigned.length === 0
                ? "All engines are already assigned."
                : "No JWT-enabled engines available — enable JWT auth on an engine first."}
            </p>
          )}
        </div>
        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            type="button"
            disabled={!engineId || assignMut.isPending}
            onClick={() => assignMut.mutate({ id: workspaceId, data: { engineId } })}
          >
            Assign
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function RequestsTab({ slug }: { slug: string }) {
  const requestsQ = useWorkspaceRequests(slug);
  const approveMut = useApproveRequest(slug);
  const denyMut = useDenyRequest(slug);
  const requests = requestsQ.data ?? [];

  if (requestsQ.isLoading) {
    return <p className="text-sm text-muted-foreground">Loading requests…</p>;
  }
  if (requests.length === 0) {
    return (
      <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
        No pending requests.
      </p>
    );
  }

  return (
    <div className="space-y-2">
      {requests.map((r) => (
        <div
          key={r.id}
          className="flex flex-wrap items-center justify-between gap-3 rounded-md border p-3"
        >
          <div className="min-w-0">
            <p className="text-sm font-medium">{r.requester_name ?? r.requester_id}</p>
            <p className="text-xs text-muted-foreground">
              Requested <span className="font-medium">{r.requested_role}</span>
              {r.reason ? ` — ${r.reason}` : ""}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={denyMut.isPending}
              onClick={() =>
                denyMut.mutate(
                  { id: r.id },
                  {
                    onSuccess: () => toast.success("Request denied"),
                    onError: (e) => toast.error(String(e)),
                  },
                )
              }
            >
              Deny
            </Button>
            <Button
              size="sm"
              disabled={approveMut.isPending}
              onClick={() =>
                approveMut.mutate(
                  { id: r.id, role: r.requested_role },
                  {
                    onSuccess: () => toast.success("Request approved"),
                    onError: (e) => toast.error(String(e)),
                  },
                )
              }
            >
              Approve
            </Button>
          </div>
        </div>
      ))}
    </div>
  );
}

function ActivityTab({ workspaceId }: { workspaceId: string }) {
  const auditQ = useControlPlaneAuditListAudit(
    { workspace: workspaceId, limit: 50 },
    { query: { retry: false } },
  );
  const page = auditQ.data?.status === 200 ? auditQ.data.data : undefined;
  const entries = page?.entries ?? [];

  if (auditQ.isLoading) {
    return <p className="text-sm text-muted-foreground">Loading activity…</p>;
  }
  if (entries.length === 0) {
    return (
      <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
        No activity recorded for this workspace yet.
      </p>
    );
  }

  return (
    <div className="divide-y rounded-md border">
      {entries.map((e) => (
        <div key={e.id} className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2 text-sm">
          <span className="text-xs text-muted-foreground" title={fullTimestamp(e.ts)}>
            {timeAgo(e.ts)}
          </span>
          <span className="font-medium">{e.action}</span>
          <span className="text-muted-foreground">
            {e.resourceType}
            {e.resourceId ? ` (${e.resourceId})` : ""}
          </span>
          {e.actorUserId && (
            <span className="text-xs text-muted-foreground" title={e.actorUserId}>
              by {e.actorUserId.slice(0, 8)}
            </span>
          )}
          {e.decision && (
            <Badge variant={e.decision === "allow" ? "outline" : "secondary"}>{e.decision}</Badge>
          )}
        </div>
      ))}
    </div>
  );
}

function RenameDialog({
  open,
  onOpenChange,
  id,
  slug,
  currentName,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  id: string;
  slug: string;
  currentName: string;
  onDone: () => void;
}) {
  const [name, setName] = useState(currentName);
  const updateMut = useAdminWorkspacesUpdateWs();

  useEffect(() => {
    if (open) setName(currentName);
  }, [open, currentName]);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    try {
      await updateMut.mutateAsync({ id, data: { name: name.trim() } });
      toast.success("Workspace renamed");
      onDone();
      onOpenChange(false);
    } catch (err) {
      toast.error(String(err));
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Rename workspace</DialogTitle>
            <DialogDescription>
              Update the display name for <span className="font-medium">{slug}</span>.
            </DialogDescription>
          </DialogHeader>
          <div className="my-4 space-y-1.5">
            <Label htmlFor="detail-rename-name">Name</Label>
            <Input
              id="detail-rename-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
              autoFocus
            />
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={updateMut.isPending || name.trim().length === 0}>
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function TransferOwnerDialog({
  open,
  onOpenChange,
  id,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  id: string;
  onDone: () => void;
}) {
  const usersQ = useQuery({ queryKey: ["control-plane", "access", "users"], queryFn: listUsers, retry: false });
  const [newOwnerId, setNewOwnerId] = useState("");
  const transferMut = useAdminWorkspacesTransferOwner();

  useEffect(() => {
    if (open) setNewOwnerId("");
  }, [open]);

  const users = useMemo(() => usersQ.data ?? [], [usersQ.data]);

  async function handleSubmit() {
    if (!newOwnerId) return;
    try {
      await transferMut.mutateAsync({ id, data: { new_owner_id: newOwnerId } });
      toast.success("Ownership transferred");
      onDone();
      onOpenChange(false);
    } catch (err) {
      toast.error(String(err));
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Transfer ownership</DialogTitle>
          <DialogDescription>
            Reassign this workspace to another user. They will be added as a workspace admin if they
            aren&apos;t already a member.
          </DialogDescription>
        </DialogHeader>
        <div className="my-4 space-y-1.5">
          <Label htmlFor="transfer-owner">New owner</Label>
          <Select value={newOwnerId} onValueChange={setNewOwnerId}>
            <SelectTrigger id="transfer-owner" className="w-full">
              <SelectValue placeholder="Select a user…" />
            </SelectTrigger>
            <SelectContent>
              {users.map((u) => (
                <SelectItem key={u.id} value={u.id}>
                  {u.email}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" onClick={handleSubmit} disabled={!newOwnerId || transferMut.isPending}>
            Transfer
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
