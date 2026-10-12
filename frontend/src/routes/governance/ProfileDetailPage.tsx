import { useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, ArrowRightLeft, Cog, Download, Loader2, Lock, Plus, ShieldCheck, Sprout, Trash2 } from "lucide-react";
import {
  type Profile,
  type PolicySummary,
  isCompilerOwnedPolicyName,
  listProfiles,
  listPolicies,
  deletePolicy,
  setPolicyEnabled,
  seedDefaultPolicies,
} from "@/api/policies";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { TrinoConfigDialog } from "@/components/policy/TrinoConfigDialog";
import { OverlapReport } from "./OverlapReport";
import { PolicyWarningsCallout } from "./PolicyWarningsCallout";
import { refusalMessage, usePolicyPermissions } from "./policyWrites";

/** Why an infra row offers no edit, toggle or delete — the server refuses all
 *  three for `managed_by = 'infra'` (SP4d-3). */
const INFRA_OWNED_REASON =
  "Managed by egret (engine delegation, admin access or query limits) — it cannot be changed here.";

const TYPES = [
  { key: "access", label: "Access" },
  { key: "masking", label: "Masking" },
  { key: "rowfilter", label: "Row-filter" },
] as const;

type PolicyType = (typeof TYPES)[number]["key"];

/**
 * Governance → Policies → a profile (drill-down L2).
 *
 * A card-less identity header (name, type, policy version, engine bindings)
 * with the seed / config actions, then one searchable, paginated policy table
 * per Ranger policy type (access / masking / row-filter) — matching the other
 * control-plane detail pages.
 */
