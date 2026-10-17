import { useControlPlaneGovernanceListCatalogs } from "@/api/generated/control-plane-governance/control-plane-governance";
import type { Scope } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

export interface ScopePickerProps {
  value: Scope;
  onChange: (next: Scope) => void;
  /**
   * True when this `Scope` selects a row filter by path (`Selector`'s
   * `Path` variant on a `row_filter` rule), rather than a grant's resource
   * or a masking rule's target.
   *
   * **Hardcoded fact 4/5 (design spec D6):** Ranger's row-filter service-def
   * (`rowFilterDef`, measured against the live service-def during SP4a)
   * declares only three resource levels — catalog, schema, table — all
   * mandatory, and no column level at all. In this mode the picker never
   * renders a Column field (the control is absent, not merely disabled),
   * and treats catalog+schema+table as jointly required for `onApply`.
   */
  rowFilterMode?: boolean;
  /**
   * Optional confirm affordance (e.g. inside a Dialog's footer). Disabled
   * until `value` satisfies this picker's mode — see `rowFilterMode` above.
   * Omit it to use `ScopePicker` as a plain continuously-controlled field.
   */
  onApply?: (scope: Scope) => void;
  disabled?: boolean;
}

/**
 * Edits a `Scope` — `{catalog_id, schema?, table?, column?}` — narrowing
 * progressively: schema is disabled until a catalog is chosen, table until a
 * schema is entered, column (outside `rowFilterMode`) until a table is
 * entered.
 *
 * `catalog_id` is a bare uuid — `perm.grants`/`perm.rules` store it that way
 * and neither `egret-perm` nor `egret-perm-compile` resolve it to a name —
 * so this picker is the one place that turns it back into something a human
 * can read, via the Governance catalog list (`GET /governance/catalogs`,
 * `policy:read` or `tag:read`). Not the catalog-registry list: that one needs
 * `catalog_registry:read`, which a steward holding `policy:read` does not
 * have, and its picker came up empty for them.
 */
export function ScopePicker({
  value,
  onChange,
  rowFilterMode = false,
  onApply,
  disabled = false,
}: ScopePickerProps) {
  const catalogsQ = useControlPlaneGovernanceListCatalogs({ query: { retry: false } });
  const catalogs = catalogsQ.data?.status === 200 ? catalogsQ.data.data : [];

  /**
   * **A picker that cannot be operated has to say so (SP4b-2 finding 2).**
   * With the registry list, `policy:read` alone found this dropdown holding
   * zero options, `Schema`/`Table`/`Column` disabled behind it and `Explain`
   * permanently disabled, with nothing on the page saying why. The Governance
   * list closes that for a steward, but an account with neither `policy:read`
   * nor `tag:read` still gets a 403 here, and it still must not fail
   * silently. The two cases read differently on purpose: a missing permission
   * is something an administrator can fix for this account, an empty registry
   * is not about this account at all.
   */
  const forbidden = catalogsQ.error instanceof ApiError && catalogsQ.error.status === 403;
  const nothingToPick = !catalogsQ.isLoading && catalogs.length === 0;

  function patch(next: Partial<Scope>) {
    onChange({ ...value, ...next });
  }

  const canPickSchema = !!value.catalog_id;
  const canPickTable = !!value.schema;
  const canPickColumn = !rowFilterMode && !!value.table;

  const satisfiesMode = rowFilterMode
    ? Boolean(value.catalog_id && value.schema && value.table && !value.column)
    : Boolean(value.catalog_id);

  return (
    <div className="space-y-3">
      <div className="space-y-1.5">
        <Label htmlFor="scope-catalog">Catalog</Label>
        <Select
          value={value.catalog_id || undefined}
          onValueChange={(v) => onChange({ catalog_id: v })}
          disabled={disabled}
        >
          <SelectTrigger
            id="scope-catalog"
            className="w-full"
            aria-describedby={nothingToPick ? "scope-catalog-unavailable" : undefined}
          >
            <SelectValue placeholder="Select a catalog" />
          </SelectTrigger>
          <SelectContent>
            {catalogs.map((c) => (
              <SelectItem key={c.id} value={c.id}>
                {c.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {nothingToPick && (
          <p
            id="scope-catalog-unavailable"
            role="alert"
            className={forbidden ? "text-xs text-destructive" : "text-xs text-muted-foreground"}
          >
            {forbidden ? (
              <>
                No catalogs to choose from: listing them needs the <code>policy:read</code>{" "}
                or <code>tag:read</code> permission, which this account does not have.
                Nothing on this form can name a target until a platform admin grants one.
              </>
            ) : (
              <>
                No catalogs are registered yet, so there is no target to name. Register one
                under Control Plane → Catalogs first.
              </>
            )}
          </p>
        )}
      </div>

      <div className="space-y-1.5">
        <Label htmlFor="scope-schema">Schema</Label>
        <Input
          id="scope-schema"
          value={value.schema ?? ""}
          onChange={(e) => patch({ schema: e.target.value || undefined, table: undefined, column: undefined })}
          disabled={disabled || !canPickSchema}
          placeholder={canPickSchema ? "e.g. sales" : "Select a catalog first"}
        />
      </div>

      <div className="space-y-1.5">
        <Label htmlFor="scope-table">
          Table
          {rowFilterMode && (
            <span className="text-destructive" aria-hidden>
              {" "}
              *
            </span>
          )}
        </Label>
        <Input
          id="scope-table"
          value={value.table ?? ""}
          onChange={(e) => patch({ table: e.target.value || undefined, column: undefined })}
          disabled={disabled || !canPickTable}
          placeholder={canPickTable ? "e.g. orders" : "Enter a schema first"}
        />
      </div>

      {!rowFilterMode && (
        <div className="space-y-1.5">
          <Label htmlFor="scope-column">Column</Label>
          <Input
            id="scope-column"
            value={value.column ?? ""}
            onChange={(e) => patch({ column: e.target.value || undefined })}
            disabled={disabled || !canPickColumn}
            placeholder={canPickColumn ? "e.g. ssn" : "Enter a table first"}
          />
        </div>
      )}

      {onApply && (
        <Button
          type="button"
          onClick={() => onApply(value)}
          disabled={disabled || !satisfiesMode}
        >
          Apply
        </Button>
      )}
    </div>
  );
}
