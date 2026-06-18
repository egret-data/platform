import { useMemo, useState } from "react";
import { useParams } from "react-router-dom";
import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  csvUrl, fetchResults, useCancelQuery, useDatabases, useHistory, useQueryStatus,
  useSaveQuery, useSavedQueries, useDeleteSavedQuery, useSubmitQuery,
  type ResultsPage,
} from "@/api/sqllab";
import { SqlEditor, type SchemaHint } from "@/components/sqllab/SqlEditor";
import { ResultsGrid } from "@/components/sqllab/ResultsGrid";
import { SchemaTree } from "@/components/sqllab/SchemaTree";

export default function SqlLab() {
  const ws = useParams().workspace!;
  const { data: databases } = useDatabases(ws);
  const [dbId, setDbId] = useState<string | null>(null);
  const db = databases?.find((d) => d.id === dbId) ?? null;
  const [sql, setSql] = useState("SELECT 1");
  const [queryId, setQueryId] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [results, setResults] = useState<ResultsPage | null>(null);
  const PAGE = 100;

  const submit = useSubmitQuery(ws);
  const cancel = useCancelQuery(ws);
  const status = useQueryStatus(ws, queryId);
  const save = useSaveQuery(ws);

  const finished = status.data?.status === "finished";
  if (finished && queryId && results === null) {
    fetchResults(ws, queryId, 0, PAGE).then(setResults).catch(() => {});
  }

  async function run() {
    if (!dbId) { toast.error("Pick a database"); return; }
    setResults(null); setPage(0);
    try { const { id } = await submit.mutateAsync({ database_id: dbId, sql }); setQueryId(id); }
    catch (e) { toast.error(String(e)); }
  }
  async function goPage(n: number) {
    if (!queryId) return;
    setPage(n); setResults(await fetchResults(ws, queryId, n, PAGE));
  }
  async function onSave() {
    const name = window.prompt("Query name?"); if (!name) return;
    await save.mutateAsync({ database_id: dbId, name, sql }); toast.success("Saved");
  }

  const running = status.data?.status === "queued" || status.data?.status === "running";
  const hint: SchemaHint = useMemo(() => ({ tables: [], columnsByTable: {} }), []);

  return (
    <PanelGroup direction="horizontal" className="h-full">
      <Panel defaultSize={20} minSize={12}>
        <div className="flex h-full flex-col border-r">
          <div className="border-b p-2">
            <Select value={dbId ?? ""} onValueChange={setDbId}>
              <SelectTrigger><SelectValue placeholder="Database" /></SelectTrigger>
              <SelectContent>{(databases ?? []).map((d) => <SelectItem key={d.id} value={d.id}>{d.name} ({d.engine})</SelectItem>)}</SelectContent>
            </Select>
          </div>
          <SchemaTree ws={ws} db={dbId} engine={db?.engine ?? null} onInsert={(n) => setSql((s) => s + n)} />
        </div>
      </Panel>
      <PanelResizeHandle className="w-1 bg-border" />
      <Panel defaultSize={80}>
        <PanelGroup direction="vertical">
          <Panel defaultSize={50} minSize={20}>
            <div className="flex h-full flex-col">
              <div className="flex items-center gap-2 border-b p-2">
                <Button size="sm" onClick={run} disabled={running}>Run</Button>
                <Button size="sm" variant="outline" disabled={!running} onClick={() => queryId && cancel.mutate(queryId)}>Cancel</Button>
                <Button size="sm" variant="outline" onClick={onSave}>Save</Button>
                {running && <span className="text-sm text-muted-foreground">running…</span>}
                {status.data?.status === "failed" && <span className="text-sm text-red-600">{status.data.error}</span>}
              </div>
              <div className="flex-1"><SqlEditor value={sql} onChange={setSql} hint={hint} /></div>
            </div>
          </Panel>
          <PanelResizeHandle className="h-1 bg-border" />
          <Panel defaultSize={50} minSize={20}>
            <Tabs defaultValue="results" className="flex h-full flex-col">
              <TabsList className="m-2 w-fit"><TabsTrigger value="results">Results</TabsTrigger><TabsTrigger value="history">History</TabsTrigger><TabsTrigger value="saved">Saved</TabsTrigger></TabsList>
              <TabsContent value="results" className="flex-1 overflow-hidden">
                <ResultsGrid page={results} pageIndex={page} onPage={goPage} truncated={status.data?.truncated ?? false}
                  csvHref={finished && queryId ? csvUrl(ws, queryId) : null} />
              </TabsContent>
              <TabsContent value="history" className="flex-1 overflow-auto"><HistoryTab ws={ws} onLoad={setSql} /></TabsContent>
              <TabsContent value="saved" className="flex-1 overflow-auto"><SavedTab ws={ws} onLoad={(s, d) => { setSql(s); if (d) setDbId(d); }} /></TabsContent>
            </Tabs>
          </Panel>
        </PanelGroup>
      </Panel>
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

function SavedTab({ ws, onLoad }: { ws: string; onLoad: (sql: string, db: string | null) => void }) {
  const { data } = useSavedQueries(ws);
  const del = useDeleteSavedQuery(ws);
  return <div className="p-2 text-sm">{(data ?? []).map((q) => (
    <div key={q.id} className="flex items-center gap-2 rounded px-2 py-1 hover:bg-accent">
      <button className="flex-1 truncate text-left" onClick={() => onLoad(q.sql, q.database_id)} title={q.sql}>{q.name}</button>
      <button className="text-xs text-red-600" onClick={() => del.mutate(q.id)}>delete</button>
    </div>
  ))}</div>;
}
