import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, Loader2, Plus, Server } from "lucide-react";

import { type CreatedProfile, type Profile, createProfile, listProfiles } from "@/api/policies";
import { ApiError } from "@/api/client";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
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
import { TrinoConfigDialog } from "@/components/policy/TrinoConfigDialog";

export const profilesQueryKey = ["control-plane", "profiles"] as const;

/**
 * Control Plane → Data governance → Policies (drill-down L1: profiles landing).
 *
 * Lists Ranger policy profiles on the shared list scaffold used across the
 * Control Plane (header → Card → SummaryStrip → FilterableTable + facet). Each
 * row links into the per-profile detail page where access / masking /
 * row-filter policies are managed.
 */
export default function Policies() {
  const [configTarget, setConfigTarget] = useState<string | null>(null);
  const [typeFacet, setTypeFacet] = useState(FACET_ALL);
  const [addOpen, setAddOpen] = useState(false);

  const profiles = useQuery<Profile[], ApiError>({
    queryKey: profilesQueryKey,
    queryFn: listProfiles,
    retry: false,
  });
  const rows = profiles.data ?? [];

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
          to={`/control-plane/policies/${encodeURIComponent(p.name)}`}
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
      cell: (p) => <Badge variant="secondary">{p.type}</Badge>,
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
      value: (p) => p.policy_version,
      cell: (p) => <span className="text-muted-foreground">v{p.policy_version}</span>,
    },
    {
      key: "policies",
      label: "Policies",
      sortable: true,
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
      cell: (p) => (
        <Button
          size="icon"
          variant="ghost"
          title="Download config"
          aria-label={`Download config for ${p.name}`}
          onClick={() => setConfigTarget(p.name)}
        >
          <Download className="h-4 w-4" />
        </Button>
      ),
    },
  ];

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Policies</h1>
        <p className="text-sm text-muted-foreground">
          Ranger policy profiles — each engine enforces one profile. Open a profile to
          manage its access, masking, and row-filter policies.
        </p>
      </div>

      <Card>
        <CardHeader>
          <div className="flex items-start justify-between gap-3">
            <div className="space-y-1.5">
              <CardTitle className="text-lg">Policy profiles</CardTitle>
              <CardDescription>
                Each engine enforces one profile; tag profiles carry classification-based
                masking &amp; row-filters. Provisioning an engine creates its profile
                automatically — use &ldquo;New profile&rdquo; for an engine egret doesn&apos;t
                provision itself (e.g. an existing StarRocks cluster).
              </CardDescription>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <Button variant="outline" size="sm" onClick={() => setAddOpen(true)}>
                <Plus className="mr-1.5 h-4 w-4" />
                New profile
              </Button>
              <Button asChild variant="outline" size="sm">
                <Link to="/control-plane/engines">
                  <Server className="mr-1.5 h-4 w-4" />
                  Provision an engine
                </Link>
              </Button>
            </div>
          </div>
        </CardHeader>
        <CardContent className="space-y-4">
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
        </CardContent>
      </Card>

      {configTarget && (
        <TrinoConfigDialog
          open
          service={configTarget}
          onOpenChange={(o) => !o && setConfigTarget(null)}
        />
      )}

      <AddProfileDialog open={addOpen} onOpenChange={setAddOpen} />
    </div>
  );
}

/**
 * "New profile" dialog — the manual analogue of Trino provisioning's
 * auto-bind, for engines egret doesn't provision itself (e.g. an existing
 * StarRocks cluster). Mirrors `Warehouses.tsx`'s `AddWarehouseDialog`.
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
      setError((err as ApiError).message ?? "Failed to create profile.");
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
