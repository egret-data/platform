import { useState } from "react";
import { format as formatSql } from "sql-formatter";
import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";
import { toast } from "sonner";
import { PanelLeftOpen } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  csvUrl, useCancelQuery, useDatabases, useDeleteSavedQuery, useHistory, useQueryStatus, useResults,
  useSaveQuery, useSavedQueries, useSubmitQuery,
} from "@/api/sqllab";
import { SqlEditor } from "@/components/sqllab/SqlEditor";
import { ResultsGrid } from "@/components/sqllab/ResultsGrid";
import { DataPanel } from "@/components/sqllab/DataPanel";
import { SaveQueryDialog } from "@/components/sqllab/SaveQueryDialog";
import type { SqlTabState } from "@/hooks/useTabs";

const LIMITS = [10, 100, 1000, 10000, 100000];

export function SqlTab({ ws, tab, patch, onOpenSaved }: {
  ws: string; tab: SqlTabState; patch: (p: Partial<SqlTabState>) => void; onOpenSaved: (sql: string, dbId: string | null) => void;
}) {
  const { data: databases } = useDatabases(ws);
  const engine = databases?.find((d) => d.id === tab.dbId)?.engine ?? null;
  const [queryId, setQueryId] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [words, setWords] = useState<string[]>([]);
  const [saveOpen, setSaveOpen] = useState(false);
  const [panelCollapsed, setPanelCollapsed] = useState(false);

  const submit = useSubmitQuery(ws);
  const cancel = useCancelQuery(ws);
  const status = useQueryStatus(ws, queryId);
  const save = useSaveQuery(ws);
  const finished = status.data?.status === "finished";
  const running = status.data?.status === "queued" || status.data?.status === "running";
  const results = useResults(ws, queryId, page, finished);

  async function run() {
    if (!tab.dbId) { toast.error("Pick a database"); return; }
    setQueryId(null); setPage(0);
    try { const { id } = await submit.mutateAsync({ database_id: tab.dbId, sql: tab.sql, limit: tab.limit }); setQueryId(id); }
    catch (e) { toast.error(String(e)); }
  }
  function doFormat() {
    const language = engine === "starrocks" ? "mysql" : "trino";
    try { patch({ sql: formatSql(tab.sql, { language }) }); } catch { /* keep as-is */ }
  }
  function insert(name: string) { patch({ sql: tab.sql + name }); }

  const main = (
    <PanelGroup direction="vertical">
      <Panel defaultSize={50} minSize={20}>
        <div className="flex h-full flex-col">
          <div className="flex items-center gap-2 border-b p-2">
            <Button size="sm" onClick={run} disabled={running}>Run</Button>
            <Button size="sm" variant="outline" disabled={!running} onClick={() => queryId && cancel.mutate(queryId)}>Cancel</Button>
            <Button size="sm" variant="outline" onClick={doFormat}>Format</Button>
            <Select value={String(tab.limit)} onValueChange={(v) => patch({ limit: Number(v) })}>
              <SelectTrigger className="h-8 w-28"><SelectValue /></SelectTrigger>
              <SelectContent>{LIMITS.map((l) => <SelectItem key={l} value={String(l)}>{l.toLocaleString()} rows</SelectItem>)}</SelectContent>
            </Select>
            <Button size="sm" variant="outline" onClick={() => setSaveOpen(true)}>Save</Button>
            {running && <span className="text-sm text-muted-foreground">running…</span>}
            {status.data?.status === "failed" && <span className="text-sm text-red-600">{status.data.error}</span>}
          </div>
          <div className="flex-1"><SqlEditor value={tab.sql} onChange={(v) => patch({ sql: v })} schemaWords={words} /></div>
        </div>
      </Panel>
      <PanelResizeHandle className="h-1 bg-border" />
      <Panel defaultSize={50} minSize={20}>
        <Tabs defaultValue="results" className="flex h-full flex-col">
          <TabsList className="m-2 w-fit"><TabsTrigger value="results">Results</TabsTrigger><TabsTrigger value="history">History</TabsTrigger><TabsTrigger value="saved">Saved</TabsTrigger></TabsList>
          <TabsContent value="results" className="flex-1 overflow-hidden">
            <ResultsGrid page={results.data ?? null} pageIndex={page} onPage={setPage} truncated={status.data?.truncated ?? false}
              csvHref={finished && queryId ? csvUrl(ws, queryId) : null} />
          </TabsContent>
          <TabsContent value="history" className="flex-1 overflow-auto"><HistoryTab ws={ws} onLoad={(s) => patch({ sql: s })} /></TabsContent>
          <TabsContent value="saved" className="flex-1 overflow-auto"><SavedTab ws={ws} onOpen={onOpenSaved} /></TabsContent>
        </Tabs>
      </Panel>
    </PanelGroup>
  );

  const saveDialog = (
    <SaveQueryDialog open={saveOpen} onOpenChange={setSaveOpen}
      onSave={async (name, description) => { await save.mutateAsync({ database_id: tab.dbId, name, sql: tab.sql, description }); toast.success("Saved"); }} />
  );

  if (panelCollapsed) {
    return (
      <div className="flex h-full">
        <button onClick={() => setPanelCollapsed(false)} title="Show data panel" className="self-start border-r p-2 text-muted-foreground hover:text-foreground">
          <PanelLeftOpen className="h-4 w-4" />
        </button>
        <div className="min-w-0 flex-1">{main}</div>
        {saveDialog}
      </div>
    );
  }

  return (
    <PanelGroup direction="horizontal" className="h-full">
      <Panel defaultSize={22} minSize={12}>
        <DataPanel ws={ws} dbId={tab.dbId} onDb={(id) => patch({ dbId: id })} onInsert={insert} onWords={setWords} onCollapse={() => setPanelCollapsed(true)} />
      </Panel>
      <PanelResizeHandle className="w-1 bg-border" />
      <Panel defaultSize={78}>{main}</Panel>
      {saveDialog}
    </PanelGroup>
  );
}

function HistoryTab({ ws, onLoad }: { ws: string; onLoad: (sql: string) => void }) {
  const { data } = useHistory(ws);
  return <div className="p-2 text-sm">{(data ?? []).map((h) => (
    <button key={h.id} className="block w-full truncate rounded px-2 py-1 text-left hover:bg-accent" onClick={() => onLoad(h.sql)} title={h.sql}>
      <span className="text-muted-foreground">[{h.status}]</span> {h.sql}
    </button>
  ))}</div>;
}
function SavedTab({ ws, onOpen }: { ws: string; onOpen: (sql: string, dbId: string | null) => void }) {
  const { data } = useSavedQueries(ws);
  const del = useDeleteSavedQuery(ws);
  return <div className="p-2 text-sm">{(data ?? []).map((q) => (
    <div key={q.id} className="flex items-center gap-2 rounded px-2 py-1 hover:bg-accent">
      <button className="flex-1 truncate text-left" onClick={() => onOpen(q.sql, q.database_id)} title={q.description || q.sql}>{q.name}</button>
      <button className="text-xs text-red-600" onClick={() => del.mutate(q.id)}>delete</button>
    </div>
  ))}</div>;
}
