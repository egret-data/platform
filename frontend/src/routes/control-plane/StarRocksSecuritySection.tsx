import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { ConfigTable, ConfigTableRow } from "./config/ConfigTable";
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
 * mode is already stored; `"none"` is sent LITERALLY. For a NEW/open engine
 * that literal `"none"` provisions an open engine, so all four options are
 * available there.
 *
 * BUT downgrading an ALREADY-delegated engine (stored jwt/impersonate) to open
 * is NOT backend-supported: `build_starrocks_provision_req` filters
 * `delegation_mode` by `!is_empty()` only, so a stored `"none"` is still treated
 * as active delegation and RE-provisions the JWT security integration + GRANT
 * IMPERSONATE — there's no DROP-security-integration / REVOKE-impersonate
 * teardown DDL. So for a currently-delegated engine we DISABLE the "None"
 * option (it would silently re-affirm delegation while claiming open) and tell
 * the user to recreate the engine instead. This teardown gap is a documented
 * follow-up, not part of the default-on delegation work.
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
  // Read the CURRENTLY-STORED mode (not the live `cfg.delegationMode`, which
  // moves as the user picks). An engine that already runs jwt/impersonate can't
  // be safely downgraded to open (no teardown DDL — see the doc comment above),
  // so "None" is disabled for it. Switching jwt⇄impersonate stays allowed.
  const hasDelegation =
    cfg.storedConfig?.delegation?.mode === "jwt" ||
    cfg.storedConfig?.delegation?.mode === "impersonate";

  const showTls =
    cfg.delegationMode === "jwt" || cfg.delegationMode === "impersonate";

  return (
    <div className="space-y-4">
      <ConfigTable>
        <ConfigTableRow
          label="Per-user delegation"
          htmlFor="sr-sec-delegation"
          details={DELEGATION_HELP[cfg.delegationMode]}
        >
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
              <SelectItem value="none" disabled={hasDelegation}>
                None (open engine)
              </SelectItem>
            </SelectContent>
          </Select>
        </ConfigTableRow>
        {showTls && (
          <ConfigTableRow
            label="Enable TLS"
            htmlFor="sr-sec-tls"
            details="FE SSL on the MySQL-protocol port. Needs a StarRocks image ≥ 3.4.1 (default 3.4 works). Recommended for both delegation modes."
          >
            <Switch
              id="sr-sec-tls"
              checked={cfg.tlsEnabled}
              onCheckedChange={cfg.setTlsEnabled}
              aria-label="Enable TLS"
            />
          </ConfigTableRow>
        )}
      </ConfigTable>
      {hasDelegation && (
        <p className="text-xs text-muted-foreground">
          Removing delegation from a running engine isn&apos;t supported yet —
          recreate the engine to make it open. You can still switch between JWT
          and Impersonate.
        </p>
      )}

      <p className="rounded-md border bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
        The Ranger authorization binding is configured when the engine is created
        and isn&apos;t editable here.
      </p>
    </div>
  );
}
