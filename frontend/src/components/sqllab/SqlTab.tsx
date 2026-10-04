import { useEffect, useMemo, useRef, useState } from "react";
import { format as formatSql } from "sql-formatter";
// `Separator` is aliased: `@/components/ui/separator` already exports that name for the
// visual divider used in the toolbars below, and v4 renamed `PanelResizeHandle` into a
// collision with it.
import { Group, Panel, Separator as PanelSeparator } from "react-resizable-panels";
import { toast } from "sonner";
import {
  BarChart3, Ban, Check, Command, Database, Download, FileSearch, Keyboard, Loader2, MoreHorizontal, PanelLeftOpen,
  Play, Save as SaveIcon, Sparkles, Table2, WandSparkles,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import {
  csvUrl, useCancelQuery, useDatabases, useQueryStatus, useResults,
  useSaveQuery, useSubmitQuery,
} from "@/api/sqllab";
import { SqlEditor } from "@/components/sqllab/SqlEditor";
import { ResultsGrid } from "@/components/sqllab/ResultsGrid";
import { ResultChart } from "@/components/sqllab/ResultChart";
import { DataPanel } from "@/components/sqllab/DataPanel";
import { SaveQueryDialog } from "@/components/sqllab/SaveQueryDialog";
import { HistoryTab } from "@/components/sqllab/HistoryTab";
import { SavedTab } from "@/components/sqllab/SavedTab";
import { SnippetsTab } from "@/components/sqllab/SnippetsTab";
import { CommandPalette, type PaletteAction } from "@/components/sqllab/CommandPalette";
import { CreateDatasetDialog } from "@/components/dataset/CreateDatasetDialog";
import { emptySchemaIndex, type SchemaIndex } from "@/components/sqllab/schemaIndex";
import type { SqlTabState } from "@/hooks/useTabs";

const LIMITS = [10, 100, 1000, 10000, 100000];
const MAX_LIMIT = 100000;
const CUSTOM = "__custom__";
const CUSTOM_CURRENT = "__custom_current__";

function fmtElapsed(ms: number): string {
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(1)} s`;
}

const PARAM_RE = /\{\{\s*([a-zA-Z_][a-zA-Z0-9_]*)\s*\}\}/g;
/** Distinct `{{name}}` template parameters referenced in `sql`, in first-seen order. */
function extractParams(sql: string): string[] {
  const seen = new Set<string>();
  for (const m of sql.matchAll(PARAM_RE)) seen.add(m[1]);
  return [...seen];
}

export function SqlTab({ ws, tab, patch, onOpenSaved, onOpenPreview }: {
  ws: string; tab: SqlTabState; patch: (p: Partial<SqlTabState>) => void;
  onOpenSaved: (sql: string, dbId: string | null) => void;
  onOpenPreview?: (init: { title: string; sql: string; dbId: string | null }) => void;
}) {
  const { data: databases } = useDatabases(ws);
  const db = databases?.find((d) => d.id === tab.dbId) ?? null;
  const engine = db?.engine ?? null;
  const [queryId, setQueryId] = useState<string | null>(null);
  const [words, setWords] = useState<string[]>([]);
  const [schema, setSchema] = useState<SchemaIndex>(emptySchemaIndex);
  const [saveOpen, setSaveOpen] = useState(false);
  const [datasetOpen, setDatasetOpen] = useState(false);
  const [panelCollapsed, setPanelCollapsed] = useState(false);
  const [selectedSql, setSelectedSql] = useState("");
  const [tabValue, setTabValue] = useState("results");
  const [customLimit, setCustomLimit] = useState(false);
  const [resultView, setResultView] = useState<"table" | "chart">("table");
  const [paramValues, setParamValues] = useState<Record<string, { type: string; value: string }>>({});
  const [elapsedMs, setElapsedMs] = useState<number | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const startedRef = useRef<number | null>(null);

  const submit = useSubmitQuery(ws);
  const cancel = useCancelQuery(ws);
  const status = useQueryStatus(ws, queryId);
  const save = useSaveQuery(ws);
  const finished = status.data?.status === "finished";
  const failed = status.data?.status === "failed";
  const running = status.data?.status === "queued" || status.data?.status === "running";
  const results = useResults(ws, queryId, finished);

  // Auto-run once for preview tabs (opened from the Explorer's Preview action).
  useEffect(() => {
    if (tab.autoRun && tab.dbId && tab.sql.trim()) { patch({ autoRun: false }); run(); }
  }, []);

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
    setQueryId(null); setElapsedMs(null); setTabValue("results"); setResultView("table");
    const params = extractParams(sql).map((n) => ({
      name: n, type: paramValues[n]?.type ?? "string", value: paramValues[n]?.value ?? "",
    }));
    try {
      const { id } = await submit.mutateAsync({ database_id: dbId, sql, limit: tab.limit, params });
      setQueryId(id); startedRef.current = performance.now();
    } catch (e) { toast.error(String(e)); }
  }
  function doFormat() {
    const language = engine === "starrocks" ? "mysql" : "trino";
    try { patch({ sql: formatSql(tab.sql, { language }) }); } catch { /* keep as-is */ }
  }
  function insert(name: string) { patch({ sql: tab.sql + name }); }
  // Show the engine's query plan without changing the editor: run EXPLAIN over
  // the current SQL (or selection) and render the plan in the Results grid.
  function explain() {
    const sql = hasSelection ? selectedSql : tab.sql;
    if (sql.trim()) run(`EXPLAIN ${sql}`);
  }
  function commitLimit(raw: string) {
    const n = Math.floor(Number(raw));
    if (Number.isFinite(n) && n > 0) patch({ limit: Math.min(n, MAX_LIMIT) });
    setCustomLimit(false);
  }

  const isPreset = LIMITS.includes(tab.limit);
  const hasSelection = selectedSql.trim().length > 0;
  const editorParams = useMemo(() => extractParams(tab.sql), [tab.sql]);
  const rowCount = finished ? status.data?.row_count ?? null : null;
  const statusLine =
    elapsedMs == null ? null
    : finished ? `Done in ${fmtElapsed(elapsedMs)}`
    : failed ? `Failed in ${fmtElapsed(elapsedMs)}`
    : null;
  const csvHref = finished && queryId ? csvUrl(ws, queryId) : null;

  // ⌘K command palette.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const actions = useMemo<PaletteAction[]>(() => {
    const list: PaletteAction[] = [
      { id: "run", label: hasSelection ? "Run selection" : "Run query", hint: "⌘↵", run: () => run(hasSelection ? selectedSql : undefined) },
      { id: "explain", label: "Explain query", run: explain },
      { id: "format", label: "Format SQL", run: doFormat },
      { id: "save", label: "Save query…", run: () => setSaveOpen(true) },
      { id: "dataset", label: "Save as dataset…", run: () => setDatasetOpen(true) },
      { id: "panel", label: panelCollapsed ? "Show explorer" : "Hide explorer", run: () => setPanelCollapsed((v) => !v) },
      { id: "results", label: "Go to Results", run: () => setTabValue("results") },
      { id: "history", label: "Go to History", run: () => setTabValue("history") },
      { id: "saved", label: "Go to Saved", run: () => setTabValue("saved") },
      { id: "shortcuts", label: "Keyboard shortcuts", run: () => setShortcutsOpen(true) },
    ];
    if (csvHref) list.push({ id: "csv", label: "Download results as CSV", run: () => { window.location.href = csvHref; } });
    return list;
  }, [hasSelection, selectedSql, panelCollapsed, csvHref, tab.sql, tab.dbId, tab.limit, engine]);

  const statusChip = running ? (
    <span className="inline-flex items-center gap-1.5 rounded-full border border-primary/30 bg-primary/10 px-2 py-0.5 text-xs font-medium text-primary">
      <Loader2 className="h-3 w-3 animate-spin" /> Running…
    </span>
  ) : statusLine ? (
    <span className={`inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-medium ${
      failed ? "border border-destructive/30 bg-destructive/10 text-destructive"
             : "border border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400"}`}>
      {failed ? <Ban className="h-3 w-3" /> : <Check className="h-3 w-3" />}
      {statusLine}{rowCount != null ? ` · ${rowCount.toLocaleString()} rows` : ""}
    </span>
  ) : null;

  const main = (
    <Group orientation="vertical">
      <Panel defaultSize="55%" minSize="20%">
        <div className="flex h-full flex-col">
          {/* Toolbar — grouped, wraps gracefully instead of clipping. */}
          <div className="flex flex-wrap items-center gap-1.5 border-b bg-muted/20 px-2 py-1.5">
            <Button size="sm" className="h-8 gap-1.5" title="Run (⌘/Ctrl+↵)"
              onClick={() => run(hasSelection ? selectedSql : undefined)} disabled={running}>
              {running ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Play className="h-3.5 w-3.5" />}
              {hasSelection ? "Run selection" : "Run"}
            </Button>
            <Button size="sm" variant="outline" className="h-8 gap-1.5" disabled={!running}
              onClick={() => queryId && cancel.mutate(queryId)}>
              <Ban className="h-3.5 w-3.5" /> Cancel
            </Button>
            <Button size="sm" variant="ghost" className="h-8 gap-1.5" title="Format SQL" onClick={doFormat}>
              <WandSparkles className="h-3.5 w-3.5" /> Format
            </Button>

            <Separator orientation="vertical" className="mx-0.5 h-5" />

            <span className="text-xs text-muted-foreground">Limit</span>
            {customLimit ? (
              <Input autoFocus type="number" min={1} className="h-8 w-24" defaultValue={tab.limit}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitLimit((e.target as HTMLInputElement).value);
                  else if (e.key === "Escape") setCustomLimit(false);
                }}
                onBlur={(e) => commitLimit(e.target.value)} />
            ) : (
              <Select value={isPreset ? String(tab.limit) : CUSTOM_CURRENT}
                onValueChange={(v) => { if (v === CUSTOM) setCustomLimit(true); else if (v !== CUSTOM_CURRENT) patch({ limit: Number(v) }); }}>
                <SelectTrigger className="h-8 w-32"><SelectValue /></SelectTrigger>
                <SelectContent>
                  {LIMITS.map((l) => <SelectItem key={l} value={String(l)}>{l.toLocaleString()} rows</SelectItem>)}
                  {!isPreset && <SelectItem value={CUSTOM_CURRENT}>{tab.limit.toLocaleString()} rows</SelectItem>}
                  <SelectItem value={CUSTOM}>Custom…</SelectItem>
                </SelectContent>
              </Select>
            )}

            <Separator orientation="vertical" className="mx-0.5 h-5" />

            <span className="inline-flex items-center gap-1.5 rounded-md border bg-background px-2 py-1 text-xs" title={db ? `${db.name} (${db.engine})` : "No database selected"}>
              <Database className={`h-3.5 w-3.5 ${db ? "text-primary" : "text-muted-foreground"}`} />
              <span className="max-w-[10rem] truncate">{db ? db.name : "No database"}</span>
              {engine && <span className="text-muted-foreground">· {engine}</span>}
            </span>
            {statusChip}

            <div className="ml-auto flex items-center gap-1.5">
              <Button size="sm" variant="outline" className="h-8 gap-1.5" onClick={() => setSaveOpen(true)}>
                <SaveIcon className="h-3.5 w-3.5" /> Save
              </Button>
              <Button size="sm" variant="outline" className="h-8 gap-1.5" onClick={() => setDatasetOpen(true)}>
                <Sparkles className="h-3.5 w-3.5" /> Save as dataset
              </Button>
              <Button size="sm" variant="ghost" className="h-8 gap-1.5 text-muted-foreground" title="Command palette (⌘K)"
                onClick={() => setPaletteOpen(true)}>
                <Command className="h-3.5 w-3.5" /> <span className="hidden sm:inline">⌘K</span>
              </Button>
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button size="icon" variant="ghost" className="h-8 w-8" aria-label="More actions">
                    <MoreHorizontal className="h-4 w-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem onClick={explain}>
                    <FileSearch className="mr-2 h-4 w-4" /> Explain query
                  </DropdownMenuItem>
                  {csvHref && (
                    <DropdownMenuItem asChild>
                      <a href={csvHref} download><Download className="mr-2 h-4 w-4" /> Download CSV</a>
                    </DropdownMenuItem>
                  )}
                  <DropdownMenuItem onClick={() => setPanelCollapsed((v) => !v)}>
                    <PanelLeftOpen className="mr-2 h-4 w-4" /> {panelCollapsed ? "Show explorer" : "Hide explorer"}
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem onClick={() => setPaletteOpen(true)}>
                    <Command className="mr-2 h-4 w-4" /> Command palette…
                  </DropdownMenuItem>
                  <DropdownMenuItem onClick={() => setShortcutsOpen(true)}>
                    <Keyboard className="mr-2 h-4 w-4" /> Keyboard shortcuts
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </div>
          {editorParams.length > 0 && (
            <div className="flex flex-wrap items-center gap-2 border-b bg-muted/10 px-2 py-1.5 text-xs">
              <span className="font-medium text-muted-foreground">Parameters</span>
              {editorParams.map((n) => {
                const pv = paramValues[n] ?? { type: "string", value: "" };
                const set = (patchPv: Partial<{ type: string; value: string }>) =>
                  setParamValues((v) => ({ ...v, [n]: { ...pv, ...patchPv } }));
                return (
                  <span key={n} className="inline-flex items-center gap-1 rounded-md border bg-background pl-2 text-xs">
                    <span className="font-mono">{n}</span>
                    <select value={pv.type} onChange={(e) => set({ type: e.target.value })}
                      aria-label={`Type of ${n}`} className="border-l bg-transparent px-1 py-1 outline-none">
                      <option value="string">str</option>
                      <option value="number">num</option>
                      <option value="boolean">bool</option>
                    </select>
                    <input value={pv.value} onChange={(e) => set({ value: e.target.value })}
                      aria-label={`Value of ${n}`} placeholder="value"
                      className="w-24 rounded-r-md border-l bg-transparent px-1.5 py-1 outline-none focus-visible:ring-2 focus-visible:ring-ring" />
                  </span>
                );
              })}
            </div>
          )}
          <div className="min-h-0 flex-1"><SqlEditor value={tab.sql} onChange={(v) => patch({ sql: v })} schemaWords={words} schema={schema}
            onRun={(sql) => run(sql)} onSelectionChange={setSelectedSql} /></div>
        </div>
      </Panel>
      <PanelSeparator className="h-1.5 bg-border transition-colors hover:bg-primary/60 active:bg-primary" />
      <Panel defaultSize="45%" minSize="20%">
        <Tabs value={tabValue} onValueChange={setTabValue} className="flex h-full flex-col">
          <div className="flex items-center gap-2 border-b px-2 py-1.5">
            <TabsList className="h-8"><TabsTrigger value="results">Results</TabsTrigger><TabsTrigger value="history">History</TabsTrigger><TabsTrigger value="saved">Saved</TabsTrigger><TabsTrigger value="snippets">Snippets</TabsTrigger></TabsList>
            {tabValue === "results" && (
              <div className="ml-auto flex items-center gap-2">
                {finished && results.data && (
                  <div className="flex rounded-md border p-0.5">
                    <button onClick={() => setResultView("table")} title="Table"
                      className={`flex h-6 items-center gap-1 rounded px-2 text-xs ${resultView === "table" ? "bg-accent font-medium text-foreground" : "text-muted-foreground hover:text-foreground"}`}>
                      <Table2 className="h-3.5 w-3.5" /> Table
                    </button>
                    <button onClick={() => setResultView("chart")} title="Chart"
                      className={`flex h-6 items-center gap-1 rounded px-2 text-xs ${resultView === "chart" ? "bg-accent font-medium text-foreground" : "text-muted-foreground hover:text-foreground"}`}>
                      <BarChart3 className="h-3.5 w-3.5" /> Chart
                    </button>
                  </div>
                )}
                {statusChip}
              </div>
            )}
          </div>
          <TabsContent value="results" className="min-h-0 flex-1 overflow-hidden">
            {resultView === "chart" && finished && results.data ? (
              <ResultChart page={results.data} />
            ) : (
              <ResultsGrid page={results.data ?? null} truncated={status.data?.truncated ?? false}
                error={failed ? (status.data?.error ?? "Query failed") : null} statusLine={statusLine} loading={running}
                csvHref={csvHref} />
            )}
          </TabsContent>
          <TabsContent value="history" className="min-h-0 flex-1 overflow-auto">
            <HistoryTab ws={ws} databases={databases} onLoad={(s) => patch({ sql: s })} onRun={(sql, dbId) => { patch({ dbId, sql }); run(sql, dbId); }} />
          </TabsContent>
          <TabsContent value="saved" className="min-h-0 flex-1 overflow-auto">
            <SavedTab ws={ws} databases={databases} onOpen={onOpenSaved} />
          </TabsContent>
          <TabsContent value="snippets" className="min-h-0 flex-1 overflow-hidden">
            <SnippetsTab ws={ws} currentSql={tab.sql}
              onInsert={(body) => patch({ sql: tab.sql.trim() ? `${tab.sql}\n${body}` : body })} />
          </TabsContent>
        </Tabs>
      </Panel>
    </Group>
  );

  const dialogs = (
    <>
      <SaveQueryDialog open={saveOpen} onOpenChange={setSaveOpen}
        onSave={async (name, description) => { await save.mutateAsync({ database_id: tab.dbId, name, sql: tab.sql, description }); toast.success("Saved"); }} />
      <CreateDatasetDialog ws={ws} open={datasetOpen} onOpenChange={setDatasetOpen}
        initialSql={tab.sql} initialDbId={tab.dbId} />
      <CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} actions={actions} />
      <Dialog open={shortcutsOpen} onOpenChange={setShortcutsOpen}>
        <DialogContent className="max-w-sm">
          <DialogHeader><DialogTitle>Keyboard shortcuts</DialogTitle></DialogHeader>
          <div className="space-y-1.5 text-sm">
            {[
              ["Run query / selection", "⌘ / Ctrl + ↵"],
              ["Command palette", "⌘ / Ctrl + K"],
              ["Autocomplete", "⌃ Space"],
              ["Rename tab", "Double-click tab"],
              ["Insert object", "Click in Explorer"],
            ].map(([label, keys]) => (
              <div key={label} className="flex items-center justify-between gap-4">
                <span className="text-muted-foreground">{label}</span>
                <kbd className="rounded border bg-muted px-1.5 py-0.5 font-mono text-xs">{keys}</kbd>
              </div>
            ))}
          </div>
        </DialogContent>
      </Dialog>
    </>
  );

  if (panelCollapsed) {
    return (
      <div className="flex h-full">
        <div className="flex h-full flex-col border-r bg-muted/20">
          <button onClick={() => setPanelCollapsed(false)} title="Show explorer" aria-label="Show explorer"
            className="p-2 text-muted-foreground hover:text-foreground">
            <PanelLeftOpen className="h-4 w-4" />
          </button>
        </div>
        <div className="min-w-0 flex-1">{main}</div>
        {dialogs}
      </div>
    );
  }

  return (
    <Group orientation="horizontal" className="h-full">
      <Panel defaultSize="22%" minSize="12%" maxSize="40%">
        <DataPanel ws={ws} dbId={tab.dbId} onDb={(id) => patch({ dbId: id })} onInsert={insert} onWords={setWords} onSchema={setSchema}
          onPreview={(fqn) => onOpenPreview?.({ title: fqn.split(".").pop() ?? fqn, sql: `SELECT * FROM ${fqn} LIMIT 100`, dbId: tab.dbId })}
          onCollapse={() => setPanelCollapsed(true)} />
      </Panel>
      <PanelSeparator className="w-1.5 bg-border transition-colors hover:bg-primary/60 active:bg-primary" />
      <Panel defaultSize="78%">{main}</Panel>
      {dialogs}
    </Group>
  );
}
