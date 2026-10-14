import { NamespaceFiles } from "@/components/workflows/namespaces/files/NamespaceFiles";
import type { NamespaceTabProps } from "@/components/workflows/namespaces/namespaceTabs";

/**
 * THE FILES TAB (W7d spec §4.3): the namespace's files in `NamespaceFiles`,
 * the tree on the left and the open files as tabs on the right. Reads need
 * `workflow:read`, which the frame has proven; without `workflow:manage`
 * (`canManage`) every write is disabled with its reason (§4.5).
 *
 * `NamespaceDetail.tsx` reaches this module through `lazy()` only, so the
 * explorer (and the Monaco wrapper it renders) stays out of the namespace
 * frame's chunk (`scripts/assert-workflows-lazy.mjs`).
 */
export default function NamespaceFilesTab({ ws, namespace, canManage }: NamespaceTabProps) {
  return (
    <section aria-labelledby="files-heading" data-testid="namespace-tab-files" className="flex h-full min-w-0 flex-col gap-3">
      <h2 id="files-heading" className="text-lg font-semibold">
        Files
      </h2>
      <NamespaceFiles ws={ws} namespace={namespace} readOnly={!canManage} />
    </section>
  );
}
