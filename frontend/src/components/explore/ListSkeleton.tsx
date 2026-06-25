import { Skeleton } from "@/components/ui/skeleton";

export function ListSkeleton({ rows = 8, cols = 4 }: { rows?: number; cols?: number }) {
  return (
    <div data-testid="list-skeleton" className="space-y-2">
      {Array.from({ length: rows }).map((_, r) => (
        <div key={r} className="flex gap-3">
          {Array.from({ length: cols }).map((_, c) => (
            <Skeleton key={c} className="h-5 flex-1" />
          ))}
        </div>
      ))}
    </div>
  );
}
