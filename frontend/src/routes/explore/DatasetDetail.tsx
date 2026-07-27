import { useEffect, useState } from "react";
import { useParams, useNavigate, useSearchParams, Link } from "react-router-dom";
import { toast } from "sonner";
import { BarChart3, Clock, Columns3, Pencil, RefreshCw, Sigma, Table2, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SqlEditor } from "@/components/sqllab/SqlEditor";
import { useDataset, useRefreshDataset, useDeleteDataset, useUpdateDataset } from "@/api/datasets";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { ColumnsTab } from "@/components/dataset/ColumnsTab";
import { MetricsTab } from "@/components/dataset/MetricsTab";
import { PreviewTab } from "@/components/dataset/PreviewTab";
import { UsageTab } from "@/components/dataset/UsageTab";

/** Compact "n unit(s)" stat chip for the header. */
function Stat({ icon, children }: { icon: React.ReactNode; children: React.ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
      {icon}
      {children}
    </span>
  );
}

export default function DatasetDetail() {
  const { workspace: ws = "", id = "" } = useParams();
  const nav = useNavigate();
  const listHref = `/explore/${ws}/datasets`;
  const { data: ds, isLoading, isError, error } = useDataset(ws, id || null);
  const refresh = useRefreshDataset(ws, id);
  const del = useDeleteDataset(ws);
  const update = useUpdateDataset(ws, id);

  const [editing, setEditing] = useState(false);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [sql, setSql] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [searchParams, setSearchParams] = useSearchParams();

  const dirty = editing && !!ds && (
    name !== ds.name ||
    description !== ds.description ||
    (ds.kind === "virtual" && sql !== (ds.sql ?? ""))
  );
  useUnsavedChangesWarning(dirty);

  const tab = searchParams.get("tab") ?? "columns";
  const setTab = (v: string) =>
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      if (v === "columns") next.delete("tab");
      else next.set("tab", v);
      return next;
    }, { replace: true });

  // Arriving from the list's "Edit" row action (…?edit=1) opens edit mode directly.
  useEffect(() => {
    if (ds && searchParams.get("edit") === "1") {
      setName(ds.name);
      setDescription(ds.description);
      setSql(ds.sql ?? "");
      setEditing(true);
      searchParams.delete("edit");
      setSearchParams(searchParams, { replace: true });
    }
  }, [ds, searchParams]);

  if (isLoading) return <div className="p-4"><ListSkeleton cols={3} /></div>;
  if (isError)
    return (
      <div className="p-4">
        <Alert variant="destructive">
          <AlertDescription>{(error as Error)?.message ?? "Failed to load dataset."}</AlertDescription>
        </Alert>
        <Link className="mt-3 inline-block text-sm underline" to={listHref}>← Back to datasets</Link>
      </div>
    );
  if (!ds) return <div className="p-4">Not found. <Link className="underline" to={listHref}>Back</Link></div>;

  const source = ds.kind === "virtual" ? (ds.sql ?? "") : [ds.catalog, ds.schema, ds.table].filter(Boolean).join(".");

  function startEdit() {
    if (!ds) return;
    setName(ds.name);
    setDescription(ds.description);
    setSql(ds.sql ?? "");
    setEditing(true);
  }

  async function save() {
    if (!ds) return;
    if (!name.trim()) { toast.error("Name required"); return; }
    const sqlChanged = ds.kind === "virtual" && sql.trim() !== (ds.sql ?? "").trim();
    try {
      await update.mutateAsync({
        name: name.trim(),
        description,
        sql: ds.kind === "virtual" ? sql : undefined,
      });
      setEditing(false);
      toast.success(sqlChanged ? "Saved — Refresh to re-sync columns" : "Saved");
    } catch (e) { toast.error(String(e)); }
  }

  return (
    <div className="p-4">
      <Link className="text-sm text-muted-foreground hover:underline" to={listHref}>← Datasets</Link>

      {/* Header card */}
      <div className="mt-2 rounded-lg border bg-card p-4">
        {editing ? (
          <div className="max-w-2xl space-y-2">
            <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="Dataset name" />
            <Input value={description} onChange={(e) => setDescription(e.target.value)} placeholder="Description (optional)" />
            {ds.kind === "virtual" && (
              <div className="h-56 overflow-hidden rounded-md border">
                <SqlEditor value={sql} onChange={setSql} schemaWords={[]} />
              </div>
            )}
            <div className="flex gap-2 pt-1">
              <Button disabled={update.isPending} onClick={save}>{update.isPending ? "Saving…" : "Save"}</Button>
              <Button variant="outline" onClick={() => { if (dirty && !window.confirm("Discard unsaved changes?")) return; setEditing(false); }}>Cancel</Button>
            </div>
          </div>
        ) : (
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <h1 className="truncate text-lg font-semibold">{ds.name}</h1>
                <Badge variant="secondary" className="gap-1">
                  {ds.kind === "virtual" ? <Sigma className="h-3 w-3" /> : <Table2 className="h-3 w-3" />}
                  {ds.kind}
                </Badge>
              </div>
              {ds.description && <p className="mt-0.5 text-sm text-muted-foreground">{ds.description}</p>}
              <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1">
                <Stat icon={<Columns3 className="h-3.5 w-3.5" />}>{ds.columns.length} columns</Stat>
                <Stat icon={<Sigma className="h-3.5 w-3.5" />}>{ds.metrics.length} metrics</Stat>
                <Stat icon={<Clock className="h-3.5 w-3.5" />}>updated <RelativeTime iso={ds.updated_at} /></Stat>
              </div>
              <div className="mt-2 flex items-start gap-1.5 text-xs text-muted-foreground">
                {ds.kind === "virtual" ? <Sigma className="mt-0.5 h-3.5 w-3.5 shrink-0" /> : <Table2 className="mt-0.5 h-3.5 w-3.5 shrink-0" />}
                <code title={source} className="min-w-0 max-w-2xl truncate rounded bg-muted px-1.5 py-0.5 font-mono">{source || "—"}</code>
              </div>
            </div>
            <div className="flex shrink-0 flex-wrap items-center gap-2">
              <Button asChild className="gap-1.5">
                <Link to={`/explore/${ws}/charts/new?dataset=${ds.id}`}><BarChart3 className="h-4 w-4" /> Create chart</Link>
              </Button>
              <Button variant="outline" size="icon" title="Edit" onClick={startEdit}><Pencil className="h-4 w-4" /></Button>
              <Button variant="outline" size="icon" title="Refresh columns" disabled={refresh.isPending}
                onClick={() => refresh.mutateAsync().then(() => toast.success("Refreshed")).catch((e) => toast.error(String(e)))}>
                <RefreshCw className={`h-4 w-4 ${refresh.isPending ? "animate-spin" : ""}`} />
              </Button>
              <Button variant="outline" size="icon" title="Delete" className="text-destructive hover:text-destructive" onClick={() => setConfirmDelete(true)}>
                <Trash2 className="h-4 w-4" />
              </Button>
            </div>
          </div>
        )}
      </div>

      <Tabs value={tab} onValueChange={setTab} className="mt-4">
        <TabsList>
          <TabsTrigger value="columns">Columns ({ds.columns.length})</TabsTrigger>
          <TabsTrigger value="metrics">Metrics ({ds.metrics.length})</TabsTrigger>
          <TabsTrigger value="usage">Usage</TabsTrigger>
          <TabsTrigger value="preview">Preview</TabsTrigger>
        </TabsList>
        <TabsContent value="columns">
          <ColumnsTab ws={ws} datasetId={id} columns={ds.columns} />
        </TabsContent>
        <TabsContent value="metrics">
          <MetricsTab ws={ws} datasetId={id} metrics={ds.metrics} />
        </TabsContent>
        <TabsContent value="usage">
          <UsageTab ws={ws} datasetId={id} />
        </TabsContent>
        <TabsContent value="preview">
          <PreviewTab ws={ws} datasetId={id} />
        </TabsContent>
      </Tabs>

      <ConfirmDeleteDialog
        open={confirmDelete}
        onOpenChange={setConfirmDelete}
        title={`Delete dataset “${ds.name}”?`}
        description="This removes the dataset and its column snapshot. The underlying table or query is not affected. This cannot be undone."
        onConfirm={() => del.mutate(id, { onSuccess: () => { toast.success("Deleted"); nav(listHref); }, onError: (e) => toast.error(String((e as Error)?.message ?? e)) })}
      />
    </div>
  );
}
