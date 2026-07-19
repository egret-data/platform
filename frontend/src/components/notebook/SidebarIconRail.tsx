import { List, Database } from "lucide-react";

export type SidebarTab = "toc" | "dataSources";

const TABS: { key: SidebarTab; icon: typeof List; title: string }[] = [
  { key: "toc", icon: List, title: "Table of Contents" },
  { key: "dataSources", icon: Database, title: "Data Sources" },
];

export function SidebarIconRail({
  tab,
  open,
  onPick,
}: {
  tab: SidebarTab;
  open: boolean;
  onPick: (tab: SidebarTab) => void;
}) {
  return (
    <aside className="flex w-12 shrink-0 flex-col items-center border-r bg-muted/50 py-2">
      {TABS.map(({ key, icon: Icon, title }) => {
        const active = tab === key && open;
        return (
          <button
            key={key}
            type="button"
            title={title}
            aria-label={title}
            aria-pressed={active}
            onClick={() => onPick(key)}
            className={`mb-2 rounded p-2 transition-colors ${
              active
                ? "bg-primary/10 text-primary"
                : "text-muted-foreground hover:bg-muted hover:text-foreground"
            }`}
          >
            <Icon className="h-5 w-5" />
          </button>
        );
      })}
    </aside>
  );
}
