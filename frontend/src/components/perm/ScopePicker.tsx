import { useControlPlaneCatalogRegistryListRegistryCatalogs } from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type { Scope } from "@/api/generated/model";
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
 * can read, via the catalog-registry list.
 */
export function ScopePicker({
  value,
  onChange,
  rowFilterMode = false,
  onApply,
  disabled = false,
}: ScopePickerProps) {
  const catalogsQ = useControlPlaneCatalogRegistryListRegistryCatalogs(undefined, {
    query: { retry: false },
  });
  const catalogs = catalogsQ.data?.status === 200 ? catalogsQ.data.data : [];

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
          <SelectTrigger id="scope-catalog" className="w-full">
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
