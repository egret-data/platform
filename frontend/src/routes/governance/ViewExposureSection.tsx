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
 * exist in the databases holding this classification's columns (view
 * governance A4).
 *
 * egret denies CREATING a materialized view there, but cannot remove one that
 * exists, and an existing one may hold copies of the classified values. The
 * list is read-only: egret reports these views and does not drop them.
 *
 * Three outcomes, never conflated (FE-STATE-1): the request failed, the engine
 * could not be listed (`starrocksMaterializedViews` is `null`), or the list is
 * genuinely empty. Only the last may say "none".
 */
export default function ViewExposureSection({ tagId }: { tagId: string }) {
  const query = useControlPlaneTagsGetViewExposure(tagId, { query: { retry: false } });
  const exposure = query.data?.status === 200 ? query.data.data : undefined;
  const views = exposure?.starrocksMaterializedViews;

  return (
    <section className="space-y-3" aria-labelledby="view-exposure-heading">
      <div>
        <h2 id="view-exposure-heading" className="text-base font-semibold">
          Existing materialized views
        </h2>
        <p className="text-sm text-muted-foreground">
          StarRocks materialized views already in the databases that hold this classification&apos;s
          columns.
        </p>
      </div>

      <Badge
        variant="warning"
        className="w-fit items-start gap-1.5 whitespace-normal text-left font-normal"
      >
        <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" />
        These views may hold real classified values; egret does not drop them. Denying new
        materialized views does not clean up the ones that exist.
      </Badge>

      {query.isLoading ? (
        <ListSkeleton rows={3} cols={3} />
      ) : query.isError ? (
        <Alert variant="destructive">
          <AlertDescription>
            {(query.error as { message?: string } | null)?.message ??
              "Could not load the materialized views."}
          </AlertDescription>
        </Alert>
      ) : !Array.isArray(views) ? (
        <Alert variant="destructive">
          <AlertDescription>
            {exposure?.starrocksListError ?? "Could not list the materialized views."}
          </AlertDescription>
        </Alert>
      ) : views.length === 0 ? (
        <EmptyState title="No materialized views found in these databases." />
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
                <TableCell className="font-mono text-xs">{v.name}</TableCell>
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
