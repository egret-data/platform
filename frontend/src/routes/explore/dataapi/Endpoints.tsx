import { useEffect, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { Search } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";
import { groupByFolder, useCollapsed } from "@/components/explore/folders";
import { useFoldersFoldersList } from "@/api/generated/folders/folders";
import { useDatabases, type DatabaseConn } from "@/api/databases";
import {
  useDataApiCreateEndpoint,
  useDataApiDeleteEndpoint,
  useDataApiDisableEndpoint,
  useDataApiEnableEndpoint,
  useDataApiListEndpoints,
} from "@/api/generated/data-api/data-api";
import type { DataApiEndpointDto } from "@/api/generated/model";
import { DownloadOpenApiDialog } from "./DownloadOpenApiDialog";

// Folders share the generic router (egret-bi's `kind_perms`); this is the
// `{kind}` URL segment data-api registers there, matching
// `crates/egret-bi/src/routes.rs`'s `kind_perms("data_api_endpoints")`.
const FOLDER_KIND = "data_api_endpoints";

type SortKey = "name" | "state" | "updated_at";

// live is the only state that earns emphasis (--highlight is reserved for a
// page's single primary accent, not a state chip) — draft and disabled both
// read as muted/negative, never as something to celebrate.
const STATE_TONE: Record<string, string> = {
  live: "bg-success/15 text-success",
  draft: "bg-muted text-muted-foreground",
  disabled: "bg-destructive/15 text-destructive",
};

const ENGINE_LABEL: Record<string, string> = { trino: "Trino", starrocks: "StarRocks" };

function stateLabel(state: string): string {
  return state.length ? state.charAt(0).toUpperCase() + state.slice(1) : state;
}

export default function Endpoints() {
  const ws = useParams().workspace!;
  const nav = useNavigate();

  const { data: envelope, isLoading, isError } = useDataApiListEndpoints(ws);
  // useMemo (not a plain ternary) so a falsy envelope doesn't hand `rows`'
  // useMemo a fresh `[]` identity on every render.
  const endpoints = useMemo(() => (envelope?.status === 200 ? envelope.data : []), [envelope]);

  const { data: foldersEnvelope } = useFoldersFoldersList(ws, FOLDER_KIND);
  const folders = useMemo(() => (foldersEnvelope?.status === 200 ? foldersEnvelope.data : []), [foldersEnvelope]);

  const { data: databases } = useDatabases(ws);

  const createMutation = useDataApiCreateEndpoint();
  const deleteMutation = useDataApiDeleteEndpoint();
  const disableMutation = useDataApiDisableEndpoint();
  const enableMutation = useDataApiEnableEndpoint();

  const collapse = useCollapsed(`folders:${ws}:${FOLDER_KIND}`);

  const [query, setQuery] = useState("");
  const [stateFilter, setStateFilter] = useState<string | null>(null);
  const [sort, setSort] = useState<SortState<SortKey>>({ key: "updated_at", dir: "desc" });
  const [createOpen, setCreateOpen] = useState(false);
  const [downloadOpen, setDownloadOpen] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<DataApiEndpointDto | null>(null);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = endpoints.filter((e) => {
      if (stateFilter && e.state !== stateFilter) return false;
      if (!q) return true;
      return [e.name, e.path].join(" ").toLowerCase().includes(q);
    });
    r = [...r].sort((a, b) => {
      // No initializer: every branch below assigns, and eslint 10 flags the dead one.
      let cmp: number;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      else if (sort.key === "state") cmp = a.state.localeCompare(b.state);
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [endpoints, query, stateFilter, sort]);

  const filtering = query.trim() !== "" || stateFilter !== null;
  const groups = useMemo(() => groupByFolder(rows, folders, filtering), [rows, folders, filtering]);

  function onSort(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }
  function toggleStateFilter(s: string) {
    setStateFilter((f) => (f === s ? null : s));
  }

  const hasData = endpoints.length > 0;
  const live = endpoints.filter((e) => e.state === "live");
  const draft = endpoints.filter((e) => e.state === "draft");
  const disabled = endpoints.filter((e) => e.state === "disabled");

  function engineLabel(databaseId: string): string {
    const db = databases?.find((d) => d.id === databaseId);
    if (!db) return "—";
    return ENGINE_LABEL[db.engine] ?? db.engine;
  }

  const renderRow = (e: DataApiEndpointDto) => (
    <tr key={e.id} className="border-t hover:bg-accent">
      <td className="py-1 pl-6">
        <Link className="font-medium hover:underline" to={`/explore/${ws}/data-api/endpoints/${e.id}`}>
          {e.name}
        </Link>
      </td>
      <td className="text-muted-foreground">
        <span className="font-mono text-xs uppercase">{e.method}</span>{" "}
        <span className="font-mono text-xs">/{e.path.replace(/^\//, "")}</span>
      </td>
      <td>
        <Badge className={STATE_TONE[e.state] ?? "bg-muted text-muted-foreground"}>
          {stateLabel(e.state)}
        </Badge>
      </td>
      {/* The list DTO carries only `live_version_id` (a nullable id, not a
          version count) — the numeric `version` on `DataApiEndpointVersionDto`
          lives behind a per-endpoint versions fetch this page does not make.
          "Published" is the honest signal available here without an N+1. */}
      <td className="text-muted-foreground">{e.live_version_id ? "Published" : "—"}</td>
      <td className="text-muted-foreground">{engineLabel(e.database_id)}</td>
      <td className="text-muted-foreground"><RelativeTime iso={e.updated_at} className="block" /></td>
      <td>
        <RowActionsMenu>
          <DropdownMenuItem onClick={() => nav(`/explore/${ws}/data-api/endpoints/${e.id}`)}>Open</DropdownMenuItem>
          {e.state === "live" && (
            <DropdownMenuItem
              onClick={() =>
                disableMutation.mutateAsync({ workspace: ws, id: e.id })
                  .then(() => toast.success("Endpoint disabled"))
                  .catch((err) => toast.error(String(err)))
              }
            >
              Disable
            </DropdownMenuItem>
          )}
          {e.state === "disabled" && (
            <DropdownMenuItem
              onClick={() =>
                enableMutation.mutateAsync({ workspace: ws, id: e.id })
                  .then(() => toast.success("Endpoint enabled"))
                  .catch((err) => toast.error(String(err)))
              }
            >
              Enable
            </DropdownMenuItem>
          )}
          <DropdownMenuSeparator />
          <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={() => setPendingDelete(e)}>
            Delete
          </DropdownMenuItem>
        </RowActionsMenu>
      </td>
    </tr>
  );

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">Endpoints</h1>
        <div className="flex items-center gap-2">
          <Button variant="outline" onClick={() => setDownloadOpen(true)}>Download OpenAPI</Button>
          <Button onClick={() => setCreateOpen(true)}>+ Endpoint</Button>
        </div>
      </div>

      {isLoading ? (
        <ListSkeleton cols={6} />
      ) : isError ? (
        <Alert variant="destructive">
          <AlertDescription>Could not load endpoints. Try again in a moment.</AlertDescription>
        </Alert>
      ) : hasData ? (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input className="pl-8" placeholder="Search endpoints…" value={query} onChange={(e) => setQuery(e.target.value)} />
            </div>
          </div>

          <div className="mb-3">
            <SummaryStrip
              items={[
                { label: "endpoints", value: rows.length },
                { label: "live", value: live.length, onClick: () => toggleStateFilter("live"), active: stateFilter === "live" },
                { label: "draft", value: draft.length, onClick: () => toggleStateFilter("draft"), active: stateFilter === "draft" },
                { label: "disabled", value: disabled.length, onClick: () => toggleStateFilter("disabled"), active: stateFilter === "disabled" },
              ]}
            />
          </div>

          {rows.length > 0 ? (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead className="text-left text-muted-foreground">
                  <tr>
                    <SortableHeader sortKey="name" label="Name" current={sort} onSort={onSort} />
                    <th className="px-2 font-medium">Method &amp; path</th>
                    <SortableHeader sortKey="state" label="State" current={sort} onSort={onSort} />
                    <th className="px-2 font-medium">Version</th>
                    <th className="px-2 font-medium">Engine</th>
                    <SortableHeader sortKey="updated_at" label="Last edited" current={sort} onSort={onSort} />
                    <th className="w-8" />
                  </tr>
                </thead>
                {groups.map((group) => {
                  const key = group.folder?.id ?? "__uncat__";
                  const isCollapsed = collapse.isCollapsed(key);
                  return (
                    <tbody key={key}>
                      <FolderHeaderRow
                        name={group.folder?.name ?? "Uncategorized"}
                        count={group.rows.length}
                        colSpan={7}
                        collapsed={isCollapsed}
                        onToggle={() => collapse.toggle(key)}
                      />
                      {!isCollapsed && group.rows.map(renderRow)}
                    </tbody>
                  );
                })}
              </table>
            </div>
          ) : (
            <p className="text-muted-foreground">No endpoints match the current filters.</p>
          )}
        </>
      ) : (
        <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
          <p>No endpoints yet. Publish a query as a REST endpoint.</p>
          <Button className="mt-3" onClick={() => setCreateOpen(true)}>Create your first endpoint</Button>
        </div>
      )}

      <CreateEndpointDialog
        ws={ws}
        open={createOpen}
        onOpenChange={setCreateOpen}
        databases={databases ?? []}
        create={createMutation}
        onCreated={(id) => nav(`/explore/${ws}/data-api/endpoints/${id}`)}
      />

      <DownloadOpenApiDialog ws={ws} open={downloadOpen} onOpenChange={setDownloadOpen} />

      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => { if (!o) setPendingDelete(null); }}
        title="Delete endpoint"
        description={`Delete "${pendingDelete?.name ?? ""}"? Any deployed client calling its URL will start getting 404s. This cannot be undone.`}
        onConfirm={async () => {
          if (!pendingDelete) return;
          try {
            await deleteMutation.mutateAsync({ workspace: ws, id: pendingDelete.id });
            toast.success("Endpoint deleted");
          } catch (err) {
            toast.error(String(err));
          }
          setPendingDelete(null);
        }}
      />
    </div>
  );
}

/**
 * Per the Alerts lesson (#485): a create dialog collects only the fields a
 * row cannot exist without, then hands off to the real editor. Everything
 * else (description, params, output shape, cache, limits…) is edited on the
 * detail route — this dialog must never grow into a second editor.
 *
 * `sql` is here too, alongside name/path/method/database — `CreateEndpointInput`
 * requires it (no `#[serde(default)]`, unlike the config fields), so a row
 * cannot exist without it either. Kept to a single-line starting point; the
 * real query editor is the detail route's Definition section.
 */
function CreateEndpointDialog({ ws, open, onOpenChange, databases, create, onCreated }: {
  ws: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  databases: DatabaseConn[];
  create: ReturnType<typeof useDataApiCreateEndpoint>;
  onCreated: (id: string) => void;
}) {
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [method, setMethod] = useState("GET");
  const [databaseId, setDatabaseId] = useState("");
  const [sql, setSql] = useState("");

  useEffect(() => {
    if (open) {
      setName("");
      setPath("");
      setMethod("GET");
      setDatabaseId("");
      setSql("");
    }
  }, [open]);

  const canSubmit = name.trim() !== "" && path.trim() !== "" && databaseId !== "" && sql.trim() !== "";

  async function submit() {
    if (!canSubmit) return;
    try {
      const res = await create.mutateAsync({
        workspace: ws,
        data: { name: name.trim(), path: path.trim(), method, database_id: databaseId, sql: sql.trim() },
      });
      // The mutator throws on any non-2xx (see mutator.ts), so this branch is
      // the only one ever reached at runtime — the check just gives TS a
      // narrower type than the response union declares.
      if (res.status !== 201) { toast.error("Could not create endpoint."); return; }
      toast.success("Endpoint created");
      onOpenChange(false);
      onCreated(res.data.id);
    } catch (e) {
      toast.error(String(e));
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader><DialogTitle>New endpoint</DialogTitle></DialogHeader>
        <div className="space-y-3">
          <div className="space-y-1.5">
            <Label htmlFor="ep-name">Name</Label>
            <Input id="ep-name" autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="Daily report" />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="ep-path">Path</Label>
            <Input id="ep-path" value={path} onChange={(e) => setPath(e.target.value)} placeholder="reports/daily" />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="ep-method">Method</Label>
              <Select value={method} onValueChange={setMethod}>
                <SelectTrigger id="ep-method" aria-label="Method"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="GET">GET</SelectItem>
                  <SelectItem value="POST">POST</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="ep-db">Database</Label>
              <Select value={databaseId} onValueChange={setDatabaseId}>
                <SelectTrigger id="ep-db" aria-label="Database">
                  <SelectValue placeholder="Choose a database" />
                </SelectTrigger>
                <SelectContent>
                  {databases.map((d) => (
                    <SelectItem key={d.id} value={d.id}>{d.name}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="ep-sql">SQL</Label>
            <Textarea id="ep-sql" className="font-mono text-xs" rows={4} value={sql} onChange={(e) => setSql(e.target.value)} placeholder="SELECT …" />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={submit} disabled={!canSubmit || create.isPending}>
            {create.isPending ? "Creating…" : "Create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
