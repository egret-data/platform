import { Database } from "lucide-react";
import { useNotebookRuntime } from "@/api/notebooks";

export function DataSourcesPanel({ ws }: { ws: string }) {
  const runtime = useNotebookRuntime(ws);
  const engines = (runtime.data?.engines ?? []).filter((e) => e.enabled);

  if (engines.length === 0) {
    return (
      <p className="px-3 py-2 text-xs text-muted-foreground">
        No data sources enabled for notebooks. Enable engines on the Notebook
        Settings page.
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-1 py-1">
      {engines.map((e) => (
        <div key={e.id} className="flex items-center gap-2 px-3 py-1.5 text-xs">
          <Database className="h-3.5 w-3.5 text-muted-foreground" />
          <span className="truncate font-medium text-foreground" title={e.name}>
            {e.name}
          </span>
          <span className="ml-auto rounded bg-muted px-1.5 py-0.5 text-[10px] uppercase text-muted-foreground">
            {e.engine}
          </span>
        </div>
      ))}
      <p className="mt-1 border-t px-3 pt-2 text-[11px] leading-relaxed text-muted-foreground">
        Query as a Daft DataFrame:
        <code className="mt-1 block rounded bg-muted px-1.5 py-1 font-mono">
          df = trino("SELECT …")
        </code>
      </p>
    </div>
  );
}
