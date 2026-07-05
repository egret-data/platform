import { Input } from "@/components/ui/input";
import { FieldLabel } from "./FieldLabel";
import { fieldHelp } from "./fieldHelp";

/**
 * Validate a size/quantity string against the shapes accepted by k8s resource
 * quantities and JVM/Trino byte sizes: a bare number, an optional decimal, and
 * an optional single unit suffix (no internal spaces). Empty is treated as
 * valid so the field can be left blank.
 *
 * This is ADVISORY only — it drives an inline hint, not a save block; the
 * backend validates quantities authoritatively.
 */
export function isValidQuantity(v: string): boolean {
  if (v.trim() === "") return true;
  return /^\d+(\.\d+)?(m|k|Ki|Mi|Gi|Ti|Pi|Ei|M|G|T|P|E|KB|MB|GB|TB|kB)?$/.test(
    v,
  );
}

/**
 * A validated size/quantity input for CPU/memory/heap/size fields. Renders a
 * `FieldLabel` (with any `fieldHelp` tooltip), a shadcn `Input`, an optional
 * unit hint, and an inline advisory error when the current value is non-empty
 * and not a recognisable quantity. It never blocks Save.
 */
export function QuantityInput({
  fieldKey,
  label,
  value,
  onChange,
  placeholder,
  help,
  docsUrl,
}: {
  fieldKey?: string;
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  help?: string;
  docsUrl?: string;
}) {
  const unit = fieldKey ? fieldHelp[fieldKey]?.unit : undefined;
  const invalid = value !== "" && !isValidQuantity(value);

  return (
    <div className="space-y-1.5">
      <FieldLabel
        fieldKey={fieldKey}
        label={label}
        help={help}
        docsUrl={docsUrl}
      />
      <Input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        aria-invalid={invalid}
      />
      {unit ? (
        <p className="text-xs text-muted-foreground">Expected: {unit}</p>
      ) : null}
      {invalid ? (
        <p className="text-xs text-destructive">
          Not a recognized quantity (e.g. 4Gi, 500m, 2GB).
        </p>
      ) : null}
    </div>
  );
}
