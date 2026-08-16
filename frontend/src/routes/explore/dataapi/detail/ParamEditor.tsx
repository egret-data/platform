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
 * The `:name` parameters a caller supplies on the query string.
 *
 * **Deliberately no `location` control** (spec §2.6). Every parameter this API
 * can serve is `query`: `ParamLocation::Body` is refused at publish time, and
 * `serve_post` (the POST handler) has no body extractor to read one from. A
 * picker that offers `body` and then has publish reject it is a control that
 * can drift out of sync with the refusal it's describing — omitting the
 * control keeps the two impossible to disagree.
 */
export function ParamEditor({
  params,
  onChange,
}: {
  params: ParamSpec[];
  onChange: (params: ParamSpec[]) => void;
}) {
  function update(i: number, patch: Partial<ParamSpec>) {
    onChange(params.map((p, j) => (j === i ? { ...p, ...patch } : p)));
  }
  function remove(i: number) {
    onChange(params.filter((_, j) => j !== i));
  }
  function add() {
    onChange([...params, newParam()]);
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
        {params.map((p, i) => (
          <div key={i} className="space-y-3 rounded-md border border-border p-3">
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1.5">
                <Label htmlFor={`param-name-${i}`}>Name</Label>
                <Input id={`param-name-${i}`} value={p.name} onChange={(e) => update(i, { name: e.target.value })} />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor={`param-type-${i}`}>Type</Label>
                <Select value={p.type} onValueChange={(v) => update(i, { type: v as ParamType })}>
                  <SelectTrigger id={`param-type-${i}`} aria-label={`Parameter ${i + 1} type`}>
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
                <Label htmlFor={`param-default-${i}`}>Default</Label>
                <Input
                  id={`param-default-${i}`}
                  value={p.default ?? ""}
                  onChange={(e) => update(i, { default: e.target.value === "" ? null : e.target.value })}
                />
              </div>
              {p.type === ParamType.enum && (
                <div className="space-y-1.5">
                  <Label htmlFor={`param-allowed-${i}`}>Allowed values</Label>
                  <Input
                    id={`param-allowed-${i}`}
                    placeholder="comma-separated"
                    value={(p.allowed ?? []).join(", ")}
                    onChange={(e) =>
                      update(i, {
                        allowed: e.target.value
                          .split(",")
                          .map((s) => s.trim())
                          .filter(Boolean),
                      })
                    }
                  />
                </div>
              )}
            </div>

            <div className="space-y-1.5">
              <Label htmlFor={`param-desc-${i}`}>Description</Label>
              <Input
                id={`param-desc-${i}`}
                value={p.description ?? ""}
                onChange={(e) => update(i, { description: e.target.value })}
              />
            </div>

            <div className="flex items-center justify-between">
              <label htmlFor={`param-required-${i}`} className="flex cursor-pointer items-center gap-2 text-sm">
                <Checkbox
                  id={`param-required-${i}`}
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
        ))}
        <Button type="button" variant="outline" size="sm" onClick={add}>
          <Plus className="h-4 w-4" /> Add parameter
        </Button>
      </CardContent>
    </Card>
  );
}
