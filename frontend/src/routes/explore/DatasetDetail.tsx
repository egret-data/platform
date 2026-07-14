import { useEffect, useState } from "react";
import { useParams, useNavigate, useSearchParams, Link } from "react-router-dom";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Badge } from "@/components/ui/badge";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { useDataset, useRefreshDataset, useDeleteDataset, useUpdateDataset } from "@/api/datasets";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { ColumnsTab } from "@/components/dataset/ColumnsTab";
import { MetricsTab } from "@/components/dataset/MetricsTab";
import { PreviewTab } from "@/components/dataset/PreviewTab";

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

  // Edit form is dirty when any editable field diverges from the loaded dataset.
  // (sql is only editable for virtual datasets.) beforeunload-only guard.
  const dirty = editing && !!ds && (
    name !== ds.name ||
    description !== ds.description ||
    (ds.kind === "virtual" && sql !== (ds.sql ?? ""))
  );
  useUnsavedChangesWarning(dirty);

  // Detail tab in the URL (?tab=), default "columns" (preserves prior behavior).
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

  const source = ds.kind === "virtual" ? ds.sql : [ds.catalog, ds.schema, ds.table].filter(Boolean).join(".");

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
      <div className="mb-4 flex items-start justify-between">
        <div className="min-w-0 flex-1">
          <Link className="text-sm text-muted-foreground hover:underline" to={listHref}>← Datasets</Link>
          {editing ? (
            <div className="mt-1 max-w-2xl space-y-2">
              <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="Dataset name" />
              <Input value={description} onChange={(e) => setDescription(e.target.value)} placeholder="Description (optional)" />
              {ds.kind === "virtual" && (
                <Textarea className="h-40 font-mono text-sm" value={sql} onChange={(e) => setSql(e.target.value)} placeholder="SELECT …" />
              )}
            </div>
          ) : (
            <>
              <h1 className="text-lg font-semibold">{ds.name} <Badge variant="secondary" className="ml-2">{ds.kind}</Badge></h1>
              {ds.description && <p className="text-sm text-muted-foreground">{ds.description}</p>}
              <pre className="mt-1 max-w-2xl overflow-auto rounded bg-muted p-2 text-xs">{source}</pre>
            </>
          )}
        </div>
        <div className="flex shrink-0 gap-2">
          {editing ? (
            <>
              <Button disabled={update.isPending} onClick={save}>{update.isPending ? "Saving…" : "Save"}</Button>
              <Button variant="outline" onClick={() => { if (dirty && !window.confirm("Discard unsaved changes?")) return; setEditing(false); }}>Cancel</Button>
            </>
          ) : (
            <>
              <Button variant="outline" onClick={startEdit}>Edit</Button>
              <Button variant="outline" disabled={refresh.isPending}
                onClick={() => refresh.mutateAsync().then(() => toast.success("Refreshed")).catch((e) => toast.error(String(e)))}>
                {refresh.isPending ? "Refreshing…" : "Refresh"}
              </Button>
              <Button variant="destructive" onClick={() => setConfirmDelete(true)}>Delete</Button>
            </>
          )}
        </div>
      </div>

      <Tabs value={tab} onValueChange={setTab} className="mt-4">
        <TabsList>
          <TabsTrigger value="columns">Columns ({ds.columns.length})</TabsTrigger>
          <TabsTrigger value="metrics">Metrics ({ds.metrics.length})</TabsTrigger>
          <TabsTrigger value="preview">Preview</TabsTrigger>
        </TabsList>
        <TabsContent value="columns">
          <ColumnsTab ws={ws} datasetId={id} columns={ds.columns} />
        </TabsContent>
        <TabsContent value="metrics">
          <MetricsTab ws={ws} datasetId={id} metrics={ds.metrics} />
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