export default function ProfileDetailPage() {
  const { service = "" } = useParams();
  const [configOpen, setConfigOpen] = useState(false);
  const queryClient = useQueryClient();
  const can = usePolicyPermissions();

  // Active policy-type tab lives in the URL (`?tab=`) so the view is
  // deep-linkable and survives a refresh. Default preserved: "access".
  const [sp, setSp] = useSearchParams();
  const tab = sp.get("tab") ?? "access";
  const setTab = (v: string) =>
    setSp(
      (prev) => {
        const n = new URLSearchParams(prev);
        n.set("tab", v);
        return n;
      },
      { replace: true },
    );

  const profiles = useQuery<Profile[], ApiError>({
    queryKey: ["control-plane", "profiles"],
    queryFn: listProfiles,
    retry: false,
  });
  const profile = profiles.data?.find((p) => p.name === service);
  // The tag service is its OWN `authz_services` row, and `list_profiles`
  // returns every service (that is how it resolves `tag_service` at all), so
  // its `origin` needs no extra request (review M-5). It is the right record
  // for the Tags tab's guard: a compiled TAG policy is stored under the tag
  // service's `service_id`, so `refuse_if_compiler_owned` reads THAT service's
  // origin — the main profile's would be marking rows against the wrong row.
  const tagProfile = profiles.data?.find((p) => p.name === profile?.tag_service);

  const seedDefaults = useMutation({
    mutationFn: () => seedDefaultPolicies(service),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] }),
  });

  return (
    <div className="space-y-5">
      <Link
        to="/governance/policies"
        className="inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
      >
        <ArrowLeft className="h-4 w-4" />
        Policies
      </Link>

      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex min-w-0 items-center gap-3">
          <span className="grid h-11 w-11 shrink-0 place-items-center rounded-lg bg-primary/10 text-primary">
            <ShieldCheck className="h-6 w-6" />
          </span>
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <h1 className="truncate text-2xl font-semibold leading-tight">
                {service || "Profile"}
              </h1>
              {profiles.isLoading && !profile ? (
                <Skeleton className="h-5 w-16" />
              ) : (
                <Badge variant="secondary">{profile?.type ?? "trino"}</Badge>
              )}
              {/* D10: `origin` has carried "generated" since SP4a with a
                  comment saying the frontend must show it — before an edit
                  overwrites something, not after. The guard underneath is on
                  the POLICY, not this badge: a foreign (non perm-grant-/
                  perm-rule-) policy in this profile stays fully editable
                  below — see the per-row treatment in PolicyTypeTable. */}
              {profile?.origin === "generated" && (
                <Badge
                  variant="outline"
                  className="gap-1"
                  title="Compiled by the permission model from grants and rules. Its perm-grant-/perm-rule- policies are protected below; everything else stays editable."
                >
                  <Cog className="h-3 w-3" />
                  Generated
                </Badge>
              )}
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-muted-foreground">
              {profile && <span>policy version {profile.policy_version}</span>}
              <span className="flex flex-wrap items-center gap-1.5">
                Engines:
                {profiles.isLoading && !profile ? (
                  <Skeleton className="h-5 w-24" />
                ) : profile && profile.engines.length > 0 ? (
                  profile.engines.map((e) => (
                    <Badge key={e.id} variant="outline" className="font-normal">
                      {e.name}
                    </Badge>
                  ))
                ) : (
                  <span>no engine</span>
                )}
              </span>
            </div>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {/* SP4b-2 finding 3: a write this account cannot make is not offered.
              Each control below names the permission ITS OWN route requires —
              they are four different keys (`policy:seed`, `policy:create`,
              `policy:update`, `policy:delete`), so one blanket "can write"
              flag would hide a control the account can in fact use. */}
          {/* The seeded set is Trino-shaped and the server refuses any other
              profile type (SP4d-3 final review I-1), so it is offered only on a
              loaded Trino profile — not while the profile is still loading. */}
          {can("policy:seed") && profile?.type === "trino" && (
            <Button
              size="sm"
              variant="secondary"
              disabled={seedDefaults.isPending || !service}
              onClick={() => seedDefaults.mutate()}
            >
              {seedDefaults.isPending ? (
                <Loader2 className="mr-1 h-4 w-4 animate-spin" />
              ) : (
                <Sprout className="mr-1 h-4 w-4" />
              )}
              Seed default policies
            </Button>
          )}
          {/* SP4d-3: a read-only page too (it says what each policy is), so
              offered on `policy:read`; its write controls check
              `policy:update` themselves. A link, per FE-PAGE-3. */}
          {service && (
            <Button size="sm" variant="outline" asChild>
              <Link to={`/governance/policies/${encodeURIComponent(service)}/migrate`}>
                <ArrowRightLeft className="mr-1 h-4 w-4" />
                Migrate policies
              </Link>
            </Button>
          )}
          {/* The downloadable config is the Trino coordinator's Ranger-plugin
              config — only meaningful for Trino profiles, and not offered
              while the profile is still loading. */}
          {profile?.type === "trino" && (
            <Button size="sm" variant="outline" onClick={() => setConfigOpen(true)}>
              <Download className="mr-1 h-4 w-4" />
              Download Trino config
            </Button>
          )}
        </div>
      </div>

      {profiles.isError && (
        <Alert variant="destructive">
          <AlertDescription>
            {profiles.error?.message ?? "Failed to load profile."}
          </AlertDescription>
        </Alert>
      )}

      {seedDefaults.isError && (
        <Alert variant="destructive">
          <AlertDescription>
            {(seedDefaults.error as ApiError)?.message ?? "Failed to seed default policies."}
          </AlertDescription>
        </Alert>
      )}

      {seedDefaults.isSuccess && (
        <Alert>
          <AlertDescription>
            Seeded {seedDefaults.data.created} default{" "}
            {seedDefaults.data.created === 1 ? "policy" : "policies"}.
          </AlertDescription>
        </Alert>
      )}

      <PolicyWarningsCallout service={service} serviceId={profile?.id} />

      <OverlapReport serviceName={service} />

      <Tabs value={tab} onValueChange={setTab} className="space-y-5">
        <TabsList>
          {TYPES.map((t) => (
            <TabsTrigger key={t.key} value={t.key}>
              {t.label}
            </TabsTrigger>
          ))}
          {profile?.tag_service && <TabsTrigger value="tags">Tags</TabsTrigger>}
        </TabsList>

        {TYPES.map((t) => (
          <TabsContent key={t.key} value={t.key}>
            <PolicyTypeTable
              service={service}
              type={t.key}
              label={t.label}
              profileOrigin={profile?.origin}
              can={can}
            />
          </TabsContent>
        ))}
        {profile?.tag_service && (
          <TabsContent value="tags">
            {/* The TAG service's own origin, not the main profile's — see
                `tagProfile` above. Marking these rows against the main
                profile's origin would be checking the wrong record; leaving
                them unmarked (as this did before review M-5) offered a
                compiler-owned tag policy for edit and let the operator
                discover the 409 by hitting it. */}
            <PolicyTypeTable
              service={service}
              listFrom={profile.tag_service}
              label="Tag"
              showType
              newHref={`/governance/policies/${encodeURIComponent(service)}/access/new?scope=tag`}
              profileOrigin={tagProfile?.origin}
              can={can}
            />
          </TabsContent>
        )}
      </Tabs>

      <TrinoConfigDialog
        open={configOpen}
        service={service}
        onOpenChange={setConfigOpen}
      />
    </div>
  );
}

/** Ranger policy table for one policy type (or, for the Tags tab, all types of
 *  a separate tag service), scoped to this profile. */
