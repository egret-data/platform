import { useState } from "react";
import { Check, Copy, Database, Eye, Loader2, Plus, Table2, Trash2 } from "lucide-react";
import {
  type NamespaceDto,
  nsDotted,
  useCatalogNamespaces,
  useCreateNamespace,
  useDeleteNamespace,
  useNamespaceContents,
  useTableDetail,
} from "@/api/catalog";
import { ApiError } from "@/api/client";
import { DetailSection } from "@/components/explore/DetailSection";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
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
import { timeAgo, fullTimestamp } from "@/lib/relativeTime";

const ROOT = "(root)";

export default function CatalogBrowse({ ws, wid }: { ws: string; wid: string }) {
  const namespaces = useCatalogNamespaces(ws, wid);
  const deleteMutation = useDeleteNamespace(ws, wid);

  const [ns, setNs] = useState<string | null>(null);
  const [table, setTable] = useState<string | null>(null);
  const [addOpen, setAddOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  const contents = useNamespaceContents(ws, wid, ns);
  const detail = useTableDetail(ws, wid, ns, table);

  function selectNamespace(value: string) {
    setNs(value);
    setTable(null);
  }

  function confirmDelete() {
    const dotted = deleteTarget;
    if (!dotted) return;
    setDeleteError(null);
    deleteMutation.mutate(dotted, {
      onSuccess: () => {
        if (ns === dotted) {
          setNs(null);
          setTable(null);
        }
        setDeleteTarget(null);
      },
      onError: (err) => {
        setDeleteError((err as ApiError).message ?? "Failed to delete namespace.");
      },
    });
  }

  return (
    <div className="p-4">
      <div className="grid grid-cols-[16rem_1fr] gap-4">
        {/* Left: namespaces */}
        <div className="rounded border border-border">
          <div className="flex items-center justify-between border-b border-border px-3 py-2 text-xs font-medium uppercase tracking-wider text-muted-foreground">
            <span>Namespaces</span>
            <Button
              variant="ghost"
              size="icon"
              className="h-6 w-6 text-muted-foreground"
              onClick={() => setAddOpen(true)}
              aria-label="New namespace"
            >
              <Plus className="h-4 w-4" />
            </Button>
          </div>
          {namespaces.isLoading ? (
            <div className="p-3">
              <ListSkeleton rows={5} cols={1} />
            </div>
          ) : namespaces.isError ? (
            <p className="p-3 text-sm text-destructive">Failed to load namespaces.</p>
          ) : (namespaces.data ?? []).length === 0 ? (
            <p className="p-3 text-sm text-muted-foreground">No namespaces.</p>
          ) : (
            <ul className="p-1">
              {(namespaces.data ?? []).map((n) => {
                const dotted = nsDotted(n.levels);
                const active = dotted === ns;
                return (
                  <li key={dotted} className="group flex items-center gap-1">
                    <button
                      type="button"
                      onClick={() => selectNamespace(dotted)}
                      className={`flex min-w-0 flex-1 items-center gap-2 rounded px-2 py-1.5 text-left text-sm transition-colors ${
                        active
                          ? "bg-accent text-accent-foreground"
                          : "text-muted-foreground hover:bg-accent/50 hover:text-foreground"
                      }`}
                    >
                      <Database className="h-4 w-4 shrink-0" />
                      <span className="truncate font-mono">{dotted}</span>
                    </button>
                    <Button
                      variant="ghost"
                      size="icon"
                      className="h-7 w-7 shrink-0 text-muted-foreground opacity-0 hover:text-destructive group-hover:opacity-100"
                      onClick={() => {
                        setDeleteError(null);
                        setDeleteTarget(dotted);
                      }}
                      aria-label={`Delete ${dotted}`}
                    >
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  </li>
                );
              })}
            </ul>
          )}
        </div>

        {/* Right: tables/views + detail */}
        <div className="min-w-0">
          {!ns ? (
            <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
              <p>Select a namespace to browse its tables and views.</p>
            </div>
          ) : table ? (
            <TableDetailView ns={ns} table={table} detail={detail} onBack={() => setTable(null)} />
          ) : (
            <NamespaceContentsView ns={ns} contents={contents} onSelectTable={setTable} />
          )}
        </div>
      </div>

      {/* New namespace dialog — keyed so useState resets between opens */}
      <NewNamespaceDialog
        key={addOpen ? "open" : "closed"}
        ws={ws}
        wid={wid}
        open={addOpen}
        onOpenChange={setAddOpen}
        namespaces={namespaces.data ?? []}
      />

      {/* Delete confirm */}
      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          // Keep deleteError visible after the dialog auto-closes on a rejected
          // delete; it's cleared when the next delete target is opened.
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete «${deleteTarget}»?` : "Delete namespace?"}
        description="Only an empty namespace can be deleted. If it still has tables or child namespaces the server will reject it."
        confirmText="delete"
        onConfirm={confirmDelete}
      />
      {deleteError && (
        <Alert variant="destructive" className="mt-4">
          <AlertDescription>{deleteError}</AlertDescription>
        </Alert>
      )}
    </div>
  );
}

function NewNamespaceDialog({
  ws,
  wid,
  open,
  onOpenChange,
  namespaces,
}: {
  ws: string;
  wid: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  namespaces: NamespaceDto[];
}) {
  const createMutation = useCreateNamespace(ws, wid);
  const [name, setName] = useState("");
  const [parent, setParent] = useState<string>(ROOT);
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await createMutation.mutateAsync({
        parent: parent === ROOT ? null : parent,
        name: name.trim(),
      });
      onOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to create namespace.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>New namespace</DialogTitle>
            <DialogDescription>
              Create a namespace under a parent, or at the root of the warehouse.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="ns-name">Name</Label>
              <Input
                id="ns-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. sales"
                required
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="ns-parent">Parent</Label>
              <Select value={parent} onValueChange={setParent}>
                <SelectTrigger id="ns-parent" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={ROOT}>{ROOT}</SelectItem>
                  {namespaces.map((n) => {
                    const dotted = nsDotted(n.levels);
                    return (
                      <SelectItem key={dotted} value={dotted}>
                        {dotted}
                      </SelectItem>
                    );
                  })}
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
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={createMutation.isPending || name.trim().length === 0}>
              {createMutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function NamespaceContentsView({
  ns,
  contents,
  onSelectTable,
}: {
  ns: string;
  contents: ReturnType<typeof useNamespaceContents>;
  onSelectTable: (t: string) => void;
}) {
  if (contents.isLoading) return <ListSkeleton cols={2} />;
  if (contents.isError) return <p className="text-sm text-destructive">Failed to load tables.</p>;

  const tables = contents.data?.tables ?? [];
  const views = contents.data?.views ?? [];

  if (tables.length === 0 && views.length === 0) {
    return (
      <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
        <p>
          <span className="font-mono">{ns}</span> has no tables or views.
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <div>
        <h2 className="mb-2 text-sm font-medium text-muted-foreground">Tables</h2>
        {tables.length === 0 ? (
          <p className="text-sm text-muted-foreground">No tables.</p>
        ) : (
          <ul className="rounded border border-border">
            {tables.map((t) => (
              <li key={t} className="border-b border-border last:border-b-0">
                <button
                  type="button"
                  onClick={() => onSelectTable(t)}
                  className="flex w-full items-center gap-2 px-3 py-2 text-left text-sm hover:bg-accent"
                >
                  <Table2 className="h-4 w-4 shrink-0 text-muted-foreground" />
                  <span className="font-mono">{t}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>

      {views.length > 0 && (
        <div>
          <h2 className="mb-2 text-sm font-medium text-muted-foreground">Views</h2>
          <ul className="rounded border border-border">
            {views.map((v) => (
              <li key={v} className="flex items-center gap-2 border-b border-border px-3 py-2 text-sm last:border-b-0">
                <Eye className="h-4 w-4 shrink-0 text-muted-foreground" />
                <span className="font-mono">{v}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

/** Compact one-line summary picking the common Iceberg snapshot counters. */
function snapshotSummary(summary: Record<string, string>): string {
  const parts: string[] = [];
  if (summary["added-records"] != null) parts.push(`+${summary["added-records"]} rows`);
  if (summary["added-data-files"] != null) parts.push(`+${summary["added-data-files"]} files`);
  if (summary["total-records"] != null) parts.push(`${summary["total-records"]} total`);
  return parts.length > 0 ? parts.join(", ") : "—";
}

function TableDetailView({
  ns,
  table,
  detail,
  onBack,
}: {
  ns: string;
  table: string;
  detail: ReturnType<typeof useTableDetail>;
  onBack: () => void;
}) {
  return (
    <div className="max-w-5xl space-y-4">
      <div className="flex items-center gap-2 text-sm">
        <button type="button" onClick={onBack} className="text-muted-foreground hover:text-foreground hover:underline">
          {ns}
        </button>
        <span className="text-muted-foreground">/</span>
        <span className="font-mono font-medium">{table}</span>
      </div>

      {detail.isLoading ? (
        <ListSkeleton cols={3} />
      ) : detail.isError ? (
        <p className="text-sm text-destructive">Failed to load table detail.</p>
      ) : detail.data ? (
        <>
          <Card className="space-y-3 p-4">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-mono text-base font-medium">{table}</span>
              {detail.data.format_version != null && (
                <Badge variant="secondary">format v{detail.data.format_version}</Badge>
              )}
            </div>
            <div className="flex items-center gap-1.5 text-sm">
              <span className="truncate font-mono text-muted-foreground">{detail.data.location}</span>
              <CopyButton value={detail.data.location} />
            </div>
            <div className="flex flex-wrap gap-x-6 gap-y-1 text-sm">
              <span>
                <span className="text-muted-foreground">Schema id</span>{" "}
                <span className="font-mono">{detail.data.current_schema_id}</span>
              </span>
              <span>
                <span className="text-muted-foreground">Current snapshot</span>{" "}
                <span className="font-mono">{detail.data.current_snapshot_id ?? "—"}</span>
              </span>
              {detail.data.last_updated_ms != null && (
                <span
                  className="text-muted-foreground"
                  title={fullTimestamp(new Date(detail.data.last_updated_ms).toISOString())}
                >
                  updated {timeAgo(new Date(detail.data.last_updated_ms).toISOString())}
                </span>
              )}
            </div>
          </Card>

          <DetailSection title="Schema">
            <table className="text-sm">
              <thead className="text-left text-muted-foreground">
                <tr className="border-b border-border">
                  <th className="py-1.5 pr-8 font-medium">Column</th>
                  <th className="py-1.5 pr-8 font-medium">Type</th>
                  <th className="py-1.5 font-medium">Nullable</th>
                </tr>
              </thead>
              <tbody>
                {detail.data.columns.map((c) => (
                  <tr key={c.name} className="border-b border-border last:border-b-0">
                    <td className="py-1.5 pr-8 font-mono">{c.name}</td>
                    <td className="py-1.5 pr-8 font-mono text-muted-foreground">{c.type}</td>
                    <td className="py-1.5 text-muted-foreground">{c.required ? "No" : "Yes"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </DetailSection>

          <div className="grid gap-4 md:grid-cols-2">
            <DetailSection title="Partition spec">
              {(detail.data.partition_fields ?? []).length > 0 ? (
                <table className="text-sm">
                  <thead className="text-left text-muted-foreground">
                    <tr className="border-b border-border">
                      <th className="py-1.5 pr-8 font-medium">Field</th>
                      <th className="py-1.5 pr-8 font-medium">Source column</th>
                      <th className="py-1.5 font-medium">Transform</th>
                    </tr>
                  </thead>
                  <tbody>
                    {(detail.data.partition_fields ?? []).map((p) => (
                      <tr key={p.name} className="border-b border-border last:border-b-0">
                        <td className="py-1.5 pr-8 font-mono">{p.name}</td>
                        <td className="py-1.5 pr-8 font-mono text-muted-foreground">{p.source_column}</td>
                        <td className="py-1.5 font-mono text-muted-foreground">{p.transform}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : (
                <p className="text-sm text-muted-foreground">Unpartitioned</p>
              )}
            </DetailSection>

            {(detail.data.sort_fields ?? []).length > 0 && (
              <DetailSection title="Sort order">
                <table className="text-sm">
                  <thead className="text-left text-muted-foreground">
                    <tr className="border-b border-border">
                      <th className="py-1.5 pr-8 font-medium">Column</th>
                      <th className="py-1.5 pr-8 font-medium">Transform</th>
                      <th className="py-1.5 pr-8 font-medium">Direction</th>
                      <th className="py-1.5 font-medium">Nulls</th>
                    </tr>
                  </thead>
                  <tbody>
                    {(detail.data.sort_fields ?? []).map((s) => (
                      <tr key={s.column} className="border-b border-border last:border-b-0">
                        <td className="py-1.5 pr-8 font-mono">{s.column}</td>
                        <td className="py-1.5 pr-8 font-mono text-muted-foreground">{s.transform}</td>
                        <td className="py-1.5 pr-8">{s.direction}</td>
                        <td className="py-1.5">{s.null_order}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </DetailSection>
            )}
          </div>

          <DetailSection title="Snapshots">
            {(detail.data.snapshots ?? []).length > 0 ? (
              <table className="text-sm">
                <thead className="text-left text-muted-foreground">
                  <tr className="border-b border-border">
                    <th className="py-1.5 pr-8 font-medium">Snapshot id</th>
                    <th className="py-1.5 pr-8 font-medium">Time</th>
                    <th className="py-1.5 pr-8 font-medium">Operation</th>
                    <th className="py-1.5 font-medium">Summary</th>
                  </tr>
                </thead>
                <tbody>
                  {(detail.data.snapshots ?? []).map((s) => (
                    <tr key={s.snapshot_id} className="border-b border-border last:border-b-0">
                      <td className="py-1.5 pr-8 font-mono">{s.snapshot_id}</td>
                      <td
                        className="py-1.5 pr-8 text-muted-foreground"
                        title={fullTimestamp(new Date(s.timestamp_ms).toISOString())}
                      >
                        {timeAgo(new Date(s.timestamp_ms).toISOString())}
                      </td>
                      <td className="py-1.5 pr-8">{s.operation}</td>
                      <td className="py-1.5 text-muted-foreground">{snapshotSummary(s.summary)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            ) : (
              <p className="text-sm text-muted-foreground">No snapshots yet.</p>
            )}
          </DetailSection>

          {Object.keys(detail.data.properties).length > 0 && (
            <DetailSection title="Properties">
              <table className="text-sm">
                <tbody>
                  {Object.entries(detail.data.properties).map(([k, v]) => (
                    <tr key={k} className="border-b border-border last:border-b-0">
                      <td className="py-1.5 pr-8 font-mono text-muted-foreground">{k}</td>
                      <td className="py-1.5 font-mono break-all">{v}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </DetailSection>
          )}
        </>
      ) : null}
    </div>
  );
}

/** Copy-to-clipboard icon button with a transient check confirmation. */
function CopyButton({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      onClick={() => {
        navigator.clipboard?.writeText(value);
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      }}
      className="shrink-0 rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground"
      title="Copy location"
      aria-label="Copy location"
    >
      {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
    </button>
  );
}
