import { useState } from "react";
import { useParams } from "react-router-dom";
import { Plus, X } from "lucide-react";
import { useTabs } from "@/hooks/useTabs";
import { SqlTab } from "@/components/sqllab/SqlTab";

export default function SqlLab() {
  // Remount per workspace so the per-workspace tab state (localStorage) reloads
  // cleanly on a workspace switch (the route param changes without remounting).
  const ws = useParams().workspace!;
  return <SqlLabWorkspace key={ws} ws={ws} />;
}

function SqlLabWorkspace({ ws }: { ws: string }) {
  const { tabs, activeId, setActive, addTab, closeTab, renameTab, patchTab, openTab } = useTabs(ws);
  const [editing, setEditing] = useState<string | null>(null);
  const active = tabs.find((t) => t.id === activeId) ?? tabs[0];

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-1 border-b bg-muted/30 px-1">
        {tabs.map((t) => (
          <div key={t.id} className={`flex items-center gap-1 border-b-2 px-3 py-1.5 text-sm ${t.id === activeId ? "border-primary bg-background font-medium text-foreground" : "border-transparent text-muted-foreground hover:bg-accent"}`}>
            {editing === t.id ? (
              <input autoFocus defaultValue={t.title} className="w-24 bg-transparent outline-none"
                onBlur={(e) => { renameTab(t.id, e.target.value || t.title); setEditing(null); }}
                onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }} />
            ) : (
              <button onClick={() => setActive(t.id)} onDoubleClick={() => setEditing(t.id)} className="max-w-[12rem] truncate">{t.title}</button>
            )}
            <button onClick={() => closeTab(t.id)} className="text-muted-foreground hover:text-foreground"><X className="h-3 w-3" /></button>
          </div>
        ))}
        <button onClick={addTab} className="px-2 py-1 text-muted-foreground hover:text-foreground" title="New tab"><Plus className="h-4 w-4" /></button>
      </div>
      <div className="flex-1 overflow-hidden">
        {active && (
          <SqlTab key={active.id} ws={ws} tab={active}
            patch={(p) => patchTab(active.id, p)}
            onOpenSaved={(sql, dbId) => openTab({ title: "Saved", sql, dbId, limit: active.limit })} />
        )}
      </div>
    </div>
  );
}