function PolicyTypeTable({
  service,
  type,
  label,
  listFrom,
  newHref,
  showType = false,
  profileOrigin,
  can,
}: {
  /** The profile whose editor create/edit links point at (the main profile). */
  service: string;
  /** Policy type; omit to list ALL types (the Tags tab mixes them). */
  type?: PolicyType;
  label: string;
  /** Service to LIST policies from — defaults to `service`. The Tags tab lists
   *  the separate `<name>_tag` service while still editing via the main profile
   *  (so the "By tag" editor round-trips correctly). */
  listFrom?: string;
  /** Override the "New policy" target (tag policies are created via the main
   *  profile's By-tag editor). */
  newHref?: string;
  /** Show a Type column (the Tags tab mixes access/masking/row-filter). */
  showType?: boolean;
  /**
   * `service`'s own `ProfileDto.origin` — used ONLY to decide whether a
   * ROW's own name is compiler-owned (D10). Deliberately not a profile-wide
   * switch: the backend's guard (`refuse_if_compiler_owned`) fires on a
   * SPECIFIC policy whose name carries the `perm-grant-`/`perm-rule-`
   * prefix, and SP4a moved it there precisely because gating on the profile
   * alone made a foreign policy inside a generated profile permanently
   * uneditable — a one-way door. Whichever service a table LISTS FROM is the
   * one whose origin belongs here: the Tags tab passes its tag service's own
   * `origin`, never the main profile's (review M-5).
   */
  profileOrigin?: string;
  /** Whether this account holds a given `policy:*` permission — see
   *  `usePolicyPermissions`. */
  can: (permission: string) => boolean;
}) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const from = listFrom ?? service;

  const policies = useQuery<PolicySummary[], ApiError>({
    queryKey: ["control-plane", "policies", from, type ?? "all"],
    queryFn: () => listPolicies(from, type),
    enabled: !!from,
    retry: false,
  });
  const rows = policies.data ?? [];

  const [deleteTarget, setDeleteTarget] = useState<PolicySummary | null>(null);
  const [togglingId, setTogglingId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const del = useMutation({
    mutationFn: (id: number) => deletePolicy(id),
    onSuccess: () => {
      setError(null);
      queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] });
    },
    onError: (e: unknown) =>
      setError(refusalMessage(e, "policy:delete", "Failed to delete policy.")),
  });

  // D10: which ROWS are compiler-owned — the policy's OWN name, not this
  // table's profile as a whole. See `profileOrigin`'s own doc comment for
  // why the guard cannot be profile-wide.
  const compilerOwned = (r: PolicySummary) =>
    profileOrigin === "generated" && isCompilerOwnedPolicyName(r.name);
  // SP4d-3: egret's own plumbing (delegation, admin access, query limits). The
  // server refuses every edit, toggle and delete of such a row, keyed on the
  // row's `managed_by` column — so this reads the same column.
  const infraOwned = (r: PolicySummary) => r.managed_by === "infra";

  const toggleEnabled = useMutation({
    mutationFn: ({ id, enabled }: { id: number; enabled: boolean }) =>
      setPolicyEnabled(id, enabled),
    onMutate: ({ id }) => setTogglingId(id),
    onSettled: () => setTogglingId(null),
    onSuccess: () => {
      setError(null);
      queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] });
    },
    onError: (e: unknown) => {
      setError(refusalMessage(e, "policy:update", "Failed to update policy."));
      // Refetch so the Switch snaps back to the real server state.
      queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] });
    },
  });

  const goToNew = () =>
    navigate(
      newHref ??
        `/governance/policies/${encodeURIComponent(service)}/${type ?? "access"}/new`,
    );
  // Always edit through the main profile (`service`) with the row's own type, so
  // a tag policy loads in the By-tag editor that created it.
  const editPath = (id: number, rowType: string) =>
    `/governance/policies/${encodeURIComponent(service)}/${rowType}/${id}`;

  // The policy name links to the editor, so there's no separate Edit action —
  // Delete is the only per-row button (mirrors the other list pages).
  const columns: Column<PolicySummary>[] = [
    {
      key: "name",
      label: "Name",
      sortable: true,
      value: (r) => r.name,
      cell: (r) =>
        compilerOwned(r) || infraOwned(r) ? (
          // D10: a compiler-owned row in a generated profile — not a Link,
          // so it is not "offered for edit" the way a foreign policy still
          // is (that distinction is the whole point; see `compilerOwned`).
          // An infra row is likewise not editable here at all.
          <div className="min-w-0">
            <span
              className="inline-flex items-center gap-1 font-medium text-muted-foreground"
              title={
                infraOwned(r)
                  ? INFRA_OWNED_REASON
                  : "Compiled by the permission model — edit the underlying grant or rule instead."
              }
            >
              <Lock className="h-3 w-3 shrink-0" />
              {r.name}
            </span>
            {r.description && (
              <div className="truncate text-xs text-muted-foreground">{r.description}</div>
            )}
          </div>
        ) : (
          <div className="min-w-0">
            <Link
              to={editPath(r.id, type ?? r.policy_type)}
              className="font-medium text-primary hover:underline"
            >
              {r.name}
            </Link>
            {r.description && (
              <div className="truncate text-xs text-muted-foreground">{r.description}</div>
            )}
          </div>
        ),
    },
    ...(showType
      ? [
          {
            key: "ptype",
            label: "Type",
            sortable: true,
            value: (r: PolicySummary) => r.policy_type,
            cell: (r: PolicySummary) => (
              <Badge variant="outline" className="font-normal capitalize">
                {r.policy_type}
              </Badge>
            ),
          } as Column<PolicySummary>,
        ]
      : []),
    {
      key: "resource",
      label: "Resource",
      sortable: true,
      value: (r) => r.resource_path,
      cell: (r) => <span className="font-mono text-xs">{r.resource_path}</span>,
    },
    {
      key: "principals",
      label: "Principals",
      value: (r) => r.principals.join(", "),
      cell: (r) => (
        <div className="max-w-[180px] truncate" title={r.principals.join(", ")}>
          {r.principals.join(", ") || <span className="text-muted-foreground">—</span>}
        </div>
      ),
    },
    {
      key: "permissions",
      label: "Permissions",
      value: (r) => r.permissions.join(", "),
      cell: (r) => (
        <div className="max-w-[280px] truncate" title={r.permissions.join(", ")}>
          {r.permissions.join(", ") || <span className="text-muted-foreground">—</span>}
        </div>
      ),
    },
    {
      key: "labels",
      label: "Labels",
      cell: (r) =>
        r.labels && r.labels.length > 0 ? (
          <div className="flex flex-wrap gap-1">
            {r.labels.map((l) => (
              <Badge key={l} variant="secondary">
                {l}
              </Badge>
            ))}
          </div>
        ) : (
          <span className="text-muted-foreground">—</span>
        ),
    },
    {
      key: "enabled",
      label: "Enabled",
      sortable: true,
      value: (r) => (r.is_enabled ? 1 : 0),
      cell: (r) => (
        <Switch
          aria-label={`toggle ${r.name}`}
          checked={r.is_enabled}
          disabled={
            (toggleEnabled.isPending && togglingId === r.id) ||
            compilerOwned(r) ||
            infraOwned(r) ||
            !can("policy:update")
          }
          title={
            infraOwned(r)
              ? INFRA_OWNED_REASON
              : compilerOwned(r)
              ? "Compiler-owned — edit the underlying grant or rule instead."
              : !can("policy:update")
                ? "Enabling or disabling a policy needs the policy:update permission."
                : undefined
          }
          onCheckedChange={(c) => toggleEnabled.mutate({ id: r.id, enabled: c })}
        />
      ),
    },
    {
      key: "actions",
      label: "",
      className: "text-right",
      cell: (r) =>
        can("policy:delete") ? (
          <Button
            variant="ghost"
            size="icon"
            disabled={compilerOwned(r) || infraOwned(r)}
            title={
              infraOwned(r)
                ? INFRA_OWNED_REASON
                : compilerOwned(r)
                ? "Compiler-owned — delete the underlying grant or rule instead."
                : "Delete policy"
            }
            aria-label={`Delete ${r.name}`}
            onClick={() => setDeleteTarget(r)}
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        ) : null,
    },
  ];

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div>
          <h2 className="text-base font-semibold">{label} policies</h2>
          <p className="text-sm text-muted-foreground">
            Ranger {label.toLowerCase()} policies enforced for this profile.
          </p>
        </div>
        {can("policy:create") && (
          <Button size="sm" onClick={goToNew}>
            <Plus className="mr-1 h-4 w-4" />
            New policy
          </Button>
        )}
      </div>

      {error && (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      {policies.isLoading ? (
        <ListSkeleton rows={4} cols={7} />
      ) : (
        <FilterableTable
          rows={rows}
          columns={columns}
          getId={(r) => String(r.id)}
          searchPlaceholder="Search policies…"
          initialSort={{ key: "name", dir: "asc" }}
          pageSize={25}
          bordered
          emptyText={
            policies.isError
              ? (policies.error?.message ?? "Failed to load policies.")
              : `No ${label.toLowerCase()} policies for this profile.`
          }
        />
      )}

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete "${deleteTarget.name}"?` : "Delete policy?"}
        description="This policy will be permanently removed from Ranger."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) del.mutate(deleteTarget.id);
          setDeleteTarget(null);
        }}
      />
    </div>
  );
}
