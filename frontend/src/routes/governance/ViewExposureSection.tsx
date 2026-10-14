import { AlertTriangle } from "lucide-react";

import { useControlPlaneTagsGetViewExposure } from "@/api/generated/control-plane-tags/control-plane-tags";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

/**
 * Governance → Classifications → a tag: the materialized views that already
 * exist and read a table of this classification (view governance A4, revised
 * 2026-10-14).
 *
 * StarRocks keeps every materialized view in `default_catalog`, so each running
 * engine is asked once and its views are matched by base table, or, for a view
 * with no base-table map, by the text of its definition ("matched by text").
 * egret denies CREATING a materialized view on an engine with classified
 * columns (A3), but cannot remove one that exists, and an existing one may hold
 * copies of the classified values. The list is read-only.
 *
 * Three outcomes, never conflated (FE-STATE-1): the request failed, an engine
 * could not be listed (`starrocksMaterializedViews` is `null`), or the list is
 * genuinely empty. Only the last may say "none". Engines that were not asked
 * (external ones, which egret never connects to, and ones not running) are
 * named as not checked, with the reason, whichever of the three it is.
 *
 * `engineGated`: the tag has a role clearance, so it compiles to the A3 deny.
 * Only then does the cost sentence apply; a group-only or informational tag
 * contributes no deny (Task 4 review M-4).
 */
export default function ViewExposureSection({
  tagId,
  engineGated,
}: {
  tagId: string;
  engineGated: boolean;
}) {
  const query = useControlPlaneTagsGetViewExposure(tagId, { query: { retry: false } });
  const exposure = query.data?.status === 200 ? query.data.data : undefined;
  const views = exposure?.starrocksMaterializedViews;
  const notChecked = exposure?.enginesNotChecked ?? [];

  return (
    <section className="space-y-3" aria-labelledby="view-exposure-heading">
      <div>
        <h2 id="view-exposure-heading" className="text-base font-semibold">
          Existing materialized views
        </h2>
        <p className="text-sm text-muted-foreground">
          StarRocks materialized views that read a table of this classification. StarRocks keeps
          every materialized view in default_catalog.
        </p>
      </div>

      <Badge
        variant="warning"
        className="w-fit items-start gap-1.5 whitespace-normal text-left font-normal"
      >
        <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" />
        <span>
          These views may hold real classified values; egret does not drop them.{" "}
          {engineGated
            ? "Because of this clearance, users not cleared for every classification on an engine cannot create any materialized view on that engine, even over unclassified data."
            : "This classification has no role clearance, so it does not stop anyone creating materialized views."}
        </span>
      </Badge>

      {notChecked.length > 0 && (
        <div className="text-sm">
          <p className="text-muted-foreground">
            Not checked: egret did not ask these engines, so their materialized views are unknown.
          </p>
          <ul className="list-disc pl-5">
            {notChecked.map((e) => (
              <li key={e.engine}>
                {e.engine}: {notCheckedText(e.reason)}
              </li>
            ))}
          </ul>
        </div>
      )}

      {query.isLoading ? (
        <ListSkeleton rows={3} cols={3} />
      ) : query.isError ? (
        <Alert variant="destructive">
          <AlertDescription>
            Could not load the materialized views:{" "}
            {(query.error as { message?: string } | null)?.message ?? "unknown error"}. Reload the
            page to try again.
          </AlertDescription>
        </Alert>
      ) : !Array.isArray(views) ? (
        <Alert variant="destructive">
          <AlertDescription>
            {exposure?.starrocksListError ?? "Could not list the materialized views."} Check that
            the engine is reachable, then reload the page.
          </AlertDescription>
        </Alert>
      ) : views.length === 0 ? (
        <EmptyState title="No materialized views read this classification's tables." />
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>View</TableHead>
              <TableHead>Database</TableHead>
              <TableHead>Engine</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {views.map((v) => (
              <TableRow key={`${v.engine}/${v.catalog}/${v.database}/${v.name}`}>
                <TableCell className="font-mono text-xs">
                  {v.name}
                  {v.matchedBy === "text" && (
                    <Badge
                      variant="outline"
                      className="ml-2 font-sans font-normal"
                      title="This view has no base-table record, so it was matched because its definition names the classified catalog and database."
                    >
                      matched by text
                    </Badge>
                  )}
                </TableCell>
                <TableCell className="font-mono text-xs">{`${v.catalog}.${v.database}`}</TableCell>
                <TableCell>{v.engine}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </section>
  );
}

/** What a not-checked reason means, and what the reader can do about it. */
function notCheckedText(reason: string): string {
  if (reason === "external") {
    return "external. egret does not connect to engines it does not run; check this engine's materialized views directly.";
  }
  if (reason.startsWith("not running")) {
    return `${reason}. Start the engine, then reload the page.`;
  }
  return reason;
}
