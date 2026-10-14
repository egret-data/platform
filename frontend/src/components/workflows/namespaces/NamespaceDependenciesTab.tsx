import { Network } from "lucide-react";
import { EmptyState } from "@/components/explore/EmptyState";

/**
 * THE DEPENDENCIES TAB'S SLOT (W7d). A placeholder that Task 8 (spec §4.2) replaces. The
 * frame renders the default export with `NamespaceTabProps`
 * (`namespaceTabs.ts`); the replacement takes them. `NamespaceDetail.tsx`
 * reaches this module through `lazy()` only, so what it grows into stays out
 * of the namespace frame's chunk (`scripts/assert-workflows-lazy.mjs`).
 */
export default function NamespaceDependenciesTab() {
  return (
    <div data-testid="namespace-tab-dependencies">
      <EmptyState variant="dashed" icon={Network} title="Not available yet" description="The graph of this namespace's flows and the flows they call is not available yet." />
    </div>
  );
}
