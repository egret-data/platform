import { Link } from "react-router-dom";
import { CircleCheck, TriangleAlert } from "lucide-react";
import type { UnresolvedPrincipal } from "@/api/generated/model";
import { usePermListUnresolvedPrincipals } from "@/api/generated/perm/perm";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { TabError } from "@/routes/governance/data/tabs/TabStates";

const KIND: Record<string, string> = {
  user: "user",
  group: "group",
  role: "role",
  workspace_members: "workspace members",
};

/** `Deny, include` for a grant principal, `Exemption, exclude` for a rule's. */
function positionText(r: UnresolvedPrincipal): string {
  const what = r.source === "rule" ? "Exemption" : r.effect === "deny" ? "Deny" : "Allow";
  return `${what}, ${r.position}`;
}

/**
 * What matching nobody does, in words (FE-A11Y-4). `failsOpen` (the server
 * decides it): the compiled policy allows more than the grant or rule states.
 * An `empty` group is listed but never fails open: nobody is in it, so
 * matching nobody is its meaning.
 */
function Effect({ r }: { r: UnresolvedPrincipal }) {
  if (r.failsOpen) {
    return (
      <span className="inline-flex items-center gap-1 font-medium text-destructive">
        <TriangleAlert className="size-4" aria-hidden />
        Widens access
      </span>
    );
  }
  return <span className="text-muted-foreground">{r.status === "empty" ? "Empty group" : "Narrows access"}</span>;
}

function SourceLink({ r }: { r: UnresolvedPrincipal }) {
  const isRule = r.source === "rule";
  const to = `/governance/${isRule ? "rules" : "grants"}/${encodeURIComponent(r.id)}`;
  const label = isRule ? (r.name ? `Rule ${r.name}` : "Rule") : "Grant";
  return (
    <Link to={to} className="break-all text-primary hover:underline">
      {label}
    </Link>
  );
}

/**
 * The index's Unmatched names tab (spec P11): every stored grant principal
 * and rule exemption whose name matches nobody (`GET
 * /perm/unresolved-principals`), the ones that widen access first, each with
 * the server's reason and a link to its grant or rule. `q` filters on the
 * stored name, here in the page: the route answers the whole list.
 */
export function UnmatchedNames({ q, onClearSearch }: { q: string; onClearSearch: () => void }) {
  const unmatched = usePermListUnresolvedPrincipals();

  if (unmatched.isError) return <TabError what="unmatched names" onRetry={() => void unmatched.refetch()} />;
  if (unmatched.data?.status !== 200) return <ListSkeleton rows={4} cols={4} />;
  const all = unmatched.data.data;
  if (all.length === 0) {
    return <EmptyState icon={CircleCheck} title="Every grant principal and rule exemption names someone egret knows." />;
  }
  const needle = q.toLowerCase();
  // `sort` is stable: within each half the server's order stays.
  const rows = all
    .filter((r) => r.ref.toLowerCase().includes(needle))
    .sort((a, b) => Number(b.failsOpen) - Number(a.failsOpen));
  if (rows.length === 0) {
    return (
      <EmptyState
        title={`No unmatched name matches “${q}”.`}
        action={
          <Button variant="outline" size="sm" onClick={onClearSearch}>
            Clear search
          </Button>
        }
      />
    );
  }

  return (
    <div className="space-y-3">
      <p className="text-sm text-muted-foreground">
        Names stored in grants and rule exemptions that match nobody. One that widens access makes the compiled policy
        allow more than its grant or rule states.
      </p>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Effect</TableHead>
            <TableHead>In</TableHead>
            <TableHead>Position</TableHead>
            <TableHead>Reason</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((r) => (
            <TableRow key={`${r.source}|${r.id}|${r.position}|${r.kind}|${r.ref}`}>
              <TableCell>
                <span className="text-xs text-muted-foreground">{KIND[r.kind] ?? r.kind}</span>{" "}
                <span className="break-all font-mono text-xs">{r.ref}</span>
              </TableCell>
              <TableCell className="whitespace-nowrap">
                <Effect r={r} />
              </TableCell>
              <TableCell>
                <SourceLink r={r} />
              </TableCell>
              <TableCell className="whitespace-nowrap">{positionText(r)}</TableCell>
              <TableCell className="min-w-48">{r.reason}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
