import { Link } from "react-router-dom";

import { ApiError } from "@/api/client";
import type { EngineConflictBody } from "@/api/generated/model/engineConflictBody";
import type { EngineDependentDto } from "@/api/generated/model/engineDependentDto";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

const KIND_LABEL: Record<string, string> = {
  dataapi_endpoint: "Data API endpoints",
  dataapi_endpoint_trashed: "Data API endpoints in trash",
  schedule: "Alerts & reports",
  dataset: "Datasets",
};

/** Where each kind is managed, under `/explore/{slug}/`. Trashed endpoints live
 * behind the Trash toggle of the same Endpoints page. */
const KIND_PATH: Record<string, string> = {
  dataapi_endpoint: "data-api/endpoints",
  dataapi_endpoint_trashed: "data-api/endpoints",
  schedule: "alerts",
  dataset: "datasets",
};

/** The dependents carried by an engine delete/unassign 409, or `null` for any other error. */
export function dependentsFromError(e: unknown): EngineDependentDto[] | null {
  if (!(e instanceof ApiError) || e.code !== "engine_has_dependents") return null;
  const dependents = (e.body as Partial<EngineConflictBody> | undefined)?.dependents;
  return Array.isArray(dependents) && dependents.length > 0 ? dependents : null;
}

/** What still uses an engine's workspace connections, one row per workspace and kind. */
export function EngineDependentsTable({ dependents }: { dependents: EngineDependentDto[] }) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Workspace</TableHead>
          <TableHead>What</TableHead>
          <TableHead className="text-right">Count</TableHead>
          <TableHead>Examples</TableHead>
          <TableHead>
            <span className="sr-only">Link</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {dependents.map((d) => (
          <TableRow key={`${d.workspaceId}:${d.kind}`}>
            <TableCell>{d.workspaceName}</TableCell>
            <TableCell>{KIND_LABEL[d.kind] ?? d.kind}</TableCell>
            <TableCell className="text-right tabular-nums">{d.count}</TableCell>
            <TableCell className="text-muted-foreground">
              {d.sample.join(", ")}
              {d.count > d.sample.length ? ", …" : ""}
            </TableCell>
            <TableCell>
              <Link
                className="text-primary underline-offset-4 hover:underline"
                to={`/explore/${d.workspaceSlug}/${KIND_PATH[d.kind] ?? ""}`}
              >
                Open
              </Link>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
