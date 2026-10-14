import { KeyRound } from "lucide-react";
import { EmptyState } from "@/components/explore/EmptyState";

/**
 * THE KV TAB'S SLOT (W7d). A placeholder that Task 5 (spec §4.4) replaces. The
 * frame renders the default export with `NamespaceTabProps`
 * (`namespaceTabs.ts`); the replacement takes them. `NamespaceDetail.tsx`
 * reaches this module through `lazy()` only, so what it grows into stays out
 * of the namespace frame's chunk (`scripts/assert-workflows-lazy.mjs`).
 */
export default function NamespaceKvTab() {
  return (
    <div data-testid="namespace-tab-kv">
      <EmptyState variant="dashed" icon={KeyRound} title="Not available yet" description="This namespace's KV keys cannot be managed here yet." />
    </div>
  );
}
