import { useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  Download,
  Loader2,
  Pencil,
  Plus,
  ShieldCheck,
  Sprout,
  Trash2,
} from "lucide-react";
import {
  type Profile,
  type PolicySummary,
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
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { TrinoConfigDialog } from "@/components/policy/TrinoConfigDialog";

const TYPES = [
  { key: "access", label: "Access" },
  { key: "masking", label: "Masking" },
  { key: "rowfilter", label: "Row-filter" },
] as const;

type PolicyType = (typeof TYPES)[number]["key"];

const COLUMNS = [
  "Name",
  "Resource",
  "Principals",
  "Permissions",
  "Labels",
  "Enabled",
  "",
];

/**
 * Control Plane → Policy profile detail (drill-down L2).
 *
 * Header identifies the profile (name, engine bindings, Ranger policy
 * version) and offers the Trino-config download; the tabs below hold one
 * policy table per Ranger policy type (access / masking / row-filter).
 */
export default function ProfileDetailPage() {
  const { service = "" } = useParams();
  const [configOpen, setConfigOpen] = useState(false);
  const queryClient = useQueryClient();

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

  const seedDefaults = useMutation({
    mutationFn: () => seedDefaultPolicies(service),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] }),
  });

  return (
    <div className="space-y-5">
      <Link
        to="/control-plane/policies"
        className="inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
      >
        <ArrowLeft className="h-4 w-4" />
        Policies
      </Link>

      <Card className="overflow-hidden">
        <div className="flex items-center gap-3 border-b bg-muted/30 p-5">
          <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
            <ShieldCheck className="h-6 w-6" />
          </div>
          <div className="min-w-0 flex-1">
            <h1 className="truncate text-2xl font-semibold leading-tight">
              {service || "Profile"}
            </h1>
            <p className="text-xs text-muted-foreground">
              {profile ? `policy version ${profile.policy_version}` : ""}
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {profiles.isLoading && !profile ? (
              <Skeleton className="h-5 w-16" />
            ) : (
              <Badge variant="secondary">{profile?.type ?? "trino"}</Badge>
            )}
          </div>
        </div>

        <div className="flex flex-wrap items-center justify-between gap-3 p-5">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm text-muted-foreground">Engines:</span>
            {profiles.isLoading && !profile ? (
              <Skeleton className="h-5 w-24" />
            ) : profile && profile.engines.length > 0 ? (
              profile.engines.map((e) => (
                <Badge key={e.id} variant="outline">
                  {e.name}
                </Badge>
              ))
            ) : (
              <span className="text-sm text-muted-foreground">no engine</span>
            )}
          </div>
          <div className="flex items-center gap-2">
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
            <Button size="sm" variant="outline" onClick={() => setConfigOpen(true)}>
              <Download className="mr-1 h-4 w-4" />
              Download Trino config
            </Button>
          </div>
        </div>
      </Card>

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

      <Tabs value={tab} onValueChange={setTab} className="space-y-5">
        <TabsList>
          {TYPES.map((t) => (
            <TabsTrigger key={t.key} value={t.key}>
              {t.label}
            </TabsTrigger>
          ))}
        </TabsList>

        {TYPES.map((t) => (
          <TabsContent key={t.key} value={t.key}>
            <PolicyTypeTable service={service} type={t.key} label={t.label} />
          </TabsContent>
        ))}
      </Tabs>

      <TrinoConfigDialog
        open={configOpen}
        service={service}
        onOpenChange={setConfigOpen}
      />
    </div>
  );
}

