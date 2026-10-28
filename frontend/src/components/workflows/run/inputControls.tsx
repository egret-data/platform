import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { IconButton } from "@/components/ui/icon-button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import type { FieldValue, RunInput } from "@/components/workflows/run/inputModel";

/**
 * One control per input type (W7a spec §6.1), each under a `Label` tied by
 * `htmlFor` (FE-FORM-2), with the input's description, a required marker, and
 * its error directly under it (FE-FORM-4).
 *
 * JSON and YAML are a monospace text area rather than Monaco: Monaco keeps Tab
 * for indentation, so a keyboard user tabbing through the run form is caught
 * in it, and its text box cannot carry the `id` a `Label` points at.
 */
export interface InputFieldProps {
  input: RunInput;
  value: FieldValue;
  onChange: (value: FieldValue) => void;
  error?: string;
  /**
   * The validate endpoint said `dependsOn` turns this input off with the values
   * entered: the start ignores it. Said under the field, which stays editable.
   */
  inactive?: boolean;
  /** Unique to the form, so two forms on a page never share an id. */
  idPrefix: string;
}

/** `datetime-local`/`time` carry seconds only with a step under a minute. */
const SECONDS = 1;

export function InputField({ input, value, onChange, error, inactive, idPrefix }: InputFieldProps) {
  const id = `${idPrefix}-${input.id}`;
  const errorId = `${id}-error`;
  const helpId = `${id}-help`;
  const name = input.displayName ?? input.id;
  const help = [helpText(input), inactive ? INACTIVE : null].filter(Boolean).join(" ") || null;
  const describedBy = [help ? helpId : null, error ? errorId : null].filter(Boolean).join(" ") || undefined;
  const aria = { "aria-invalid": error ? true : undefined, "aria-describedby": describedBy };
  const group = input.type === "MULTISELECT" || input.type === "ARRAY";

  const heading = (
    <>
      {name}
      {input.required && !input.hasDefault ? (
        <span className="text-destructive">
          <span aria-hidden>*</span>
          <span className="sr-only">(required)</span>
        </span>
      ) : null}
    </>
  );

  return (
    <div className="grid gap-1.5" data-testid={`run-input-${input.id}`}>
      {group ? (
        <fieldset className="grid gap-1.5" aria-describedby={describedBy}>
          <legend className="mb-1.5 flex items-center gap-1 text-sm font-medium">{heading}</legend>
          {input.type === "MULTISELECT" ? (
            <MultiSelect input={input} id={id} value={value as string[]} onChange={onChange} />
          ) : (
            <ArrayRows input={input} id={id} name={name} value={value as string[]} onChange={onChange} />
          )}
        </fieldset>
      ) : (
        <>
          <Label htmlFor={id} className="gap-1">{heading}</Label>
          <ScalarControl input={input} id={id} value={value} onChange={onChange} aria={aria} />
        </>
      )}
      {help ? (
        <p id={helpId} className="text-xs text-muted-foreground">
          {help}
        </p>
      ) : null}
      {error ? (
        // A refusal of several values has one line per value.
        <p id={errorId} className="text-xs whitespace-pre-line text-destructive">
          {error}
        </p>
      ) : null}
    </div>
  );
}

const INACTIVE = "With the values entered, the inputs it depends on turn this input off, so the run ignores it.";

/** `min`/`max` or `after`/`before` as a sentence, or undefined when the input has none. */
function boundsText(input: RunInput): string | undefined {
  const [low, high, below, above] = ["DATETIME", "DATE", "TIME"].includes(input.type)
    ? [input.after, input.before, "On or after", "On or before"]
    : ["INT", "FLOAT", "DURATION"].includes(input.type)
      ? [input.min, input.max, "At least", "At most"]
      : [];
  if (low !== undefined && high !== undefined) return `From ${low} to ${high}.`;
  if (low !== undefined) return `${below} ${low}.`;
  if (high !== undefined) return `${above} ${high}.`;
  return undefined;
}

/** The input's description, then its bounds and pattern, then what the form cannot do for it. */
function helpText(input: RunInput): string | null {
  const parts = [input.description];
  if (input.type === "DURATION") parts.push("An ISO-8601 duration, such as PT1H or P1D.");
  parts.push(boundsText(input));
  // Checked by the server on Run; Java's regex dialect is not the browser's, so it is not run here.
  if (input.validator) parts.push(`It must match the pattern ${input.validator}`);
  if (input.expressionDefault) parts.push(`Left empty, the flow's default applies: ${input.expressionDefault}`);
  if (input.valuesExpression) parts.push(`Its values come from an expression (${input.valuesExpression}), so type one.`);
  if (input.dependsOn) parts.push("This input depends on other inputs; its values here are not updated from them.");
  if (input.allowedFileExtensions?.length) parts.push(`Accepted: ${input.allowedFileExtensions.join(", ")}`);
  const text = parts.filter(Boolean).join(" ");
  return text || null;
}

type Aria = { "aria-invalid"?: boolean; "aria-describedby"?: string };

