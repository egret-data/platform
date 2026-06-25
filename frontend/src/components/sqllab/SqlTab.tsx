import { useEffect, useRef, useState } from "react";
import { format as formatSql } from "sql-formatter";
import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";
import { toast } from "sonner";
import { PanelLeftOpen } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  csvUrl, useCancelQuery, useDatabases, useQueryStatus, useResults,
  useSaveQuery, useSubmitQuery,
} from "@/api/sqllab";
import { SqlEditor } from "@/components/sqllab/SqlEditor";
import { ResultsGrid } from "@/components/sqllab/ResultsGrid";
import { DataPanel } from "@/components/sqllab/DataPanel";
import { SaveQueryDialog } from "@/components/sqllab/SaveQueryDialog";
import { HistoryTab } from "@/components/sqllab/HistoryTab";
import { SavedTab } from "@/components/sqllab/SavedTab";
import { CreateDatasetDialog } from "@/components/dataset/CreateDatasetDialog";
import type { SqlTabState } from "@/hooks/useTabs";

const LIMITS = [10, 100, 1000, 10000, 100000];
const MAX_LIMIT = 100000;
const CUSTOM = "__custom__";
const CUSTOM_CURRENT = "__custom_current__";

function fmtElapsed(ms: number): string {
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(1)} s`;
}

export function SqlTab({ ws, tab, patch, onOpenSaved }: {
  ws: string; tab: SqlTabState; patch: (p: Partial<SqlTabState>) => void; onOpenSaved: (sql: string, dbId: string | null) => void;
}) {
  const { data: databases } = useDatabases(ws);
  const engine = databases?.find((d) => d.id === tab.dbId)?.engine ?? null;
  const [queryId, setQueryId] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [words, setWords] = useState<string[]>([]);
  const [saveOpen, setSaveOpen] = useState(false);
  const [datasetOpen, setDatasetOpen] = useState(false);
  const [panelCollapsed, setPanelCollapsed] = useState(false);
  const [selectedSql, setSelectedSql] = useState("");
  const [tabValue, setTabValue] = useState("results");
  const [customLimit, setCustomLimit] = useState(false);
  const [elapsedMs, setElapsedMs] = useState<number | null>(null);
  const startedRef = useRef<number | null>(null);

  const submit = useSubmitQuery(ws);
  const cancel = useCancelQuery(ws);
  const status = useQueryStatus(ws, queryId);
  const save = useSaveQuery(ws);
  const finished = status.data?.status === "finished";
  const failed = status.data?.status === "failed";
  const running = status.data?.status === "queued" || status.data?.status === "running";
  const results = useResults(ws, queryId, page, finished);

  // Freeze a client-measured elapsed time once the query reaches a terminal state.
  useEffect(() => {
    if ((finished || failed) && startedRef.current != null && elapsedMs == null) {
      setElapsedMs(performance.now() - startedRef.current);
      startedRef.current = null;
    }
  }, [finished, failed, elapsedMs]);

  async function run(sqlOverride?: string, dbOverride?: string | null) {
    if (running) return;
    const dbId = dbOverride ?? tab.dbId;
    if (!dbId) { toast.error("Pick a database"); return; }
    const sql = sqlOverride ?? tab.sql;
    setQueryId(null); setPage(0); setElapsedMs(null); setTabValue("results");
    try {
      const { id } = await submit.mutateAsync({ database_id: dbId, sql, limit: tab.limit });
      setQueryId(id); startedRef.current = performance.now();
    } catch (e) { toast.error(String(e)); }
  }
  function doFormat() {
    const language = engine === "starrocks" ? "mysql" : "trino";
    try { patch({ sql: formatSql(tab.sql, { language }) }); } catch { /* keep as-is */ }
  }
  function insert(name: string) { patch({ sql: tab.sql + name }); }
  function commitLimit(raw: string) {
    const n = Math.floor(Number(raw));
    if (Number.isFinite(n) && n > 0) patch({ limit: Math.min(n, MAX_LIMIT) });
    setCustomLimit(false);
  }

  const isPreset = LIMITS.includes(tab.limit);
  const statusLine =
    elapsedMs == null ? null
    : finished ? `Done in ${fmtElapsed(elapsedMs)}`
    : failed ? `Failed in ${fmtElapsed(elapsedMs)}`
    : null;

  const main = (
    <PanelGroup direction="vertical">
      <Panel defaultSize={50} minSize={20}>
        <div className="flex h-full flex-col">
          <div className="flex items-center gap-2 border-b p-2">
            <Button size="sm" title="⌘/Ctrl+↵" onClick={() => run(selectedSql.trim() ? selectedSql : undefined)} disabled={running}>
              {selectedSql.trim() ? "Run selection" : "Run"}
            </Button>
            <Button size="sm" variant="outline" disabled={!running} onClick={() => queryId && cancel.mutate(queryId)}>Cancel</Button>
            <Button size="sm" variant="outline" onClick={doFormat}>Format</Button>
            <span className="text-sm text-muted-foreground">Limit</span>
            {customLimit ? (
              <Input
                autoFocus type="number" min={1} className="h-8 w-24" defaultValue={tab.limit}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitLimit((e.target as HTMLInputElement).value);
                  else if (e.key === "Escape") setCustomLimit(false);
                }}
                onBlur={(e) => commitLimit(e.target.value)}
              />
            ) : (
              <Select
                value={isPreset ? String(tab.limit) : CUSTOM_CURRENT}
                onValueChange={(v) => { if (v === CUSTOM) setCustomLimit(true); else if (v !== CUSTOM_CURRENT) patch({ limit: Number(v) }); }}
              >
                <SelectTrigger className="h-8 w-36"><SelectValue /></SelectTrigger>
                <SelectContent>
                  {LIMITS.map((l) => <SelectItem key={l} value={String(l)}>{l.toLocaleString()} rows</SelectItem>)}
                  {!isPreset && <SelectItem value={CUSTOM_CURRENT}>{tab.limit.toLocaleString()} rows</SelectItem>}
                  <SelectItem value={CUSTOM}>Custom…</SelectItem>
                </SelectContent>
              </Select>
            )}
            <Button size="sm" variant="outline" onClick={() => setSaveOpen(true)}>Save</Button>
            <Button size="sm" variant="outline" onClick={() => setDatasetOpen(true)}>Save as dataset</Button>
            {running && <span className="text-sm text-muted-foreground">running…</span>}
          </div>
          <div className="flex-1"><SqlEditor value={tab.sql} onChange={(v) => patch({ sql: v })} schemaWords={words}
            onRun={(sql) => run(sql)} onSelectionChange={setSelectedSql} /></div>
        </div>
      </Panel>
      <PanelResizeHandle className="h-1.5 bg-border transition-colors hover:bg-primary/60 data-[resize-handle-state=drag]:bg-primary" />
      <Panel defaultSize={50} minSize={20}>
        <Tabs value={tabValue} onValueChange={setTabValue} className="flex h-full flex-col">
          <TabsList className="m-2 w-fit"><TabsTrigger value="results">Results</TabsTrigger><TabsTrigger value="history">History</TabsTrigger><TabsTrigger value="saved">Saved</TabsTrigger></TabsList>
          <TabsContent value="results" className="flex-1 overflow-hidden">
            <ResultsGrid page={results.data ?? null} pageIndex={page} onPage={setPage} truncated={status.data?.truncated ?? false}
              error={failed ? (status.data?.error ?? "Query failed") : null} statusLine={statusLine}
              csvHref={finished && queryId ? csvUrl(ws, queryId) : null} />
          </TabsContent>
          <TabsContent value="history" className="flex-1 overflow-auto">
            <HistoryTab ws={ws} databases={databases} onLoad={(s) => patch({ sql: s })} onRun={(sql, dbId) => { patch({ dbId, sql }); run(sql, dbId); }} />
          </TabsContent>
          <TabsContent value="saved" className="flex-1 overflow-auto">
            <SavedTab ws={ws} databases={databases} onOpen={onOpenSaved} />
          </TabsContent>
        </Tabs>
      </Panel>
    </PanelGroup>
  );

  const saveDialog = (
    <SaveQueryDialog open={saveOpen} onOpenChange={setSaveOpen}
      onSave={async (name, description) => { await save.mutateAsync({ database_id: tab.dbId, name, sql: tab.sql, description }); toast.success("Saved"); }} />
  );

  const datasetDialog = (
    <CreateDatasetDialog
      ws={ws}
      open={datasetOpen}
      onOpenChange={setDatasetOpen}
      initialSql={tab.sql}
      initialDbId={tab.dbId}
      initialTab="sql"
    />
  );

  if (panelCollapsed) {
    return (
      <div className="flex h-full">
        <div className="flex h-full flex-col border-r">
          <button onClick={() => setPanelCollapsed(false)} title="Show data panel" className="p-2 text-muted-foreground hover:text-foreground">
            <PanelLeftOpen className="h-4 w-4" />
          </button>
        </div>
        <div className="min-w-0 flex-1">{main}</div>
        {saveDialog}
        {datasetDialog}
      </div>
    );
  }

  return (
    <PanelGroup direction="horizontal" className="h-full">
      <Panel defaultSize={22} minSize={12}>
        <DataPanel ws={ws} dbId={tab.dbId} onDb={(id) => patch({ dbId: id })} onInsert={insert} onWords={setWords} onCollapse={() => setPanelCollapsed(true)} />
      </Panel>
      <PanelResizeHandle className="w-1.5 bg-border transition-colors hover:bg-primary/60 data-[resize-handle-state=drag]:bg-primary" />
      <Panel defaultSize={78}>{main}</Panel>
      {saveDialog}
      {datasetDialog}
    </PanelGroup>
  );
}
