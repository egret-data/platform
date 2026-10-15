import { AlertTriangle } from "lucide-react";

import { useControlPlaneTagsGetViewExposure } from "@/api/generated/control-plane-tags/control-plane-tags";
import type { TrinoViewItem } from "@/api/generated/model/trinoViewItem";
import type { ViewExposureDto } from "@/api/generated/model/viewExposureDto";
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
    <div className="space-y-6">
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
      <TrinoViews exposure={exposure} isLoading={query.isLoading} isError={query.isError} />
    </div>
  );
}

/**
 * The Trino half (view governance B5): the views in egret's catalogs whose
 * lineage reads this classification or a masked or row-filtered table, with
 * each view's security mode as stored (`trino.run-as-owner`), its owner and
 * whether the owner is cleared. Minimal by the user's ruling (2026-10-14): the
 * DG frontend is redesigned later as its own spec.
 *
 * `trinoViews` is `null` exactly when egret's catalogs could not all be
 * listed; that is an error, never "none" (FE-STATE-1).
 */
function TrinoViews({
  exposure,
  isLoading,
  isError,
}: {
  exposure: ViewExposureDto | undefined;
  isLoading: boolean;
  isError: boolean;
}) {
  const views = exposure?.trinoViews;
  return (
    <section className="space-y-3" aria-labelledby="trino-views-heading">
      <h2 id="trino-views-heading" className="text-base font-semibold">
        Trino views
      </h2>
      <p className="text-sm text-muted-foreground">
        DEFINER views run as their owner: base masks and row filters apply as the owner, and an
        uncleared owner blocks every reader. INVOKER views run as the reader, so prefer SECURITY
        INVOKER.
      </p>
      {isLoading ? (
        <ListSkeleton rows={3} cols={6} />
      ) : isError ? (
        <Alert variant="destructive">
          <AlertDescription>Could not load the Trino views. Reload the page to try again.</AlertDescription>
        </Alert>
      ) : !Array.isArray(views) ? (
        <Alert variant="destructive">
          <AlertDescription className="break-words">
            {exposure?.trinoListError ??
              "egret's views could not all be listed. Reload the page to try again."}
          </AlertDescription>
        </Alert>
      ) : views.length === 0 ? (
        <EmptyState title="No Trino views read this classification or a masked table." />
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>View</TableHead>
              <TableHead>Mode</TableHead>
              <TableHead>Owner</TableHead>
              <TableHead>Owner clearance</TableHead>
              <TableHead>Why listed</TableHead>
              <TableHead>Lineage</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {views.map((v) => (
              <TableRow key={`${v.catalog}/${v.schema}/${v.name}`}>
                <TableCell className="break-all font-mono text-xs">{`${v.catalog}.${v.schema}.${v.name}`}</TableCell>
                <TableCell className="whitespace-normal break-words">{modeText(v)}</TableCell>
                <TableCell className="break-all">{v.owner ?? "—"}</TableCell>
                <TableCell>{clearanceText(v)}</TableCell>
                <TableCell className="whitespace-normal">{reasonText(v)}</TableCell>
                <TableCell className="whitespace-normal break-words">{lineageText(v)}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </section>
  );
}

function modeText(v: TrinoViewItem): string {
  if (v.securityMode === "definer") return "DEFINER";
  if (v.securityMode === "invoker") return "INVOKER";
  // `securityModeError` is a short server message; the detail is in egret's log.
  return `unknown: ${v.securityModeError ?? "the view's metadata could not be read"}`;
}

function clearanceText(v: TrinoViewItem): string {
  if (v.securityMode !== "definer") return "—";
  if (v.ownerCleared === true) return "cleared";
  if (v.ownerCleared === false) return "not cleared";
  return "clearance unknown";
}

function reasonText(v: TrinoViewItem): string {
  if (v.reason === "mask_or_row_filter") return "reads a table with a mask or row filter";
  return v.lineageState === "computed" ? "reads classified data" : "may read classified data";
}

function lineageText(v: TrinoViewItem): string {
  if (v.lineageState === "computed") return "computed";
  if (v.lineageState === "missing") {
    return "lineage not computed; egret has not judged this view. Run the lineage sweep.";
  }
  // `lineageDetail` can carry an engine's raw error, so it is not shown here;
  // the lineage sweep's report names the reason.
  return "lineage could not be computed; egret has not judged this view. Run the lineage sweep, whose report names the reason.";
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
