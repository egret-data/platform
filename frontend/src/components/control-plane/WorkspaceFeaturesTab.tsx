import { useEffect, useState } from "react";
import { toast } from "sonner";

import { useSetWorkspaceFeatures, useWorkspaceFeaturesList } from "@/api/features";
import { ConfigTable, ConfigTableRow } from "@/components/config/ConfigTable";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";

/**
 * Control Plane → Workspaces → detail → Features tab.
 *
 * These five switches are an ENTITLEMENT, not a role: they decide which
 * modules this workspace HAS at all, not what any particular user inside it
 * may do with them. RBAC (`ws_viewer`/`ws_editor`/`ws_admin`) still governs
 * read/write depth once a module is on — a feature has no "read-only" state
 * of its own, and it applies to every member of the workspace, platform
 * admins included, with no bypass (enforced server-side in `WsContext`, not
 * just hidden here).
 *
 * One editing surface + one Save, mirroring the engine/StarRocks config tabs:
 * local `draft` state seeded from the loaded features, Save disabled until it
 * differs from what's stored, and the PUT always sends the full five-key map.
 * The API itself merges (a key omitted from the body is left as-is), but this
 * UI shows and edits all five, so it should own all five — sending the full
 * map keeps the saved state exactly matching what the switches display.
 *
 * `dashboards` depends on `charts`: a dashboard's saved cards render through
 * the `dashboards`-gated surface, but its interactive filter dropdowns and
 * edit-mode card previews both run through the `charts`-gated
 * `POST /charts/run`, so `dashboards` on with `charts` off is a broken
 * configuration, not just an unusual one (the backend rejects it too — see
 * `violates_dependencies` in `egret-workspace`). This component keeps the
 * draft unable to represent that combination at all: toggling Charts off
 * also turns Dashboards off and disables its switch, and toggling Dashboards
 * on also turns Charts on.
 */
export function WorkspaceFeaturesTab({ workspaceId }: { workspaceId: string }) {
  const { features, isLoading } = useWorkspaceFeaturesList(workspaceId);
  const setFeatures = useSetWorkspaceFeatures(workspaceId);
  const [draft, setDraft] = useState<Record<string, boolean> | null>(null);

  // Seed (and re-seed on refetch, e.g. after Save) from the loaded features.
  useEffect(() => {
    if (features) setDraft(Object.fromEntries(features.map((f) => [f.key, f.enabled])));
  }, [features]);

  if (isLoading || !features || !draft) {
    return <p className="text-sm text-muted-foreground">Loading features…</p>;
  }

  const dirty = features.some((f) => draft[f.key] !== f.enabled);
  const chartsOn = draft.charts ?? true;

  function setFeature(key: string, checked: boolean) {
    setDraft((d) => {
      const next = { ...(d ?? {}), [key]: checked };
      // Keep the dependency true by construction, in whichever direction
      // caused the change: turning Charts off takes Dashboards with it, and
      // turning Dashboards on brings Charts along.
      if (key === "charts" && !checked) next.dashboards = false;
      if (key === "dashboards" && checked) next.charts = true;
      return next;
    });
  }

  function handleSave() {
    if (!draft) return;
    setFeatures.mutate(
      { id: workspaceId, data: { features: draft } },
      {
        onSuccess: () => toast.success("Workspace features updated"),
        onError: (e) => toast.error(String(e)),
      },
    );
  }

  return (
    <div className="space-y-4">
      <p className="text-sm text-muted-foreground">
        These switches decide which modules this workspace has at all —
        turning one off hides it from the sidebar and blocks its API for
        everyone in this workspace, including platform admins. It doesn&apos;t
        delete anything: existing content is kept and comes back exactly as it
        was when the module is switched back on. Who can read or write within
        an enabled module is still decided by workspace roles.
      </p>

      <ConfigTable settingLabel="Module">
        {features.map((f) => {
          const isDashboards = f.key === "dashboards";
          const disabled = isDashboards && !chartsOn;
          const details = disabled
            ? `${f.description} Requires Charts to be enabled.`
            : f.key === "data_api"
              ? `${f.description} Turning this off hides the Data API pages here — live endpoints keep serving; unpublish each one individually to actually stop it.`
              : f.description;
          return (
            <ConfigTableRow key={f.key} label={f.label} details={details} htmlFor={`feature-${f.key}`}>
              <Switch
                id={`feature-${f.key}`}
                aria-label={f.label}
                checked={draft[f.key] ?? f.enabled}
                disabled={disabled}
                onCheckedChange={(checked) => setFeature(f.key, checked)}
              />
            </ConfigTableRow>
          );
        })}
      </ConfigTable>

      <div className="flex justify-end">
        <Button onClick={handleSave} disabled={!dirty || setFeatures.isPending}>
          Save
        </Button>
      </div>
    </div>
  );
}
