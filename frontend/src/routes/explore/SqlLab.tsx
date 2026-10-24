import { useState } from "react";
import { useParams } from "react-router-dom";
import { Command, Plus, X } from "lucide-react";
import { useTabs } from "@/hooks/useTabs";
import { SqlTab } from "@/components/sqllab/SqlTab";
import { PageContainer } from "@/components/layout/PageContainer";
import { PageTitle } from "@/components/layout/PageTitle";

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
    // A tool page: full width, and its title is the compact 16px one, at the
    // start of the tab bar where it costs the editor no height (FE-PAGE-7).
    <PageContainer className="flex h-full flex-col bg-background">
      {/* Tab strip — VS Code-style editor tabs. */}
      <div className="flex items-stretch border-b bg-muted/40">
        <PageTitle size="compact" className="flex shrink-0 items-center border-r px-3">
          SQL Lab
        </PageTitle>
        <div className="flex min-w-0 flex-1 items-stretch overflow-x-auto">
          {tabs.map((t) => {
            const activeTab = t.id === activeId;
            return (
              <div key={t.id}
                className={`group flex min-w-0 items-center gap-1.5 border-r px-3 text-sm ${
                  activeTab
                    ? "border-t-2 border-t-primary bg-background font-medium text-foreground"
                    : "border-t-2 border-t-transparent text-muted-foreground hover:bg-accent/60"}`}>
                {editing === t.id ? (
                  <input autoFocus defaultValue={t.title} aria-label="Tab name"
                    // Select the old name, so typing replaces it (KI-094).
                    onFocus={(e) => e.currentTarget.select()}
                    className="my-1.5 w-28 rounded bg-transparent px-1 outline-none ring-2 ring-ring"
                    onBlur={(e) => { renameTab(t.id, e.target.value || t.title); setEditing(null); }}
                    onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); if (e.key === "Escape") setEditing(null); }} />
                ) : (
                  <button onClick={() => setActive(t.id)} onDoubleClick={() => setEditing(t.id)}
                    title="Double-click to rename"
                    className="max-w-[12rem] truncate py-2">{t.title}</button>
                )}
                <button onClick={() => closeTab(t.id)} aria-label={`Close ${t.title}`}
                  className="flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 transition hover:bg-accent hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100">
                  <X className="h-3.5 w-3.5" />
                </button>
              </div>
            );
          })}
          <button onClick={addTab} title="New tab" aria-label="New tab"
            className="flex h-9 w-9 shrink-0 items-center justify-center text-muted-foreground hover:bg-accent hover:text-foreground">
            <Plus className="h-4 w-4" />
          </button>
        </div>
        <div className="hidden items-center gap-1 border-l px-3 text-[11px] text-muted-foreground sm:flex">
          <Command className="h-3 w-3" /> K for commands
        </div>
      </div>
      {/* Every tab stays mounted and only the active one is shown, so a tab
          keeps its result, run status and parameter values while another is
          in front (KI-082). */}
      {tabs.map((t) => (
        <div key={t.id} hidden={t.id !== active?.id} className="min-h-0 flex-1 overflow-hidden">
          <SqlTab ws={ws} tab={t} active={t.id === active?.id}
            patch={(p) => patchTab(t.id, p)}
            onOpenSaved={(sql, dbId, name) => openTab({ title: name, sql, dbId, limit: t.limit })}
            onOpenPreview={(init) => openTab({ ...init, limit: 100, autoRun: true })} />
        </div>
      ))}
    </PageContainer>
  );
}
