import { useEffect, useState } from "react";
import { toast } from "sonner";

import { useSetWorkspaceFeatures, useWorkspaceFeaturesList } from "@/api/features";
import { ConfigTable, ConfigTableRow } from "@/routes/control-plane/config/ConfigTable";
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
 * differs from what's stored, and the PUT always sends the full map (the API
 * is a full replacement, not a per-key patch).
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
        {features.map((f) => (
          <ConfigTableRow key={f.key} label={f.label} details={f.description} htmlFor={`feature-${f.key}`}>
            <Switch
              id={`feature-${f.key}`}
              aria-label={f.label}
              checked={draft[f.key] ?? f.enabled}
              onCheckedChange={(checked) => setDraft((d) => ({ ...(d ?? {}), [f.key]: checked }))}
            />
          </ConfigTableRow>
        ))}
      </ConfigTable>

      <div className="flex justify-end">
        <Button onClick={handleSave} disabled={!dirty || setFeatures.isPending}>
          Save
        </Button>
      </div>
    </div>
  );
}
