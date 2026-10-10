import type { ReactNode } from "react";
import { useWorkspaceFeatures } from "@/hooks/useWorkspaceFeatures";
import { Forbidden } from "@/routes/Forbidden";

/**
 * Route guard for a single explore module (`sql_lab` / `datasets` / `charts` /
 * `dashboards` / `apps`): renders `Forbidden` when the current
 * workspace does not have `feature` switched on, otherwise renders `children`.
 *
 * Reads the same optimistic list the Sidebar filters on
 * (`useWorkspaceFeatures`) — while `/me/workspaces` is still loading that
 * returns every key, so this never flashes Forbidden at a user who does have
 * the module. The backend's `WsContext` extractor is the real, un-bypassable
 * gate; this only keeps the UI from showing a broken page instead of a clear
 * "you don't have this" message.
 */
export function RequireFeature({ feature, children }: { feature: string; children: ReactNode }) {
  const enabled = useWorkspaceFeatures();
  if (!enabled.includes(feature)) return <Forbidden />;
  return <>{children}</>;
}
