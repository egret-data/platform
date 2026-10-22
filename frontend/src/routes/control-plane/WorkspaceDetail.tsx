import { useEffect, useMemo, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArchiveRestore,
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
import { WorkspaceFeaturesTab } from "@/components/control-plane/WorkspaceFeaturesTab";
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
import { PageContainer } from "@/components/layout/PageContainer";
import { BackLink, DetailHeader } from "@/components/layout/DetailHeader";
import { StatusBadge } from "@/components/ui/status-badge";

const TABS = ["overview", "members", "resources", "features", "requests", "activity"] as const;
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
      <PageContainer className="space-y-4">
        <BackLink to="/control-plane/workspaces">Workspaces</BackLink>
        <p className="text-sm text-destructive" role="alert">
          Workspace not found.
        </p>
      </PageContainer>
    );
  }

  const isArchived = ws.status === "archived";

  return (
    <PageContainer className="space-y-5">
      <DetailHeader
        className=""
        back={{ to: "/control-plane/workspaces", label: "Workspaces" }}
        title={ws.name}
        status={
          <>
            <code className="rounded bg-muted px-2 py-0.5 font-mono text-sm text-muted-foreground">
              {ws.slug}
            </code>
            {isArchived ? (
              <StatusBadge tone="inactive">archived</StatusBadge>
            ) : (
              <StatusBadge tone="success">active</StatusBadge>
            )}
          </>
        }
        actions={
          <>
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
          </>
        }
      />

      <Tabs value={tab} onValueChange={setTab}>
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="members">Members</TabsTrigger>
          <TabsTrigger value="resources">Resources</TabsTrigger>
          <TabsTrigger value="features">Features</TabsTrigger>
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

        <TabsContent value="features" className="mt-4">
          <WorkspaceFeaturesTab workspaceId={id} />
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
    </PageContainer>
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
      <dl className="grid grid-cols-1 gap-4 rounded-md border p-4 sm:grid-cols-3">
        <div className="space-y-1">
          <dt className="text-xs uppercase tracking-wide text-muted-foreground">Owner</dt>
          <dd className="text-sm">{owner ?? "—"}</dd>
        </div>
        <div className="space-y-1">
          <dt className="text-xs uppercase tracking-wide text-muted-foreground">Created</dt>
          <dd className="text-sm" title={fullTimestamp(createdAt)}>
            {timeAgo(createdAt)}
          </dd>
        </div>
        <div className="space-y-1">
          <dt className="text-xs uppercase tracking-wide text-muted-foreground">Status</dt>
          <dd className="text-sm capitalize">{status}</dd>
        </div>
      </dl>
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

/** Iceberg warehouses owned by this workspace (read-only list; management lives
 * on the warehouse's catalog page, Control Plane → Catalogs). */
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
    <section className="space-y-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h2 className="text-base font-semibold">Warehouses</h2>
          <p className="text-sm text-muted-foreground">Iceberg catalogs owned by this workspace.</p>
        </div>
        <Button asChild size="sm" variant="outline">
          {/* The Warehouses LIST is gone (SP3c-2 Task 8); Catalogs supports
              the same `?workspace=` filter server-side (`Catalogs.tsx`'s own
              `searchParams.get("workspace")`). */}
          <Link to={`/control-plane/catalogs?workspace=${workspaceId}`}>Manage</Link>
        </Button>
      </div>
      {whQ.isLoading ? (
        <p className="text-sm text-muted-foreground">Loading warehouses…</p>
      ) : warehouses.length === 0 ? (
        <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
          No warehouses in this workspace yet.
        </p>
      ) : (
        <div className="overflow-x-auto rounded-md border">
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
                      {/* Still a warehouse id, deliberately: this card's rows
                          are `Warehouse`s, and `/control-plane/warehouses/:id`
                          now redirects to the matching catalog (SP3c §3). The
                          redirect exists precisely so every remaining
                          warehouse-id link keeps working without rewiring the
                          card's query onto the registry list. */}
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
    </section>
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
    <section className="space-y-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h2 className="text-base font-semibold">Assigned engines</h2>
          <p className="text-sm text-muted-foreground">
            Egret-provisioned engines this workspace can query. Only JWT-enabled
            engines can be assigned.
          </p>
        </div>
        <Button size="sm" onClick={() => setAssignOpen(true)}>
          <Cpu className="mr-1 h-4 w-4" /> Assign engine
        </Button>
      </div>
      {assignedQ.isLoading ? (
        <p className="text-sm text-muted-foreground">Loading engines…</p>
      ) : assigned.length === 0 ? (
        <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
          No engines assigned to this workspace yet.
        </p>
      ) : (
        <div className="overflow-x-auto rounded-md border">
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

      <AssignEngineDialog
        open={assignOpen}
        onOpenChange={setAssignOpen}
        workspaceId={workspaceId}
        assignedIds={assigned.map((e) => e.id)}
        onDone={invalidate}
      />
    </section>
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

  // Resolve actor UUIDs to emails, matching the Audit log page (shared cache key).
  const usersQ = useQuery({
    queryKey: ["control-plane", "access", "users"],
    queryFn: listUsers,
    retry: false,
  });
  const emailById = useMemo(
    () => new Map((usersQ.data ?? []).map((u) => [u.id, u.email])),
    [usersQ.data],
  );

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
    <div className="overflow-x-auto rounded-md border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Time</TableHead>
            <TableHead>Action</TableHead>
            <TableHead>Resource</TableHead>
            <TableHead>Actor</TableHead>
            <TableHead>Decision</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {entries.map((e) => {
            const actorEmail = e.actorUserId ? emailById.get(e.actorUserId) : undefined;
            return (
              <TableRow key={e.id}>
                <TableCell
                  className="whitespace-nowrap tabular-nums text-muted-foreground"
                  title={fullTimestamp(e.ts)}
                >
                  {timeAgo(e.ts)}
                </TableCell>
                <TableCell className="font-mono text-xs font-medium">{e.action}</TableCell>
                <TableCell className="text-muted-foreground">
                  {e.resourceType}
                  {/* The resourceId is often this workspace's own UUID; truncate it
                      to keep the column quiet on a page already scoped to it. */}
                  {e.resourceId ? (
                    <span className="font-mono text-xs text-muted-foreground/70">
                      {" "}
                      ({e.resourceId.slice(0, 8)}…)
                    </span>
                  ) : null}
                </TableCell>
                <TableCell>
                  {e.actorUserId ? (
                    <span title={e.actorUserId}>
                      {actorEmail ?? (
                        <span className="font-mono text-xs">{e.actorUserId.slice(0, 8)}</span>
                      )}
                    </span>
                  ) : (
                    <span className="text-muted-foreground">system</span>
                  )}
                </TableCell>
                <TableCell>
                  {e.decision === "allow" ? (
                    <StatusBadge tone="success">allow</StatusBadge>
                  ) : e.decision === "deny" ? (
                    <StatusBadge tone="destructive">deny</StatusBadge>
                  ) : (
                    <span className="text-muted-foreground">—</span>
                  )}
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
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
