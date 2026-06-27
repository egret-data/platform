import { useState } from "react";
import { useParams } from "react-router-dom";
import { Boxes, Database, Eye, Table2 } from "lucide-react";
import {
  nsDotted,
  useCatalogNamespaces,
  useCatalogSummary,
  useNamespaceContents,
  useTableDetail,
} from "@/api/catalog";
import { ListSkeleton } from "@/components/explore/ListSkeleton";

export default function CatalogBrowse() {
  const ws = useParams().workspace!;
  const summary = useCatalogSummary(ws);

  const hasCatalog = !!summary.data;
  const namespaces = useCatalogNamespaces(ws, hasCatalog);

  const [ns, setNs] = useState<string | null>(null);
  const [table, setTable] = useState<string | null>(null);

  const contents = useNamespaceContents(ws, ns);
  const detail = useTableDetail(ws, ns, table);

  function selectNamespace(value: string) {
    setNs(value);
    setTable(null);
  }

  if (summary.isLoading) {
    return (
      <div className="p-4">
        <ListSkeleton cols={3} />
      </div>
    );
  }

  if (summary.isError) {
    return (
      <div className="p-4">
        <p className="text-destructive">Failed to load catalog.</p>
      </div>
    );
  }

  if (!summary.data) {
    return (
      <div className="p-4">
        <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
          <Boxes className="mx-auto mb-3 h-8 w-8 opacity-60" />
          <p>No catalog provisioned for this workspace yet.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="p-4">
      <div className="mb-4 flex items-baseline justify-between">
        <h2 className="text-base font-semibold">{summary.data.name}</h2>
        <span className="font-mono text-xs text-muted-foreground">{summary.data.warehouse_location}</span>
      </div>

      <div className="grid grid-cols-[16rem_1fr] gap-4">
        {/* Left: namespaces */}
        <div className="rounded border border-border">
          <div className="border-b border-border px-3 py-2 text-xs font-medium uppercase tracking-wider text-muted-foreground">
            Namespaces
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
                  <li key={dotted}>
                    <button
                      type="button"
                      onClick={() => selectNamespace(dotted)}
                      className={`flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-sm transition-colors ${
                        active
                          ? "bg-accent text-accent-foreground"
                          : "text-muted-foreground hover:bg-accent/50 hover:text-foreground"
                      }`}
                    >
                      <Database className="h-4 w-4 shrink-0" />
                      <span className="truncate font-mono">{dotted}</span>
                    </button>
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
    </div>
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
    <div className="space-y-4">
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
          <div className="flex flex-wrap gap-x-6 gap-y-1 text-sm text-muted-foreground">
            <span className="font-mono break-all">{detail.data.location}</span>
          </div>
          <div className="flex flex-wrap gap-x-6 gap-y-1 text-sm">
            <span>
              <span className="text-muted-foreground">Schema id:</span> {detail.data.current_schema_id}
            </span>
            <span>
              <span className="text-muted-foreground">Current snapshot:</span>{" "}
              {detail.data.current_snapshot_id ?? "—"}
            </span>
            <span>
              <span className="text-muted-foreground">Snapshots:</span> {detail.data.snapshot_count}
            </span>
          </div>

          <div>
            <h2 className="mb-2 text-sm font-medium text-muted-foreground">Schema</h2>
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr className="border-b border-border">
                  <th className="py-1 pr-4 font-medium">Column</th>
                  <th className="py-1 pr-4 font-medium">Type</th>
                  <th className="py-1 font-medium">Nullable</th>
                </tr>
              </thead>
              <tbody>
                {detail.data.columns.map((c) => (
                  <tr key={c.name} className="border-b border-border last:border-b-0">
                    <td className="py-1 pr-4 font-mono">{c.name}</td>
                    <td className="py-1 pr-4 font-mono text-muted-foreground">{c.type}</td>
                    <td className="py-1">{c.required ? "no" : "yes"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {Object.keys(detail.data.properties).length > 0 && (
            <div>
              <h2 className="mb-2 text-sm font-medium text-muted-foreground">Properties</h2>
              <table className="w-full text-sm">
                <tbody>
                  {Object.entries(detail.data.properties).map(([k, v]) => (
                    <tr key={k} className="border-b border-border last:border-b-0">
                      <td className="py-1 pr-4 font-mono text-muted-foreground">{k}</td>
                      <td className="py-1 font-mono break-all">{v}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      ) : null}
    </div>
  );
}
