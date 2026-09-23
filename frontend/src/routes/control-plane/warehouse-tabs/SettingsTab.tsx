import { useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronRight, ListChecks, Pencil } from "lucide-react";

import {
  type WarehouseConfig,
  type WarehouseTask,
  getWarehouseConfig,
  listStorage,
  listWarehouseTasks,
  setDeleteProfile,
  setFormatPolicy,
  setWarehouseStatus,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { ListSkeleton } from "@/components/explore/ListSkeleton";

export function SettingsTab({
  warehouseId,
  storageConnId,
}: {
  warehouseId: string;
  storageConnId?: string;
}) {
  const queryClient = useQueryClient();
  const config = useQuery<WarehouseConfig, ApiError>({
    queryKey: ["cp", "warehouse-config", warehouseId],
    queryFn: () => getWarehouseConfig(warehouseId),
    retry: false,
  });
  const storageQ = useQuery({
    queryKey: ["control-plane", "storage"],
    queryFn: listStorage,
    retry: false,
  });
  const storageName = storageQ.data?.find((s) => s.id === storageConnId)?.name;
  const storageLink = storageConnId
    ? `/control-plane/storage/${storageConnId}`
    : "/control-plane/storage";
  const c = config.data;
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["cp", "warehouse-config", warehouseId] });

  const statusMut = useMutation<void, ApiError, boolean>({
    mutationFn: (active) => setWarehouseStatus(warehouseId, active),
    onSuccess: invalidate,
  });
  const ttlMut = useMutation<void, ApiError, number | null>({
    mutationFn: (seconds) => setDeleteProfile(warehouseId, seconds),
    onSuccess: invalidate,
  });
  const fmtMut = useMutation<void, ApiError, number[]>({
    mutationFn: (versions) => setFormatPolicy(warehouseId, versions),
    onSuccess: invalidate,
  });

  const [fmt, setFmt] = useState("1,2");
  const [editing, setEditing] = useState(false);
  // Edit-mode drafts; each Value cell is a single Select for a light, uniform look.
  const [editActive, setEditActive] = useState(false);
  const [retChoice, setRetChoice] = useState("hard"); // "hard" | "<days>"

  const currentDays =
    c?.deleteProfileSeconds != null ? Math.round(c.deleteProfileSeconds / 86400) : null;
  const retentionValue = currentDays != null ? `${currentDays} day${currentDays === 1 ? "" : "s"}` : "Hard delete";
  const fmtLabel =
    fmt === "2" ? "Require v2" : fmt === "2,3" ? "Allow v2 and v3" : "Allow v1 and v2";

  const DAY_PRESETS = ["7", "14", "30", "60", "90"];
  // Keep a custom current window selectable, not just the presets.
  const retDayOptions =
    currentDays != null && !DAY_PRESETS.includes(String(currentDays))
      ? [String(currentDays), ...DAY_PRESETS]
      : DAY_PRESETS;

  const startEdit = () => {
    setEditActive(!!c?.active);
    setRetChoice(currentDays != null ? String(currentDays) : "hard");
    setEditing(true);
  };
  const save = async () => {
    try {
      if (editActive !== !!c?.active) await statusMut.mutateAsync(editActive);
      await ttlMut.mutateAsync(retChoice === "hard" ? null : Math.max(7, Number(retChoice)) * 86400);
      await fmtMut.mutateAsync(fmt.split(",").map(Number));
      setEditing(false);
    } catch {
      /* errors surface in the alert below the Edit row */
    }
  };
  const savePending = statusMut.isPending || ttlMut.isPending || fmtMut.isPending;
  const saveError = statusMut.error ?? ttlMut.error ?? fmtMut.error;

  return (
    <div className="space-y-4">
      {/* An unprovisioned warehouse 404s on config, so status/retention/format
          writes fail until its catalog is live — say so instead of letting the
          saves silently error.

          Only on a 404. "Isn't provisioned yet" is a claim about the WAREHOUSE,
          and a 403 or a 500 is not evidence for it (SP3c-1 Task 8 review F1,
          which found the same sentence on the Connect tab making the same
          mistake). Any other failure gets the server's own words. */}
      {config.isError &&
        (config.error?.status === 404 ? (
          <p className="rounded-md border bg-muted/40 px-4 py-2.5 text-sm text-muted-foreground">
            This warehouse isn&apos;t provisioned yet — status and table policies take effect once
            its catalog is live.
          </p>
        ) : (
          <Alert variant="destructive" data-testid="warehouse-config-error">
            <AlertDescription>
              Couldn&apos;t read this warehouse&apos;s settings:{" "}
              {config.error?.message ?? "the request failed"}. The values below may be stale, and
              saving may fail for the same reason.
            </AlertDescription>
          </Alert>
        ))}
      <div className="flex items-center justify-end gap-2">
        {editing ? (
          <>
            <Button size="sm" variant="outline" disabled={savePending} onClick={() => setEditing(false)}>
              Cancel
            </Button>
            <Button size="sm" disabled={savePending} onClick={save}>
              Save
            </Button>
          </>
        ) : (
          <Button size="sm" variant="outline" onClick={startEdit}>
            <Pencil className="mr-1 h-4 w-4" /> Edit
          </Button>
        )}
      </div>

      {editing && saveError && (
        <Alert variant="destructive">
          <AlertDescription>{saveError.message ?? "Couldn't save settings."}</AlertDescription>
        </Alert>
      )}

      {config.isLoading ? (
        <ListSkeleton rows={4} cols={3} />
      ) : (
        <div className="overflow-x-auto rounded-md border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-48">Setting</TableHead>
                <TableHead className="hidden md:table-cell">Details</TableHead>
                <TableHead>Value</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {/* Status — activation lifecycle (always a direct toggle). */}
              <TableRow>
                <TableCell className="align-middle font-medium">Status</TableCell>
                <TableCell className="hidden align-middle text-muted-foreground md:table-cell">
                  Whether the catalog is active and serving reads and writes.
                </TableCell>
                <TableCell className="align-middle">
                  {editing ? (
                    <Select
                      value={editActive ? "active" : "inactive"}
                      onValueChange={(v) => setEditActive(v === "active")}
                    >
                      <SelectTrigger aria-label="Status" className="h-8 w-72">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="active">Active</SelectItem>
                        <SelectItem value="inactive">Inactive</SelectItem>
                      </SelectContent>
                    </Select>
                  ) : (
                    <span className="inline-flex items-center gap-2 text-sm">
                      <span className={`h-1.5 w-1.5 rounded-full ${c?.active ? "bg-primary" : "bg-muted-foreground"}`} />
                      {c?.active ? "Active" : "Inactive"}
                    </span>
                  )}
                </TableCell>
              </TableRow>

              {/* Soft-delete retention — inline number input in edit mode. */}
              <TableRow>
                <TableCell className="align-middle font-medium">Soft-delete retention</TableCell>
                <TableCell className="hidden align-middle text-muted-foreground md:table-cell">
                  How long dropped tables stay recoverable before they are purged. Minimum ~7 days.
                </TableCell>
                <TableCell className="align-middle">
                  {editing ? (
                    <Select value={retChoice} onValueChange={setRetChoice}>
                      <SelectTrigger aria-label="Soft-delete retention" className="h-8 w-72">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="hard">Hard delete (no recovery)</SelectItem>
                        {retDayOptions.map((d) => (
                          <SelectItem key={d} value={d}>
                            {d} days
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  ) : (
                    <span className="text-sm">{retentionValue}</span>
                  )}
                </TableCell>
              </TableRow>

              {/* Format-version policy — inline select in edit mode. */}
              <TableRow>
                <TableCell className="align-middle font-medium">Format version</TableCell>
                <TableCell className="hidden align-middle text-muted-foreground md:table-cell">
                  Which Iceberg table format versions clients may create.
                </TableCell>
                <TableCell className="align-middle">
                  {editing ? (
                    <Select value={fmt} onValueChange={setFmt}>
                      <SelectTrigger aria-label="Allowed format versions" className="h-8 w-72">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="1,2">Allow v1 and v2</SelectItem>
                        <SelectItem value="2">Require v2</SelectItem>
                        <SelectItem value="2,3">Allow v2 and v3</SelectItem>
                      </SelectContent>
                    </Select>
                  ) : (
                    <span className="text-sm">{fmtLabel}</span>
                  )}
                </TableCell>
              </TableRow>

              {/* Storage connection — links out to the connection's detail page. */}
              <TableRow>
                <TableCell className="align-middle font-medium">Storage connection</TableCell>
                <TableCell className="hidden align-middle text-muted-foreground md:table-cell">
                  The connection this warehouse&apos;s data lives on. Its S3 key is managed there.
                </TableCell>
                <TableCell className="align-middle">
                  {editing ? (
                    // Styled like the other Selects for a uniform row, but it opens
                    // the connection's detail page rather than a dropdown.
                    <Button
                      asChild
                      variant="outline"
                      className="h-8 w-72 justify-between px-3 font-normal"
                    >
                      <Link to={storageLink}>
                        <span className="truncate">{storageName ?? "connection"}</span>
                        <ChevronRight className="h-4 w-4 shrink-0 opacity-50" />
                      </Link>
                    </Button>
                  ) : (
                    <Link to={storageLink} className="text-sm hover:text-primary hover:underline">
                      {storageName ?? "connection"}
                    </Link>
                  )}
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
      )}

      <WarehouseTasksCard warehouseId={warehouseId} />
    </div>
  );
}

function WarehouseTasksCard({ warehouseId }: { warehouseId: string }) {
  const tasks = useQuery<WarehouseTask[], ApiError>({
    queryKey: ["cp", "warehouse-tasks", warehouseId],
    queryFn: () => listWarehouseTasks(warehouseId),
    retry: false,
  });
  const rows = tasks.data ?? [];
  return (
    <section className="space-y-3">
      <div className="space-y-1">
        <h2 className="flex items-center gap-2 text-base font-semibold">
          <ListChecks className="h-4 w-4 text-muted-foreground" />
          Background tasks
        </h2>
        <p className="text-sm text-muted-foreground">
          The catalog&apos;s GC queues (tabular expiry / purge / task-log cleanup) that reclaim
          storage from dropped tables.
        </p>
      </div>
      {tasks.isError ? (
        <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-destructive">
          {tasks.error?.message ?? "Couldn't load background tasks."}
        </p>
      ) : tasks.isLoading ? (
        <ListSkeleton rows={3} cols={3} />
      ) : rows.length === 0 ? (
        <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
          No pending tasks.
        </p>
      ) : (
        <div className="overflow-x-auto rounded-md border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Queue</TableHead>
                <TableHead>Entity</TableHead>
                <TableHead>Status</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((t, i) => (
                <TableRow key={i}>
                  <TableCell className="font-mono text-xs">{t.queue}</TableCell>
                  <TableCell className="font-mono text-xs">{t.entity ?? "—"}</TableCell>
                  <TableCell>
                    <Badge variant="outline">{t.status}</Badge>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}
    </section>
  );
}

