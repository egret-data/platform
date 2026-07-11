import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

import { Card, CardContent } from "@/components/ui/card";
import { cn } from "@/lib/utils";

/** Tone maps a stat to a semantic value colour (default text, teal primary,
 * amber highlight, destructive red). Uses the design tokens, so it adapts to
 * light/dark automatically. */
const TONE_VALUE: Record<string, string> = {
  default: "text-foreground",
  success: "text-primary",
  warning: "text-highlight",
  danger: "text-destructive",
};

export interface StatCardProps {
  /** Short uppercase label (e.g. "Running"). */
  label: string;
  /** The metric — a number/string. Rendered with tabular figures. */
  value: ReactNode;
  /** Optional one-line context under the value. */
  hint?: string;
  /** Optional leading icon (Lucide). Decorative — aria-hidden. */
  icon?: LucideIcon;
  tone?: "default" | "success" | "warning" | "danger";
  className?: string;
}

/** A compact KPI/stat card for control-plane overview rows (Data-Dense Dashboard
 * pattern): a muted label, a large tabular value, an optional hint + icon. */
export function StatCard({ label, value, hint, icon: Icon, tone = "default", className }: StatCardProps) {
  return (
    <Card className={cn("shadow-xs", className)}>
      <CardContent className="flex items-start justify-between gap-3 p-4">
        <div className="min-w-0 space-y-1">
          <div className="text-xs font-medium uppercase tracking-wide text-muted-foreground">{label}</div>
          <div className={cn("text-2xl font-semibold leading-none tabular-nums", TONE_VALUE[tone])}>{value}</div>
          {hint ? <div className="truncate text-xs text-muted-foreground">{hint}</div> : null}
        </div>
        {Icon ? <Icon className="h-5 w-5 shrink-0 text-muted-foreground" aria-hidden /> : null}
      </CardContent>
    </Card>
  );
}

/** Responsive grid that lays out a row of `StatCard`s. */
export function StatCardRow({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn("grid grid-cols-2 gap-3 lg:grid-cols-4", className)}>{children}</div>;
}
