import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { DELEGATION_HELP } from "./ControlPlaneEngines";
import {
  type StarRocksConfigForm,
  type StarRocksDelegationOption,
} from "./useStarRocksConfigForm";

/**
 * Control Plane → Engine detail → Settings → Security & Delegation (StarRocks).
 *
 * The StarRocks Security & Delegation content (per-user delegation + transport
 * TLS + the read-only Ranger note), with NO Card or SaveBar of its own. Rendered
 * as the last section of the merged StarRocks Settings left-nav
 * (`StarRocksConfigTab`), driven by the SAME `useStarRocksConfigForm` instance
 * the config sections use — so the single shared `SaveBar` covers delegation
 * edits together with any pending config edits (one form, one Save).
 *
 * Same Auto/JWT/Impersonate/None options as the create wizard
 * (`ControlPlaneEngines.tsx`) and the Trino `EngineSecurity.tsx` selector.
 * Auto (`""`) is omitted from the PUT body so the backend preserves whatever
 * mode is already stored; `"none"` is sent LITERALLY and actually disables
 * delegation (`build_starrocks_spec` rewrites the block for any non-empty
 * mode) — so, unlike before, it is a freely reachable choice rather than a
 * disabled no-op.
 *
 * Scoped to per-user delegation + transport TLS. The Ranger/authz binding
 * (`authzServiceId`) is NOT editable here: the StarRocks update path
 * (`update_starrocks_engine` → `build_starrocks_spec`) never reads
 * `authz_service_id` from the PUT body, so it's fixed at creation. Surfacing an
 * editable control would fake a change the backend silently ignores.
 */
export function StarRocksSecuritySection({
  cfg,
}: {
  cfg: StarRocksConfigForm;
}) {
  return (
    <div className="space-y-4">
      <div className="space-y-1.5">
        <Label htmlFor="sr-sec-delegation">Per-user delegation</Label>
        <Select
          value={cfg.delegationMode || "auto"}
          onValueChange={(v) =>
            cfg.setDelegationMode(
              v === "auto" ? "" : (v as StarRocksDelegationOption),
            )
          }
        >
          <SelectTrigger id="sr-sec-delegation" className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="auto">
              Auto — JWT, else impersonate (recommended)
            </SelectItem>
            <SelectItem value="jwt">JWT (per-user)</SelectItem>
            <SelectItem value="impersonate">Impersonate (service user)</SelectItem>
            <SelectItem value="none">None (open engine)</SelectItem>
          </SelectContent>
        </Select>
        <p className="text-xs text-muted-foreground">
          {DELEGATION_HELP[cfg.delegationMode]}
        </p>
      </div>

      {(cfg.delegationMode === "jwt" || cfg.delegationMode === "impersonate") && (
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
            Enables FE SSL on the MySQL-protocol port. Requires a StarRocks image
            &ge; 3.4.1 (the default 3.4 line works); an older FE can&apos;t serve
            the SSL listener. Recommended for both delegation modes.
          </p>
        </div>
      )}

      <p className="rounded-md border bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
        The Ranger authorization binding is configured when the engine is created
        and isn&apos;t editable here.
      </p>
    </div>
  );
}
