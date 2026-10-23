import { useEffect, useState } from "react";
import { useParams, useNavigate, useSearchParams, Link } from "react-router-dom";
import { toast } from "sonner";
import { BadgeCheck, BarChart3, Clock, Columns3, Pencil, RefreshCw, Sigma, Table2, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Switch } from "@/components/ui/switch";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SqlEditor } from "@/components/sqllab/SqlEditor";
import { useAuth } from "@/auth/useAuth";
import { useDataset, useRefreshDataset, useUpdateDataset, useSetDatasetCertification } from "@/api/datasets";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { ColumnsTab } from "@/components/dataset/ColumnsTab";
import { MetricsTab } from "@/components/dataset/MetricsTab";
import { PreviewTab } from "@/components/dataset/PreviewTab";
import { UsageTab } from "@/components/dataset/UsageTab";
import { DeleteDatasetDialog } from "@/components/dataset/DeleteDatasetDialog";
import { refreshSummary } from "@/components/dataset/refreshSummary";
import { PageContainer } from "@/components/layout/PageContainer";
import { PageTitle } from "@/components/layout/PageTitle";
import { BackLink } from "@/components/layout/DetailHeader";

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
  const update = useUpdateDataset(ws, id);
  const setCertification = useSetDatasetCertification(ws, id);
  // PLATFORM-scoped, so unlike `dashboard:manage` this DOES appear in the session permission
  // list and can be read here. Mirrors DashboardView.tsx — platform_admin always passes.
  const { me } = useAuth();
  const canCertify = !!me && (me.platform_roles.includes("platform_admin") || me.permissions.includes("dataset:certify"));

  const [editing, setEditing] = useState(false);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [sql, setSql] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [certifyOpen, setCertifyOpen] = useState(false);
  // No `by`: the certifier is the SESSION now (0109), so there is nothing for the user to type.
  const [cert, setCert] = useState({ certified: false, details: "" });
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

  if (isLoading) return <PageContainer><ListSkeleton cols={3} /></PageContainer>;
  if (isError)
    return (
      <PageContainer>
        <Alert variant="destructive">
          <AlertDescription>{(error as Error)?.message ?? "Failed to load dataset."}</AlertDescription>
        </Alert>
        <div className="mt-3">
          <BackLink to={listHref}>Datasets</BackLink>
        </div>
      </PageContainer>
    );
  if (!ds)
    return (
      <PageContainer>
        <p className="mb-3">Not found.</p>
        <BackLink to={listHref}>Datasets</BackLink>
      </PageContainer>
    );

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

  function openCertify() {
    if (!ds) return;
    setCert({ certified: ds.certified, details: ds.certification_details });
    setCertifyOpen(true);
  }
  async function saveCert() {
    if (!ds) return;
    try {
      // Its own endpoint, not `update` — see `useSetDatasetCertification`.
      await setCertification.mutateAsync({ certified: cert.certified, details: cert.details });
      setCertifyOpen(false);
      toast.success(cert.certified ? "Dataset certified" : "Certification removed");
    } catch (e) { toast.error(String(e)); }
  }

  return (
    <PageContainer>
      <BackLink to={listHref}>Datasets</BackLink>

      {/* Header card */}
      <div className="mt-2 rounded-lg border bg-card p-4">
        {editing ? (
          <div className="space-y-4">
            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-1.5">
                <Label htmlFor="ds-name">Name</Label>
                <Input id="ds-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="Dataset name" />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="ds-desc">Description</Label>
                <Input id="ds-desc" value={description} onChange={(e) => setDescription(e.target.value)} placeholder="What this dataset is for (optional)" />
              </div>
            </div>
            {ds.kind === "virtual" && (
              <div className="space-y-1.5">
                <Label>SQL query</Label>
                <div className="h-64 overflow-hidden rounded-md border">
                  <SqlEditor value={sql} onChange={setSql} schemaWords={[]} />
                </div>
                <p className="text-xs text-muted-foreground">
                  The query result becomes the dataset. After changing it, click <span className="font-medium">Refresh</span> to re-sync the columns.
                </p>
              </div>
            )}
            <div className="flex gap-2">
              <Button disabled={update.isPending} onClick={save}>{update.isPending ? "Saving…" : "Save"}</Button>
              <Button variant="outline" onClick={() => { if (dirty && !window.confirm("Discard unsaved changes?")) return; setEditing(false); }}>Cancel</Button>
            </div>
          </div>
        ) : (
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <PageTitle className="truncate">{ds.name}</PageTitle>
                <Badge variant="secondary" className="gap-1">
                  {ds.kind === "virtual" ? <Sigma className="h-3 w-3" /> : <Table2 className="h-3 w-3" />}
                  {ds.kind}
                </Badge>
                {ds.certified && (
                  <Badge className="gap-1" title={[ds.certified_by && `Certified by ${ds.certified_by}`, ds.certification_details].filter(Boolean).join(" — ") || "Certified"}>
                    <BadgeCheck className="h-3 w-3" /> Certified
                  </Badge>
                )}
              </div>
              {ds.description && <p className="mt-0.5 text-sm text-muted-foreground">{ds.description}</p>}
              <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1">
                <Stat icon={<Columns3 className="h-3.5 w-3.5" />}>{ds.columns.length} {ds.columns.length === 1 ? "column" : "columns"}</Stat>
                <Stat icon={<Sigma className="h-3.5 w-3.5" />}>{ds.metrics.length} {ds.metrics.length === 1 ? "metric" : "metrics"}</Stat>
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
              {/* Withheld, not disabled, exactly as on a dashboard: a greyed control advertises
                  a capability the reader does not have. The BADGE itself stays visible below —
                  reading who vouched for the data is not a stewardship act. */}
              {canCertify && (
                <Button variant="outline" size="icon" title={ds.certified ? "Certification" : "Certify dataset"} onClick={openCertify}>
                  <BadgeCheck className={`h-4 w-4 ${ds.certified ? "text-primary" : ""}`} />
                </Button>
              )}
              <Button variant="outline" size="icon" title="Edit" onClick={startEdit}><Pencil className="h-4 w-4" /></Button>
              <Button variant="outline" size="icon" title="Refresh columns" disabled={refresh.isPending}
                onClick={() => refresh.mutateAsync().then((r) => toast.success(refreshSummary(r))).catch((e) => toast.error(String(e)))}>
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
          <PreviewTab ws={ws} datasetId={id} columns={ds.columns} />
        </TabsContent>
      </Tabs>

      <Dialog open={certifyOpen} onOpenChange={setCertifyOpen}>
        <DialogContent>
          <DialogHeader><DialogTitle>Certify dataset</DialogTitle></DialogHeader>
          <p className="-mt-1 text-sm text-muted-foreground">Mark this as a trusted, official dataset so people know it's vetted.</p>
          <div className="space-y-3 pt-1">
            <label className="flex items-center gap-2 text-sm">
              <Switch checked={cert.certified} onCheckedChange={(v) => setCert((c) => ({ ...c, certified: v }))} /> Certified
            </label>
            {cert.certified && (
              <>
                <div className="space-y-1.5">
                  <Label htmlFor="cert-det">Details <span className="font-normal text-muted-foreground">(optional)</span></Label>
                  <Input id="cert-det" placeholder="Why it's trusted" value={cert.details} onChange={(e) => setCert((c) => ({ ...c, details: e.target.value }))} />
                </div>
              </>
            )}
            <div className="flex justify-end gap-2">
              <Button variant="outline" onClick={() => setCertifyOpen(false)}>Cancel</Button>
              <Button onClick={saveCert} disabled={update.isPending}>{update.isPending ? "Saving…" : "Save"}</Button>
            </div>
          </div>
        </DialogContent>
      </Dialog>

      <DeleteDatasetDialog
        ws={ws}
        dataset={confirmDelete ? ds : null}
        onOpenChange={setConfirmDelete}
        toastOnSuccess={false}
        onDeleted={() => nav(listHref)}
      />
    </PageContainer>
  );
}
