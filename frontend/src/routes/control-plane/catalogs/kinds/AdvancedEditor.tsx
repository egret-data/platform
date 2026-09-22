import { useState } from "react";
import { ChevronDown, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

/** `Advanced` (`config.rs:252-258`): a `BTreeMap<String,String>` per engine. */
export type AdvancedValue = {
  trino?: Record<string, string>;
  starrocks?: Record<string, string>;
};

/** One refused advanced key, read back off the server's own sentence. */
export type AdvancedIssue = { engine: string; key: string; text: string };

const ENGINES = ["trino", "starrocks"] as const;

/**
 * Pull the engine and key out of a 400 that names an advanced property.
 *
 * Three of `ValidationError`'s messages start `advanced.<engine>.<key> …`
 * (`Collision`, `PlaintextSecret`, and the generic kind's "is required"), and
 * all three are about one key the user can see and fix. Parsed here so the
 * page can put the sentence next to that key instead of dropping a raw 400
 * body at the top of a long form. A message that does not match is left to the
 * page's own error banner — guessing is worse than showing the server's words.
 */
export function parseAdvancedIssue(message: string): AdvancedIssue | null {
  const m = /advanced\.(trino|starrocks)\.(\S+)/.exec(message);
  if (!m) return null;
  return { engine: m[1], key: m[2].replace(/[.,;:]$/, ""), text: message };
}

/**
 * Free-form properties per engine, written after everything the typed form
 * above sets.
 *
 * Kept per engine because that is how they are stored and rendered: Trino
 * reads a catalog properties file, StarRocks reads `CREATE EXTERNAL CATALOG`
 * properties, and the same concept has a different key in each.
 */
export function AdvancedEditor({
  value,
  onChange,
  requiredKeys,
  issues = [],
  defaultOpen = false,
}: {
  value: AdvancedValue;
  onChange: (v: AdvancedValue) => void;
  /** Keys this kind cannot be saved without, per engine (the generic kind). */
  requiredKeys?: Record<string, string>;
  issues?: AdvancedIssue[];
  defaultOpen?: boolean;
}) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <Collapsible open={open} onOpenChange={setOpen} className="rounded-lg border">
      <CollapsibleTrigger className="group flex w-full items-center justify-between p-4 text-sm font-medium">
        Advanced properties
        <ChevronDown className="h-4 w-4 text-muted-foreground transition-transform group-data-[state=open]:rotate-180" />
      </CollapsibleTrigger>
      <CollapsibleContent className="space-y-4 border-t p-4">
        <p className="text-xs text-muted-foreground">
          Written after the properties the form above sets. A key the form already sets is
          refused, and a value that looks like a credential is refused — store it as a secret
          and reference its id.
        </p>
        {ENGINES.map((engine) => (
          <EngineProperties
            key={engine}
            engine={engine}
            required={requiredKeys?.[engine]}
            value={value[engine] ?? {}}
            issues={issues.filter((i) => i.engine === engine)}
            onChange={(next) => onChange({ ...value, [engine]: next })}
          />
        ))}
      </CollapsibleContent>
    </Collapsible>
  );
}

function EngineProperties({
  engine,
  value,
  onChange,
  required,
  issues,
}: {
  engine: string;
  value: Record<string, string>;
  onChange: (next: Record<string, string>) => void;
  required?: string;
  issues: AdvancedIssue[];
}) {
  const [newKey, setNewKey] = useState("");
  const keys = Object.keys(value);
  const rows = required && !keys.includes(required) ? [required, ...keys] : keys;
  const unplaced = issues.filter((i) => !rows.includes(i.key));
  return (
    <div className="space-y-2 rounded-md border p-3">
      <p className="text-sm font-medium">{engine}</p>
      {required && (
        <p className="text-xs text-muted-foreground">
          <code>{required}</code> is required for this kind.
        </p>
      )}
      {rows.length === 0 ? (
        <p className="text-xs text-muted-foreground">No properties.</p>
      ) : (
        rows.map((k) => {
          const issue = issues.find((i) => i.key === k);
          return (
            <div key={k} className="space-y-1">
              <div className="flex items-end gap-2">
                <div className="min-w-0 flex-1 space-y-1.5">
                  <Label htmlFor={`cc-adv-${engine}-${k}`}>{k}</Label>
                  <Input
                    id={`cc-adv-${engine}-${k}`}
                    value={value[k] ?? ""}
                    aria-invalid={issue ? true : undefined}
                    onChange={(e) => onChange({ ...value, [k]: e.target.value })}
                  />
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  aria-label={`Remove ${engine} property ${k}`}
                  onClick={() => {
                    const next = { ...value };
                    delete next[k];
                    onChange(next);
                  }}
                >
                  <Trash2 className="h-4 w-4" />
                </Button>
              </div>
              {issue && <p className="text-xs text-destructive">{issue.text}</p>}
            </div>
          );
        })
      )}
      {unplaced.map((i) => (
        <p key={i.key} className="text-xs text-destructive">
          {i.text}
        </p>
      ))}
      <div className="flex items-end gap-2">
        <div className="min-w-0 flex-1 space-y-1.5">
          <Label htmlFor={`cc-adv-${engine}-newkey`}>{engine} property name</Label>
          <Input
            id={`cc-adv-${engine}-newkey`}
            value={newKey}
            placeholder="e.g. hive.max-partitions-per-scan"
            onChange={(e) => setNewKey(e.target.value)}
          />
        </div>
        <Button
          type="button"
          variant="outline"
          aria-label={`Add ${engine} property`}
          disabled={newKey.trim() === "" || newKey.trim() in value}
          onClick={() => {
            onChange({ ...value, [newKey.trim()]: "" });
            setNewKey("");
          }}
        >
          Add
        </Button>
      </div>
    </div>
  );
}
