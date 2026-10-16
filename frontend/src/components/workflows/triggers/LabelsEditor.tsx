import { Plus, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { labelRowError, type LabelRow } from "@/components/workflows/triggers/backfillForm";

/**
 * The backfill's labels (W7f spec §4.3): rows of key and value, each with its
 * error beside it (FE-FORM-4). Every input has its own `Label` (FE-FORM-2).
 * A row left fully empty is dropped from the request, so Remove is the only
 * way a row needs to go.
 */
export function LabelsEditor({
  rows,
  onChange,
  idPrefix,
  serverError,
}: {
  rows: LabelRow[];
  onChange: (rows: LabelRow[]) => void;
  /** Unique to the form, so two forms on a page never share an id. */
  idPrefix: string;
  /** The server's refusal of the labels, shown beside the list. */
  serverError?: string;
}) {
  const update = (i: number, patch: Partial<LabelRow>) => onChange(rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  return (
    <fieldset className="grid gap-2">
      <legend className="text-sm font-medium">Labels</legend>
      <p className="text-xs text-muted-foreground">Added to every execution this backfill starts. A backfill label overrides a flow label with the same key.</p>
      {rows.map((row, i) => {
        const error = labelRowError(row);
        const keyId = `${idPrefix}-label-key-${i}`;
        const valueId = `${idPrefix}-label-value-${i}`;
        const errorId = `${idPrefix}-label-error-${i}`;
        return (
          <div key={i} className="grid gap-1">
            <div className="grid grid-cols-[1fr_1fr_auto] items-end gap-2">
              <div className="grid gap-1">
                <Label htmlFor={keyId}>Key</Label>
                <Input
                  id={keyId}
                  value={row.key}
                  aria-invalid={error ? true : undefined}
                  aria-describedby={error ? errorId : undefined}
                  onChange={(e) => update(i, { key: e.target.value })}
                />
              </div>
              <div className="grid gap-1">
                <Label htmlFor={valueId}>Value</Label>
                <Input
                  id={valueId}
                  value={row.value}
                  aria-invalid={error ? true : undefined}
                  aria-describedby={error ? errorId : undefined}
                  onChange={(e) => update(i, { value: e.target.value })}
                />
              </div>
              <Button
                type="button"
                variant="ghost"
                size="iconSm"
                aria-label={`Remove label ${i + 1}`}
                onClick={() => onChange(rows.filter((_, j) => j !== i))}
              >
                <X aria-hidden />
              </Button>
            </div>
            {error ? (
              <p id={errorId} className="text-sm text-destructive">
                {error}
              </p>
            ) : null}
          </div>
        );
      })}
      {serverError ? <p className="text-sm text-destructive">{serverError}</p> : null}
      <div>
        <Button type="button" variant="outline" size="sm" onClick={() => onChange([...rows, { key: "", value: "" }])}>
          <Plus aria-hidden />
          Add label
        </Button>
      </div>
    </fieldset>
  );
}