function ScalarControl({
  input,
  id,
  value,
  onChange,
  aria,
}: {
  input: RunInput;
  id: string;
  value: FieldValue;
  onChange: (v: FieldValue) => void;
  aria: Aria;
}) {
  const text = typeof value === "string" ? value : "";
  const onText = (e: { target: { value: string } }) => onChange(e.target.value);
  switch (input.type) {
    case "INT":
    case "FLOAT":
      return (
        <Input
          id={id}
          type="number"
          inputMode={input.type === "INT" ? "numeric" : "decimal"}
          step={input.type === "INT" ? 1 : "any"}
          className="tabular-nums"
          value={text}
          onChange={onText}
          {...aria}
        />
      );
    case "BOOL":
      return <Switch id={id} checked={value === true} onCheckedChange={(c) => onChange(c)} {...aria} />;
    case "DATETIME":
      return <Input id={id} type="datetime-local" step={SECONDS} className="tabular-nums" value={text} onChange={onText} {...aria} />;
    case "DATE":
      return <Input id={id} type="date" className="tabular-nums" value={text} onChange={onText} {...aria} />;
    case "TIME":
      return <Input id={id} type="time" step={SECONDS} className="tabular-nums" value={text} onChange={onText} {...aria} />;
    case "DURATION":
      return <Input id={id} placeholder="PT1H" value={text} onChange={onText} {...aria} />;
    case "SECRET":
      return <Input id={id} type="password" autoComplete="off" value={text} onChange={onText} {...aria} />;
    case "EMAIL":
      return <Input id={id} type="email" value={text} onChange={onText} {...aria} />;
    case "URI":
      return <Input id={id} type="url" value={text} onChange={onText} {...aria} />;
    case "SELECT":
      if (input.valuesExpression) return <Input id={id} value={text} onChange={onText} {...aria} />;
      return (
        <Select value={text} onValueChange={(v) => onChange(v)}>
          <SelectTrigger id={id} className="w-full" {...aria}>
            <SelectValue placeholder="Choose a value" />
          </SelectTrigger>
          <SelectContent>
            {input.values.map((v) => (
              <SelectItem key={v} value={v}>
                {v}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      );
    case "JSON":
    case "YAML":
      return (
        <Textarea
          id={id}
          spellCheck={false}
          className="min-h-24 font-mono text-xs"
          value={text}
          onChange={onText}
          {...aria}
        />
      );
    case "FILE":
      return (
        <>
          <Input
            id={id}
            type="file"
            accept={input.allowedFileExtensions?.join(",")}
            onChange={(e) => onChange(e.target.files?.[0] ?? null)}
            {...aria}
          />
          {/* A file input cannot be given a file back, so after the form is reopened it looks empty
              while the chosen file is still in state: say which one will be sent. */}
          {value instanceof File ? <p className="text-xs text-muted-foreground">Chosen: {value.name}</p> : null}
        </>
      );
    default:
      return <Input id={id} value={text} onChange={onText} {...aria} />;
  }
}

function MultiSelect({
  input,
  id,
  value,
  onChange,
}: {
  input: RunInput;
  id: string;
  value: string[];
  onChange: (v: FieldValue) => void;
}) {
  const chosen = new Set(value);
  // A default not among `values` is still shown, so it is never sent unseen.
  const options = [...input.values, ...value.filter((v) => !input.values.includes(v))];
  return (
    <div className="grid gap-2">
      {options.map((option, i) => {
        const optionId = `${id}-${i}`;
        return (
          <div key={option} className="flex items-center gap-2">
            <Checkbox
              id={optionId}
              checked={chosen.has(option)}
              onCheckedChange={(c) =>
                onChange(c === true ? [...value, option] : value.filter((v) => v !== option))
              }
            />
            <Label htmlFor={optionId} className="font-normal">
              {option}
            </Label>
          </div>
        );
      })}
    </div>
  );
}

function ArrayRows({
  input,
  id,
  name,
  value,
  onChange,
}: {
  input: RunInput;
  id: string;
  name: string;
  value: string[];
  onChange: (v: FieldValue) => void;
}) {
  const numeric = input.itemType === "INT" || input.itemType === "FLOAT";
  const setRow = (i: number, v: string) => onChange(value.map((r, j) => (j === i ? v : r)));
  return (
    <div className="grid gap-2">
      {value.map((row, i) => {
        const rowId = `${id}-item-${i}`;
        return (
          // Rows have no identity but their position; removing one re-keys the rest, which is harmless for text fields.
          <div key={i} className="flex items-center gap-2">
            <Label htmlFor={rowId} className="w-6 shrink-0 justify-end text-xs text-muted-foreground tabular-nums">
              <span className="sr-only">{name} item </span>
              {i + 1}
            </Label>
            <Input
              id={rowId}
              type={numeric ? "number" : "text"}
              step={input.itemType === "INT" ? 1 : numeric ? "any" : undefined}
              className={numeric ? "tabular-nums" : undefined}
              value={row}
              onChange={(e) => setRow(i, e.target.value)}
            />
            <IconButton
              aria-label={`Remove ${name} item ${i + 1}`}
              className="size-8"
              onClick={() => onChange(value.filter((_, j) => j !== i))}
            >
              <Trash2 className="size-4" aria-hidden />
            </IconButton>
          </div>
        );
      })}
      <Button type="button" size="sm" variant="outline" className="justify-self-start" onClick={() => onChange([...value, ""])}>
        <Plus aria-hidden />
        Add an item
      </Button>
      {input.itemType ? <p className="text-xs text-muted-foreground">Each item is {input.itemType}.</p> : null}
    </div>
  );
}
