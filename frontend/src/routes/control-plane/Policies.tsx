import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { Download } from "lucide-react";

import { type Profile, listProfiles } from "@/api/policies";
import { ApiError } from "@/api/client";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { TrinoConfigDialog } from "@/components/policy/TrinoConfigDialog";

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

  const profiles = useQuery<Profile[], ApiError>({
    queryKey: ["control-plane", "profiles"],
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
          <CardTitle className="text-lg">Policy profiles</CardTitle>
          <CardDescription>
            Each engine enforces one profile; tag profiles carry classification-based
            masking &amp; row-filters.
          </CardDescription>
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
                  : "No profiles yet."
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
    </div>
  );
}
