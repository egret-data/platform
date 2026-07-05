import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { FieldLabel } from "./FieldLabel";
import type { KnownProperty } from "./knownProperties";
import { SELECT_NONE } from "./podEditors";
import { QuantityInput } from "./QuantityInput";

/**
 * Renders the appropriate input widget for a single `KnownProperty` by its
 * `type`, reading and writing one string value. A pure controlled component:
 * value in, `onChange` out — it holds no `configProperties` knowledge (the
 * sections wire `getProp`/`setProp`).
 *
 * Every variant surfaces the property's inline `help` tooltip + `docsUrl` link
 * via `FieldLabel`, so catalog-driven help needs no `fieldHelp.ts` entry.
 */
export function PropertyField({
  prop,
  value,
  onChange,
}: {
  prop: KnownProperty;
  value: string;
  onChange: (v: string) => void;
}) {
  if (prop.type === "size") {
    // QuantityInput owns its own FieldLabel; forward the inline help/docsUrl so
    // it shows the catalog help while keeping the unit hint + validation.
    return (
      <QuantityInput
        fieldKey={prop.key}
        label={prop.label}
        value={value}
        onChange={onChange}
        placeholder={prop.default}
        help={prop.help}
        docsUrl={prop.docsUrl}
      />
    );
  }

  const labelEl = (
    <FieldLabel label={prop.label} help={prop.help} docsUrl={prop.docsUrl} />
  );

  if (prop.type === "enum") {
    return (
      <div className="space-y-1.5">
        {labelEl}
        <Select
          value={value || SELECT_NONE}
          onValueChange={(v) => onChange(v === SELECT_NONE ? "" : v)}
        >
          <SelectTrigger className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={SELECT_NONE}>(default)</SelectItem>
            {(prop.enumOptions ?? []).map((opt) => (
              <SelectItem key={opt} value={opt}>
                {opt}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    );
  }

  if (prop.type === "boolean") {
    return (
      <div className="space-y-1.5">
        {labelEl}
        <div>
          <Switch
            checked={value === "true"}
            onCheckedChange={(v) => onChange(v ? "true" : "")}
          />
        </div>
      </div>
    );
  }

  // number | duration | string
  return (
    <div className="space-y-1.5">
      {labelEl}
      <Input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={prop.default}
      />
    </div>
  );
}