/** Ranger policy table for a single policy type, scoped to this profile. */
function PolicyTypeTable({
  service,
  type,
  label,
}: {
  service: string;
  type: PolicyType;
  label: string;
}) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const policies = useQuery<PolicySummary[], ApiError>({
    queryKey: ["control-plane", "policies", service, type],
    queryFn: () => listPolicies(service, type),
    enabled: !!service,
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
      setError((e as ApiError)?.message ?? "Failed to delete policy."),
  });

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
      setError((e as ApiError)?.message ?? "Failed to update policy.");
      // Refetch so the Switch snaps back to the real server state.
      queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] });
    },
  });

  const goToNew = () =>
    navigate(`/control-plane/policies/${encodeURIComponent(service)}/${type}/new`);
  const goToEdit = (id: number) =>
    navigate(`/control-plane/policies/${encodeURIComponent(service)}/${type}/${id}`);
  const editPath = (id: number) =>
    `/control-plane/policies/${encodeURIComponent(service)}/${type}/${id}`;

  return (
    <Card>
      <CardHeader>
        <div className="flex items-center justify-between gap-2">
          <div className="space-y-1.5">
            <CardTitle className="text-lg">{label} policies</CardTitle>
            <CardDescription>
              Ranger {label.toLowerCase()} policies enforced for this profile.
            </CardDescription>
          </div>
          <Button size="sm" onClick={goToNew}>
            <Plus className="mr-1 h-4 w-4" />
            New policy
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-3">
        {error && (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}
        <Table>
          <TableHeader>
            <TableRow>
              {COLUMNS.map((col) => (
                <TableHead
                  key={col || "actions"}
                  className={col === "" ? "text-right" : undefined}
                >
                  {col || <span className="sr-only">Actions</span>}
                </TableHead>
              ))}
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((r) => (
              <TableRow key={r.id}>
                <TableCell className="font-medium">
                  <Link
                    to={editPath(r.id)}
                    className="text-primary hover:underline"
                  >
                    {r.name}
                  </Link>
                  {r.description && (
                    <div className="text-xs font-normal text-muted-foreground">
                      {r.description}
                    </div>
                  )}
                </TableCell>
                <TableCell className="font-mono text-xs">
                  {r.resource_path}
                </TableCell>
                <TableCell>
                  <div
                    className="max-w-[180px] truncate"
                    title={r.principals.join(", ")}
                  >
                    {r.principals.join(", ")}
                  </div>
                </TableCell>
                <TableCell>
                  <div
                    className="max-w-[280px] truncate"
                    title={r.permissions.join(", ")}
                  >
                    {r.permissions.join(", ")}
                  </div>
                </TableCell>
                <TableCell>
                  {r.labels && r.labels.length > 0 ? (
                    <div className="flex flex-wrap gap-1">
                      {r.labels.map((l) => (
                        <Badge key={l} variant="secondary">
                          {l}
                        </Badge>
                      ))}
                    </div>
                  ) : (
                    <span className="text-muted-foreground">—</span>
                  )}
                </TableCell>
                <TableCell>
                  <Switch
                    aria-label={`toggle ${r.name}`}
                    checked={r.is_enabled}
                    disabled={toggleEnabled.isPending && togglingId === r.id}
                    onCheckedChange={(c) =>
                      toggleEnabled.mutate({ id: r.id, enabled: c })
                    }
                  />
                </TableCell>
                <TableCell className="text-right">
                  <div className="flex justify-end gap-1">
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label={`Edit ${r.name}`}
                      onClick={() => goToEdit(r.id)}
                    >
                      <Pencil className="h-4 w-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label={`Delete ${r.name}`}
                      onClick={() => setDeleteTarget(r)}
                    >
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            ))}
            {rows.length === 0 && (
              <TableRow className="hover:bg-transparent">
                <TableCell
                  colSpan={COLUMNS.length}
                  className="text-center text-muted-foreground"
                >
                  {policies.isError ? (
                    <span className="text-destructive">
                      {policies.error?.message ?? "Failed to load policies."}
                    </span>
                  ) : policies.isLoading ? (
                    <ListSkeleton rows={4} cols={COLUMNS.length} />
                  ) : (
                    `No ${label.toLowerCase()} policies for this profile.`
                  )}
                </TableCell>
              </TableRow>
            )}
          </TableBody>
        </Table>
      </CardContent>

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
    </Card>
  );
}
