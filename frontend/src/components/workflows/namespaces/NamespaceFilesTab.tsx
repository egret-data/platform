import { FileCode } from "lucide-react";
import { EmptyState } from "@/components/explore/EmptyState";

/**
 * THE FILES TAB'S SLOT (W7d). A placeholder that Tasks 6 and 7 (spec §4.3) replaces. The
 * frame renders the default export with `NamespaceTabProps`
 * (`namespaceTabs.ts`); the replacement takes them. `NamespaceDetail.tsx`
 * reaches this module through `lazy()` only, so what it grows into stays out
 * of the namespace frame's chunk (`scripts/assert-workflows-lazy.mjs`).
 */
export default function NamespaceFilesTab() {
  return (
    <div data-testid="namespace-tab-files">
      <EmptyState variant="dashed" icon={FileCode} title="Not available yet" description="This namespace's files cannot be browsed here yet." />
    </div>
  );
}
