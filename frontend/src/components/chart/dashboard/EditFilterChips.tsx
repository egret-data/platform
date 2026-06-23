import type { DashboardParameter } from "@/api/dashboards";

interface Props { parameters: DashboardParameter[]; activeId: string | null; onSelect: (id: string) => void }

export function EditFilterChips({ parameters, activeId, onSelect }: Props) {
  if (parameters.length === 0) return null;
  return (
    <div className="mb-3 flex flex-wrap items-center gap-2">
      {parameters.map((p) => (
        <button key={p.id} type="button" aria-label={p.name} data-active={p.id === activeId}
          className={`inline-flex h-8 items-center rounded-sm border px-2 text-sm ${p.id === activeId ? "border-primary bg-secondary text-primary" : "bg-background hover:bg-accent/40"}`}
          onClick={() => onSelect(p.id)}>{p.name}{p.required ? " *" : ""}</button>
      ))}
    </div>
  );
}
