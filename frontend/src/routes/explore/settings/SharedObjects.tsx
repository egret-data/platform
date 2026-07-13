import { useMemo, useState, type CSSProperties } from "react";
import { useParams } from "react-router-dom";
import { MoreHorizontal } from "lucide-react";

import { useWorkspaceShares, type ObjectGrantRollup } from "@/api/workspaceAccess";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { ShareDialog, type ShareKind } from "@/components/sharing/ShareDialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

/** Narrow the react-query error into an HTTP status if we have one. */
function errorStatus(err: unknown): number | undefined {
  if (err && typeof err === "object" && "status" in err) {
    const s = (err as { status?: unknown }).status;
    if (typeof s === "number") return s;
  }
  return undefined;
}

const allowStyle: CSSProperties = {
  backgroundColor: "color-mix(in oklab, var(--success) 16%, transparent)",
  color: "var(--success)",
  borderColor: "color-mix(in oklab, var(--success) 32%, transparent)",
};
const denyStyle: CSSProperties = {
  backgroundColor: "color-mix(in oklab, var(--destructive) 16%, transparent)",
  color: "var(--destructive)",
  borderColor: "color-mix(in oklab, var(--destructive) 32%, transparent)",
};

/** allow/deny badge tinted with the semantic success/destructive token. */
function EffectBadge({ effect }: { effect: string }) {
  const deny = effect === "deny";
  return (
    <Badge className="border font-normal" style={deny ? denyStyle : allowStyle}>
      {effect}
    </Badge>
  );
}

/** Object identifying a row's share target, used to open ShareDialog. */
interface ShareTarget {
  kind: string;
  id: string;
  name: string | null;
}

/**
 * Workspace admin → Settings → Shared objects.
 *
 * A workspace-wide roll-up of every shared object (dashboard, chart,
 * dataset, folder, notebook, saved query) and who holds each grant on it —
 * one row per (object, principal) pair. Read-only: editing a grant opens the
 * existing per-object `ShareDialog`, which owns the actual mutation.
 */
export default function SharedObjects() {
  const ws = useParams().workspace!;
  const q = useWorkspaceShares(ws);

  const rows = q.data ?? [];
  const forbidden = errorStatus(q.error) === 403;

  const [kindFacet, setKindFacet] = useState(FACET_ALL);
  const [effectFacet, setEffectFacet] = useState(FACET_ALL);
  const [shareTarget, setShareTarget] = useState<ShareTarget | null>(null);

  const kindOptions = useMemo(
    () =>
      Array.from(new Set(rows.map((r) => r.kind)))
        .sort()
        .map((k) => ({ value: k, label: k })),
    [rows],
  );

  const filtered = rows.filter(
    (r) =>
      (kindFacet === FACET_ALL || r.kind === kindFacet) &&
      (effectFacet === FACET_ALL || r.effect === effectFacet),
  );

  const totalShares = rows.length;
  const distinctObjects = new Set(rows.map((r) => r.object_id)).size;
  const denied = rows.filter((r) => r.effect === "deny").length;

  const columns: Column<ObjectGrantRollup>[] = [
    {
      key: "object",
      label: "Object",
      sortable: true,
      value: (r) => r.object_name ?? r.object_id,
      cell: (r) => (
        <span className="flex min-w-0 items-center gap-2">
          <Badge variant="secondary" className="shrink-0 font-normal">
            {r.kind}
          </Badge>
          <span className="truncate text-sm" title={r.object_id}>
            {r.object_name ?? r.object_id}
          </span>
        </span>
      ),
    },
    {
      key: "principal",
      label: "Principal",
      sortable: true,
      value: (r) => r.principal_name ?? r.principal_id,
      cell: (r) => (
        <span className="flex min-w-0 items-center gap-2">
          <span className="truncate text-sm" title={r.principal_id}>
            {r.principal_name ?? r.principal_id}
          </span>
          <Badge variant="outline" className="shrink-0">
            {r.principal_type === "group" ? "Group" : "User"}
          </Badge>
        </span>
      ),
    },
    {
      key: "role",
      label: "Role",
      sortable: true,
      value: (r) => r.role,
      cell: (r) => <Badge variant="outline">{r.role}</Badge>,
    },
    {
      key: "effect",
      label: "Effect",
      sortable: true,
      value: (r) => r.effect,
      cell: (r) => <EffectBadge effect={r.effect} />,
    },
    {
      key: "actions",
      label: "",
      className: "text-right",
      cell: (r) => (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              aria-label={`Actions for ${r.object_name ?? r.object_id}`}
            >
              <MoreHorizontal className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem
              onClick={() =>
                setShareTarget({ kind: r.kind, id: r.object_id, name: r.object_name })
              }
            >
              Manage sharing…
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      ),
    },
  ];

  return (
    <div className="space-y-5">
      <header>
        <h2 className="text-lg font-semibold">Shared objects</h2>
        <p className="text-sm text-muted-foreground">
          Everything shared inside <span className="font-medium">{ws}</span> and who can access
          it.
        </p>
      </header>

      {q.isLoading ? (
        <ListSkeleton rows={6} cols={5} />
      ) : forbidden ? (
        <p
          className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground"
          role="alert"
        >
          Only workspace admins can view shares.
        </p>
      ) : q.isError ? (
        <p className="py-6 text-sm text-destructive" role="alert">
          Couldn&apos;t load shares. {String(q.error)}
        </p>
      ) : rows.length === 0 ? (
        <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
          Nothing shared in this workspace yet.
        </p>
      ) : (
        <div className="space-y-4">
          <SummaryStrip
            items={[
              { label: "shares", value: totalShares },
              { label: "objects", value: distinctObjects },
              { label: "denied", value: denied },
            ]}
          />
          <FilterableTable
            rows={filtered}
            columns={columns}
            getId={(r) => `${r.kind}:${r.object_id}:${r.principal_type}:${r.principal_id}`}
            searchPlaceholder="Search shares…"
            initialSort={{ key: "object", dir: "asc" }}
            emptyText="No shares match the current filters."
            filters={
              <>
                <FacetSelect label="Kind" value={kindFacet} onChange={setKindFacet} options={kindOptions} />
                <FacetSelect
                  label="Effect"
                  value={effectFacet}
                  onChange={setEffectFacet}
                  options={[
                    { value: "allow", label: "Allow" },
                    { value: "deny", label: "Deny" },
                  ]}
                />
              </>
            }
          />
        </div>
      )}

      {shareTarget && (
        <ShareDialog
          ws={ws}
          kind={shareTarget.kind as ShareKind}
          id={shareTarget.id}
          name={shareTarget.name ?? undefined}
          open={shareTarget !== null}
          onOpenChange={(o) => {
            if (!o) setShareTarget(null);
          }}
        />
      )}
    </div>
  );
}
