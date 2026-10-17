import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Cog, Download, Loader2, MoreHorizontal, Plus, Server, Trash2 } from "lucide-react";

import {
  type CreatedProfile,
  type Profile,
  createProfile,
  deleteProfile,
  listProfiles,
} from "@/api/policies";
import { ApiError } from "@/api/client";
import { useAuth } from "@/auth/useAuth";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { TrinoConfigDialog } from "@/components/policy/TrinoConfigDialog";
import { OverlapReport } from "./OverlapReport";
import { refusalMessage, usePolicyPermissions } from "./policyWrites";

export const profilesQueryKey = ["control-plane", "profiles"] as const;

/**
 * Governance → Policies (drill-down L1: profiles landing).
 *
 * Lists Ranger policy profiles on the shared card-less list scaffold used across
 * the Control Plane (header → SummaryStrip → bordered FilterableTable + facet).
 * Each row links into the per-profile detail page where access / masking /
 * row-filter policies are managed, and its actions menu can download the engine
 * config or delete the profile (only when no engine still enforces it).
 */
export default function Policies() {
  const { me } = useAuth();
  const canProvisionEngine =
    (me?.platform_roles.includes("platform_admin") ?? false) || (me?.permissions?.includes("engine:create") ?? false);
  // SP4b-2 finding 3, the sibling path the first pass missed: this page offered
  // a `policy:read` steward `New profile` and `Delete profile`, and answered
  // the submitted dialog with the server's bare `forbidden`. Same per-key
  // gating and same refusal wording as the detail page — see `policyWrites`
  // for the control→route→permission table this surface as a whole obeys.
  const can = usePolicyPermissions();
  const queryClient = useQueryClient();
  const [configTarget, setConfigTarget] = useState<string | null>(null);
  const [typeFacet, setTypeFacet] = useState(FACET_ALL);
  const [addOpen, setAddOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Profile | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  const profiles = useQuery<Profile[], ApiError>({
    queryKey: profilesQueryKey,
    queryFn: listProfiles,
    retry: false,
  });
  const rows = profiles.data ?? [];

  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: (name) => deleteProfile(name),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: profilesQueryKey });
      setDeleteError(null);
    },
    onError: (err) =>
      setDeleteError(refusalMessage(err, "policy:delete", "Failed to delete profile.")),
  });

  const filtered = rows.filter((p) => typeFacet === FACET_ALL || p.type === typeFacet);
  const totalPolicies = rows.reduce((n, p) => n + p.policy_count, 0);
  const boundEngines = rows.reduce((n, p) => n + p.engines.length, 0);

  const typeOptions = useMemo(() => {
    const seen = new Set<string>();
    const opts: { value: string; label: string }[] = [];
    for (const p of rows) {
      if (seen.has(p.type)) continue;
      seen.add(p.type);
      opts.push({ value: p.type, label: p.type });
    }
    return opts;
  }, [rows]);

  const columns: Column<Profile>[] = [
    {
      key: "name",
      label: "Name",
      sortable: true,
      className: "font-medium",
      value: (p) => p.name,
      cell: (p) => (
        <Link
          to={`/governance/policies/${encodeURIComponent(p.name)}`}
          className="font-medium text-primary hover:underline"
        >
          {p.name}
        </Link>
      ),
    },
    {
      key: "type",
      label: "Type",
      sortable: true,
      value: (p) => p.type,
      // A profile's type can be a long Ranger service-instance name; cap it so
      // the column stays tidy (clean ellipsis, full value on hover). The inner
      // span carries the truncation because a bare `truncate` on the inline-flex
      // Badge clips mid-string instead of showing an ellipsis.
      cell: (p) => (
        <div className="flex flex-wrap items-center gap-1">
          <Badge variant="secondary" className="max-w-[16rem]" title={p.type}>
            <span className="min-w-0 truncate">{p.type}</span>
          </Badge>
          {/* D10: `origin` has carried "generated" since SP4a with a comment
              saying the frontend must show it — this is that. A generated
              profile's `perm-grant-`/`perm-rule-` policies are compiler-owned
              and refuse a direct edit (409); everything else in it stays
              editable — see ProfileDetailPage, which is where that
              distinction actually matters. */}
          {p.origin === "generated" && (
            <Badge
              variant="outline"
              className="gap-1"
              title="Compiled by the permission model from grants and rules. Its perm-grant-/perm-rule- policies are protected; everything else in this profile stays editable."
            >
              <Cog className="h-3 w-3" />
              Generated
            </Badge>
          )}
        </div>
      ),
    },
    {
      key: "engines",
      label: "Engines",
      value: (p) => p.engines.map((e) => e.name).join(", "),
      cell: (p) =>
        p.engines.length ? (
          p.engines.map((e) => e.name).join(", ")
        ) : (
          <span className="text-muted-foreground">—</span>
        ),
    },
    {
      key: "version",
      label: "Version",
      sortable: true,
      className: "w-24",
      value: (p) => p.policy_version,
      cell: (p) => <span className="text-muted-foreground">v{p.policy_version}</span>,
    },
    {
      key: "policies",
      label: "Policies",
      sortable: true,
      className: "w-24",
      value: (p) => p.policy_count,
      cell: (p) => (
        <Badge
          variant="secondary"
          className={p.policy_count === 0 ? "text-muted-foreground" : undefined}
        >
          {p.policy_count}
        </Badge>
      ),
    },
    {
      key: "actions",
      label: "",
      className: "text-right",
      cell: (p) => {
        const bound = p.engines.length > 0;
        const canDownload = p.type.startsWith("trino");
        const canDelete = can("policy:delete");
        // Every item in this menu is conditional, so the trigger has to be
        // too: a menu that opens on nothing is its own small lie.
        if (!canDownload && !canDelete) return null;
        return (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon" aria-label={`Actions for ${p.name}`}>
                <MoreHorizontal className="h-4 w-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {/* The config download is the Trino coordinator's Ranger-plugin
                  config — only meaningful for Trino profiles. A GET on
                  `policy:read`, so never gated here. */}
              {canDownload && (
                <>
                  <DropdownMenuItem onClick={() => setConfigTarget(p.name)}>
                    <Download className="mr-2 h-4 w-4" />
                    Download config
                  </DropdownMenuItem>
                  {canDelete && <DropdownMenuSeparator />}
                </>
              )}
              {canDelete && (
              <DropdownMenuItem
                disabled={bound}
                title={
                  bound
                    ? "Enforced by an engine — unbind or delete the engine first."
                    : undefined
                }
                className="text-destructive focus:text-destructive"
                onClick={() => {
                  setDeleteError(null);
                  setDeleteTarget(p);
                }}
              >
                <Trash2 className="mr-2 h-4 w-4" />
                Delete profile
              </DropdownMenuItem>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        );
      },
    },
  ];

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold leading-tight">Policies</h1>
          <p className="text-sm text-muted-foreground">
            Ranger policy profiles — each engine enforces one profile. Open a profile to
            manage its access, masking, and row-filter policies.
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {can("policy:create") && (
            <Button
              variant="outline"
              size="sm"
              onClick={() => setAddOpen(true)}
              title="For an engine egret doesn't provision itself (e.g. an existing StarRocks cluster). Provisioning an engine creates its profile automatically."
            >
              <Plus className="mr-1.5 h-4 w-4" />
              New profile
            </Button>
          )}
          {canProvisionEngine && (
            <Button asChild variant="outline" size="sm">
              <Link to="/control-plane/engines">
                <Server className="mr-1.5 h-4 w-4" />
                Provision an engine
              </Link>
            </Button>
          )}
        </div>
      </div>

      {/* Every profile's stored overlapping pairs (SP4d-4), here since Mask &
          filter rules is gone (DG redesign #2, P8). A profile's own page shows
          only the pairs reported under its name, so a pair reported under a
          tag service no profile names is listed here alone. */}
      <OverlapReport />

      <div className="space-y-4">
        {deleteError && (
          <Alert variant="destructive">
            <AlertDescription>{deleteError}</AlertDescription>
          </Alert>
        )}
        <SummaryStrip
          items={[
            { label: "profiles", value: rows.length },
            { label: "policies", value: totalPolicies },
            { label: "bound engines", value: boundEngines },
          ]}
        />
        {profiles.isLoading ? (
          <ListSkeleton rows={5} cols={6} />
        ) : (
          <FilterableTable
            rows={filtered}
            columns={columns}
            getId={(p) => String(p.id)}
            searchPlaceholder="Search profiles…"
            initialSort={{ key: "name", dir: "asc" }}
            pageSize={25}
            bordered
            emptyText={
              profiles.isError
                ? (profiles.error?.message ?? "Failed to load profiles.")
                : "No profiles yet — provision an engine to create one."
            }
            filters={
              typeOptions.length > 1 ? (
                <FacetSelect
                  label="Type"
                  value={typeFacet}
                  onChange={setTypeFacet}
                  options={typeOptions}
                />
              ) : undefined
            }
          />
        )}
      </div>

      {configTarget && (
        <TrinoConfigDialog
          open
          service={configTarget}
          onOpenChange={(o) => !o && setConfigTarget(null)}
        />
      )}

      <AddProfileDialog open={addOpen} onOpenChange={setAddOpen} />

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete "${deleteTarget.name}"?` : "Delete profile?"}
        description="This profile and all of its access, masking, and row-filter policies will be permanently removed from Ranger."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget.name);
          setDeleteTarget(null);
        }}
      />
    </div>
  );
}

/**
 * "New profile" dialog — the manual analogue of Trino provisioning's
 * auto-bind, for engines egret doesn't provision itself (e.g. an existing
 * StarRocks cluster). Mirrors the create dialog pattern used across Control
 * Plane (e.g. `Workspaces.tsx`).
 */
function AddProfileDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const createMutation = useMutation<
    CreatedProfile,
    ApiError,
    { name: string; type: "trino" | "starrocks" }
  >({
    mutationFn: ({ name, type }) => createProfile(name, type),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: profilesQueryKey }),
  });

  const [name, setName] = useState("");
  const [type, setType] = useState<"trino" | "starrocks">("trino");
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setName("");
    setType("trino");
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
      await createMutation.mutateAsync({ name: name.trim(), type });
      handleOpenChange(false);
    } catch (err) {
      // M-1: the create route's OWN refusal, on the dialog the steward has
      // just filled in. `New profile` is hidden without `policy:create`, so
      // this fires for a stale permission snapshot — and must still say what
      // is missing rather than the server's bare `forbidden`.
      setError(refusalMessage(err, "policy:create", "Failed to create profile."));
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>New profile</DialogTitle>
            <DialogDescription>
              Creates a Ranger service instance of the given engine type.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="np-name">Name</Label>
              <Input
                id="np-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. starrocks-prod"
                required
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="np-type">Type</Label>
              <Select value={type} onValueChange={(v) => setType(v as "trino" | "starrocks")}>
                <SelectTrigger id="np-type" className="w-full">
                  <SelectValue placeholder="Select a type" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="trino">trino</SelectItem>
                  <SelectItem value="starrocks">starrocks</SelectItem>
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
            <Button type="submit" disabled={createMutation.isPending || !name.trim() || !type}>
              {createMutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Create profile
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
