import { History } from "lucide-react";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListPageShell } from "@/components/explore/ListPageShell";

/**
 * The Executions entry of the Workflows nav group, until W7b ships the list
 * (spec §3). It holds the route and the nav item now so W7b replaces a page
 * rather than adding a group, and says plainly that the list is not here yet.
 */
export default function ExecutionsPlaceholder() {
  return (
    <ListPageShell title="Executions">
      <EmptyState
        variant="dashed"
        icon={History}
        title="The executions list is not here yet"
        description="It arrives in a later release. Until then, a run you start from a flow opens its own execution page."
      />
    </ListPageShell>
  );
}
