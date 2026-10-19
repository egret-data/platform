import type { EngineTab } from "@/routes/governance/engines/engineUrl";
import { TAB_LABEL, type TabContext } from "@/routes/governance/engines/PageFrame";
import { EnginePermissionsTab } from "@/routes/governance/engines/tabs/EnginePermissionsTab";

/**
 * One body per tab of the engine and profile pages (spec §6.1–§6.6). Each tab
 * receives the page's `TabContext` and owns its own loading, error and empty
 * states (FE-STATE-1). Tabs not built yet render their name only.
 */
export function EngineTabBody({ tab, ctx }: { tab: EngineTab; ctx: TabContext }) {
  if (tab === "engine-permissions") return <EnginePermissionsTab ctx={ctx} />;
  return <p className="text-sm text-muted-foreground">{TAB_LABEL[tab]}</p>;
}
