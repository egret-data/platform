import type { LucideIcon } from "lucide-react";
import { AlertTriangle } from "lucide-react";
import { Skeleton } from "@/components/ui/skeleton";
import { Button } from "@/components/ui/button";

export function ChartEmptyState({ icon: Icon, title, hint }: { icon?: LucideIcon; title: string; hint: string }) {
  return (
    <div className="flex min-h-[60vh] flex-col items-center justify-center gap-2.5 rounded-lg border border-dashed p-6 text-center">
      {Icon && <Icon className="h-10 w-10 text-muted-foreground/40" aria-hidden />}
      <p className="text-sm font-medium">{title}</p>
      <p className="max-w-xs text-xs text-muted-foreground">{hint}</p>
    </div>
  );
}

export function ChartLoadingState() {
  const bars = [48, 72, 56, 88, 62, 78, 52, 68];
  return (
    <div className="flex min-h-[60vh] flex-col rounded-lg border p-4" role="status" aria-label="Loading chart">
      <div className="flex flex-1 items-end gap-2 pb-6 pt-10">
        {bars.map((h, i) => (
          <Skeleton key={i} className="flex-1 rounded-sm" style={{ height: `${h}%` }} />
        ))}
      </div>
      <Skeleton className="h-3 w-24 self-center" />
    </div>
  );
}

export function ChartErrorState({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return (
    <div className="flex min-h-[40vh] flex-col items-center justify-center gap-3 rounded-lg border border-destructive/30 bg-destructive/5 p-6 text-center">
      <AlertTriangle className="h-8 w-8 text-destructive" aria-hidden />
      <p className="max-w-md text-sm text-destructive">{message}</p>
      {onRetry && (
        <Button size="sm" variant="outline" onClick={onRetry}>
          Retry
        </Button>
      )}
    </div>
  );
}
