import { Circle } from "lucide-react";
import type { KernelStatus } from "@/lib/kernel/KernelConnection";

interface StatusStyle {
  label: string;
  /** Tailwind text-color utility, or "" when using inlineColor. */
  cls: string;
  /** Inline color for tokens without a utility (e.g. --success). */
  inlineColor?: string;
  pulse?: boolean;
}

const STYLES: Record<KernelStatus, StatusStyle> = {
  disconnected: { label: "Disconnected", cls: "text-muted-foreground" },
  connecting: { label: "Connecting…", cls: "text-primary", pulse: true },
  spawning: { label: "Starting…", cls: "text-primary", pulse: true },
  connected: { label: "Connected", cls: "", inlineColor: "var(--success)" },
  idle: { label: "Ready", cls: "", inlineColor: "var(--success)" },
  busy: { label: "Running…", cls: "text-primary", pulse: true },
  interrupting: { label: "Interrupting…", cls: "text-primary", pulse: true },
  restarting: { label: "Restarting…", cls: "text-primary", pulse: true },
  error: { label: "Error", cls: "text-destructive" },
  dead: { label: "Kernel dead", cls: "text-destructive" },
};

export function KernelStatusBadge({ status }: { status: KernelStatus }) {
  const s = STYLES[status];
  return (
    <div className="flex items-center gap-1.5" title={s.label} aria-live="polite">
      <Circle
        className={`h-2.5 w-2.5 fill-current ${s.pulse ? "motion-safe:animate-pulse" : ""} ${s.cls}`}
        style={s.inlineColor ? { color: s.inlineColor } : undefined}
      />
      {/* Fixed width sized to the longest label so the toolbar doesn't shift. */}
      <span className="inline-block w-28 whitespace-nowrap text-sm text-muted-foreground">
        {s.label}
      </span>
    </div>
  );
}
