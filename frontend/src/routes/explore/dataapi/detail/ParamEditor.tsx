import { useRef, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Checkbox } from "@/components/ui/checkbox";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { ParamLocation, ParamType, type ParamSpec } from "@/api/generated/model";

const PARAM_TYPES: ParamType[] = [
  ParamType.string,
  ParamType.number,
  ParamType.boolean,
  ParamType.date,
  ParamType.timestamp,
  ParamType.enum,
];

function newParam(): ParamSpec {
  // Every parameter is `query` — there is no location control here. See the
  // module doc below for why offering one would be worse than omitting it.
  return { name: "", type: ParamType.string, location: ParamLocation.query, required: false };
}

/**
 * The comma-separated "allowed values" field for an `enum` parameter.
 *
 * Deliberately uncontrolled between keystrokes: an earlier version was
 * `value={allowed.join(", ")}` with an `onChange` that immediately re-split,
 * trimmed and `filter(Boolean)`-ed on every keystroke. That normalizes a
 * trailing comma away the instant it's typed — existing value `"a"`, type
 * `,` → `"a,"` → split/trim/filter → back to `["a"]` → rendered as `"a"` →
 * the keystroke is silently eaten, and a second value can never be started
 * by typing. This field instead holds the raw text locally and only projects
 * it to `string[]` `onBlur`, which is also where it re-formats the display
 * (`"a,b"` → `"a, b"`) so a typo in spacing doesn't linger once the field is
 * left.
 */
function AllowedValuesField({
  id,
  allowed,
  onChange,
}: {
  id: string;
  allowed: string[];
  onChange: (allowed: string[]) => void;
}) {
  const [text, setText] = useState(() => allowed.join(", "));

  function commit() {
    const projected = text
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    onChange(projected);
    setText(projected.join(", "));
  }

  return (
    <Input
      id={id}
      placeholder="comma-separated"
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
    />
  );
}

/**
 * The `:name` parameters a caller supplies on the query string.
 *
 * **Deliberately no `location` control** (spec §2.6). Every parameter this API
 * can serve is `query`: `ParamLocation::Body` is refused at publish time, and
 * `serve_post` (the POST handler) has no body extractor to read one from. A
 * picker that offers `body` and then has publish reject it is a control that
 * can drift out of sync with the refusal it's describing — omitting the
 * control keeps the two impossible to disagree.
 *
 * Rows are keyed by a stable, locally-generated id — never the array index.
 * `ParamSpec` has no server-assigned id to key on, but an index breaks the
 * moment a row in the middle is removed: React re-associates each surviving
 * row's DOM node with the NEXT row's data, which just relocates a controlled
 * `<Input>`'s value (it's fully re-rendered either way) but leaves a Radix
 * `Select`'s open/focus state and this file's own `AllowedValuesField` local
 * text attached to the wrong row. The whole editor also remounts fresh
 * (`EndpointDetail` keys the `Outlet` by endpoint id) whenever the author
 * navigates to a different endpoint, so these ids never need to survive that.
 */
export function ParamEditor({
  params,
  onChange,
}: {
  params: ParamSpec[];
  onChange: (params: ParamSpec[]) => void;
}) {
  // Existing rows keep their index as a stable id for the component's whole
  // lifetime — assigned once here, at mount, and never regenerated from
  // `params` again. `nextRowId` only hands out ids for rows added AFTER
  // mount (via `add`, an event handler — mutating a ref there is fine; doing
  // it inside a `useState` lazy initializer is not, since that runs during
  // render). Starting it at `params.length` guarantees a new row's id can
  // never collide with one of the initial ones.
  const nextRowId = useRef(params.length);
  const [rowIds, setRowIds] = useState<number[]>(() => params.map((_, i) => i));

  function update(i: number, patch: Partial<ParamSpec>) {
    onChange(params.map((p, j) => (j === i ? { ...p, ...patch } : p)));
  }
  function remove(i: number) {
    onChange(params.filter((_, j) => j !== i));
    setRowIds((ids) => ids.filter((_, j) => j !== i));
  }
  function add() {
    const rowId = nextRowId.current++;
    onChange([...params, newParam()]);
    setRowIds((ids) => [...ids, rowId]);
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-base">Parameters</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        {params.length === 0 && (
          <p className="text-sm text-muted-foreground">
            No parameters yet. Every parameter is read from the query string — write it into the SQL
            as <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">:name</code>.
          </p>
        )}
        {params.map((p, i) => {
          const rowId = rowIds[i] ?? i;
          return (
            <div key={rowId} className="space-y-3 rounded-md border border-border p-3">
              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1.5">
                  <Label htmlFor={`param-name-${rowId}`}>Name</Label>
                  <Input
                    id={`param-name-${rowId}`}
                    value={p.name}
                    onChange={(e) => update(i, { name: e.target.value })}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor={`param-type-${rowId}`}>Type</Label>
                  <Select value={p.type} onValueChange={(v) => update(i, { type: v as ParamType })}>
                    <SelectTrigger id={`param-type-${rowId}`} aria-label={`Parameter ${i + 1} type`}>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {PARAM_TYPES.map((t) => (
                        <SelectItem key={t} value={t}>
                          {t}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1.5">
                  <Label htmlFor={`param-default-${rowId}`}>Default</Label>
                  <Input
                    id={`param-default-${rowId}`}
                    value={p.default ?? ""}
                    onChange={(e) => update(i, { default: e.target.value === "" ? null : e.target.value })}
                  />
                </div>
                {p.type === ParamType.enum && (
                  <div className="space-y-1.5">
                    <Label htmlFor={`param-allowed-${rowId}`}>Allowed values</Label>
                    <AllowedValuesField
                      id={`param-allowed-${rowId}`}
                      allowed={p.allowed ?? []}
                      onChange={(allowed) => update(i, { allowed })}
                    />
                  </div>
                )}
              </div>

              <div className="space-y-1.5">
                <Label htmlFor={`param-desc-${rowId}`}>Description</Label>
                <Input
                  id={`param-desc-${rowId}`}
                  value={p.description ?? ""}
                  onChange={(e) => update(i, { description: e.target.value })}
                />
              </div>

              <div className="flex items-center justify-between">
                <label htmlFor={`param-required-${rowId}`} className="flex cursor-pointer items-center gap-2 text-sm">
                  <Checkbox
                    id={`param-required-${rowId}`}
                    checked={!!p.required}
                    onCheckedChange={(v) => update(i, { required: v === true })}
                  />
                  Required
                </label>
                <Button type="button" variant="ghost" size="sm" onClick={() => remove(i)}>
                  <Trash2 className="h-4 w-4" /> Remove
                </Button>
              </div>
            </div>
          );
        })}
        <Button type="button" variant="outline" size="sm" onClick={add}>
          <Plus className="h-4 w-4" /> Add parameter
        </Button>
      </CardContent>
    </Card>
  );
}
