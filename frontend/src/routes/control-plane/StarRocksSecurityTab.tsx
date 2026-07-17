import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { SaveBar } from "./EngineDetail";
import { DELEGATION_HELP } from "./ControlPlaneEngines";
import {
  type StarRocksConfigForm,
  type StarRocksDelegationOption,
} from "./useStarRocksConfigForm";

/**
 * Control Plane → Engine detail → Security & Exposure (StarRocks).
 *
 * StarRocks-shaped mirror of the Trino `SecurityTab`, driven by the SAME
 * `useStarRocksConfigForm` instance the Configuration tab uses (passed down from
 * `EngineTabs`). Editing delegation / TLS here and pressing Save re-provisions
 * the engine together with any pending config edits — one form, one Save.
 *
 * Scoped to per-user delegation + transport TLS. The Ranger/authz binding
 * (`authzServiceId`) is NOT editable here: the StarRocks update path
 * (`update_starrocks_engine` → `build_starrocks_spec`) never reads
 * `authz_service_id` from the PUT body, so it's fixed at creation. Surfacing an
 * editable control would fake a change the backend silently ignores.
 */
export function StarRocksSecurityTab({ cfg }: { cfg: StarRocksConfigForm }) {
  // Read the CURRENTLY-STORED mode (not the live `cfg.delegationMode`, which
  // moves as the user picks) so the "can't be disabled" note reflects the
  // engine's persisted state regardless of the in-progress selection. Delegation
  // can be switched jwt⇄impersonate, but not removed after creation: `save()`
  // omits delegation when mode=none and `build_starrocks_spec` treats an omitted
  // block as "preserve", so picking None resolves back to the stored mode (a
  // silent no-op). Surface that honestly instead of offering a dead choice.
  const hasDelegation =
    cfg.storedConfig?.delegation?.mode === "jwt" ||
    cfg.storedConfig?.delegation?.mode === "impersonate";

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-lg">Security &amp; Exposure</CardTitle>
        <CardDescription>
          Per-user delegation and transport security. Saving re-provisions the
          engine.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="sr-sec-delegation">Per-user delegation</Label>
            <Select
              value={cfg.delegationMode}
              onValueChange={(v) =>
                cfg.setDelegationMode(v as StarRocksDelegationOption)
              }
            >
              <SelectTrigger id="sr-sec-delegation" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="none" disabled={hasDelegation}>
                  None (default)
                </SelectItem>
                <SelectItem value="jwt">JWT over TLS</SelectItem>
                <SelectItem value="impersonate">Impersonate</SelectItem>
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              {DELEGATION_HELP[cfg.delegationMode]}
            </p>
            {hasDelegation && (
              <p className="text-xs text-muted-foreground">
                Delegation can&apos;t be disabled after creation — you can switch
                between JWT and Impersonate, but removing it requires recreating
                the engine.
              </p>
            )}
          </div>

          {cfg.delegationMode !== "none" && (
            <div className="space-y-1.5">
              <div className="flex items-center justify-between gap-4">
                <Label htmlFor="sr-sec-tls" className="font-normal">
                  Enable TLS
                </Label>
                <Switch
                  id="sr-sec-tls"
                  checked={cfg.tlsEnabled}
                  onCheckedChange={cfg.setTlsEnabled}
                  aria-label="Enable TLS"
                />
              </div>
              <p className="text-xs text-muted-foreground">
                Enables FE SSL on the MySQL-protocol port. Requires a StarRocks
                image &ge; 3.4.1 (the default 3.4 line works); an older FE
                can&apos;t serve the SSL listener. Recommended for both
                delegation modes.
              </p>
            </div>
          )}

          <p className="rounded-md border bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
            The Ranger authorization binding is configured when the engine is
            created and isn&apos;t editable here.
          </p>

          <SaveBar cfg={cfg} />
        </div>
      </CardContent>
    </Card>
  );
}
