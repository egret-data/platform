import type { ReactNode } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ListSkeleton } from "@/components/explore/ListSkeleton";

/**
 * The four-way branch every Explore list page opens with: loading, error, empty,
 * or data.
 *
 * Written out as a nested ternary in all six pages, and they have already drifted
 * — `Dashboards.tsx` checks `hasData` before `isError`, so a failed refetch that
 * still holds cached rows renders the table and swallows the error, while the
 * other five render the alert.
 *
 * **`isError` is checked before `hasData`, and empty is never used to stand in
 * for a failure.** Collapsing the two is the defect two account pickers shipped:
 * an operator was told to create a service account that already existed.
 */
export function ListStates({
  isLoading,
  isError,
  error,
  errorText,
  hasData,
  skeletonCols,
  empty,
  children,
}: {
  isLoading: boolean;
  isError: boolean;
  error?: unknown;
  /** Shown when the thrown error carries no message of its own. */
  errorText: string;
  hasData: boolean;
  skeletonCols: number;
  empty: ReactNode;
  children: ReactNode;
}) {
  if (isLoading) return <ListSkeleton cols={skeletonCols} />;
  if (isError) {
    // `|| errorText`, not `??`: a message-less `UnauthorizedError` and an
    // `ApiError` built from an empty `statusText` both produce "", which `??`
    // would happily render as a blank alert.
    const message = (error as Error | undefined)?.message || errorText;
    return (
      <Alert variant="destructive">
        <AlertDescription>{message}</AlertDescription>
      </Alert>
    );
  }
  if (!hasData) return <>{empty}</>;
  return <>{children}</>;
}
