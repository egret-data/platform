import { useEffect, useState } from "react";
import { useParams, useNavigate, useSearchParams, Link } from "react-router-dom";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent,
  AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { useDataset, useRefreshDataset, useDeleteDataset, useUpdateDataset } from "@/api/datasets";

export default function DatasetDetail() {
  const { workspace: ws = "", id = "" } = useParams();
  const nav = useNavigate();
  const listHref = `/explore/${ws}/datasets`;
  const { data: ds, isLoading } = useDataset(ws, id || null);
  const refresh = useRefreshDataset(ws, id);
  const del = useDeleteDataset(ws);
  const update = useUpdateDataset(ws, id);

  const [editing, setEditing] = useState(false);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [sql, setSql] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [searchParams, setSearchParams] = useSearchParams();

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

  if (isLoading) return <div className="p-4 text-muted-foreground">Loading…</div>;
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
              <h1 className="text-lg font-semibold">{ds.name} <span className="ml-2 rounded bg-muted px-1.5 py-0.5 text-xs">{ds.kind}</span></h1>
              {ds.description && <p className="text-sm text-muted-foreground">{ds.description}</p>}
              <pre className="mt-1 max-w-2xl overflow-auto rounded bg-muted p-2 text-xs">{source}</pre>
            </>
          )}
        </div>
        <div className="flex shrink-0 gap-2">
          {editing ? (
            <>
              <Button disabled={update.isPending} onClick={save}>{update.isPending ? "Saving…" : "Save"}</Button>
              <Button variant="outline" onClick={() => setEditing(false)}>Cancel</Button>
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

      <table className="w-full text-sm">
        <thead className="text-left text-muted-foreground">
          <tr><th className="py-1">Column</th><th>Type</th><th>Class</th><th>Temporal</th></tr>
        </thead>
        <tbody>
          {ds.columns.map((c) => (
            <tr key={c.column_name} className="border-t">
              <td className="py-1 font-mono">{c.column_name}</td>
              <td className="font-mono text-muted-foreground">{c.data_type}</td>
              <td><span className="rounded bg-muted px-1.5 py-0.5 text-xs">{c.type_generic}</span></td>
              <td>{c.is_temporal ? "✓" : ""}</td>
            </tr>
          ))}
        </tbody>
      </table>

      <AlertDialog open={confirmDelete} onOpenChange={setConfirmDelete}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete dataset “{ds.name}”?</AlertDialogTitle>
            <AlertDialogDescription>
              This removes the dataset and its column snapshot. The underlying table or query is not affected. This cannot be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={() => del.mutate(id, { onSuccess: () => { toast.success("Deleted"); nav(listHref); } })}>
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
