import { useEffect, useMemo } from "react";
import { TriangleAlert } from "lucide-react";
import { type ResourceLevel, type ServiceDefResource } from "@/api/policies";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ResourceLevelEditor } from "@/components/policy/ResourceLevelEditor";
import { grantsSelect, levelGaps, stopsAboveColumn } from "@/components/policy/resourceLevels";

export interface ResourceSelectorProps {
  /** The applicable resources from the service-def: `def.resources`, `def.dataMaskDef.resources`, or `def.rowFilterDef.resources`. */
  resourceDefs: ServiceDefResource[];
  /** Service name, forwarded to `resourceLookup` for value autocomplete. */
  service: string;
  /** The generic resources map, e.g. `{ catalog: {...}, schema: {...} }` or `{ role: {...} }`. */
  value: Record<string, ResourceLevel>;
  onChange: (value: Record<string, ResourceLevel>) => void;
  /** Reports the current chain's leaf resource type name (or null if empty), so the editor can restrict access-types. */
  onLeafChange?: (leafResourceName: string | null) => void;
  /**
   * The access types the policy grants (an access policy only). When they
   * include `select` (or `all`) and the chain stops above `column`, the
   * selector warns that no column read is granted. Omit for masking and
   * row-filter policies, which grant no access types.
   */
  grantedAccessTypes?: string[];
}

const NONE = "__none__";

const emptyLevel = (): ResourceLevel => ({ values: [] });
/** Default value for a level added via "Add level" — a wildcard, not empty,
 * so a freshly-extended chain isn't immediately flagged "Required". */
const defaultNewLevel = (): ResourceLevel => ({ values: ["*"], isExcludes: false, isRecursive: false });

/** Friendly labels for resource type names that don't read well merely capitalized. */
const LABELS: Record<string, string> = {
  trinouser: "Trino user",
  sysinfo: "Sys info",
  systemproperty: "System property",
  sessionproperty: "Session property",
  schemafunction: "Schema function",
  queryid: "Query ID",
};

function labelFor(name: string): string {
  return LABELS[name] ?? name.charAt(0).toUpperCase() + name.slice(1);
}

/**
 * Def-driven cascading resource-type + value picker, matching Ranger's
 * `ResourceComp`: at each level, a resource-TYPE selector (its options are
 * the children of the previous level's chosen type, or the hierarchy roots
 * at level 0) followed by a value control. When only one type is valid at a
 * level it's shown as a static label instead of a dropdown. Switching a
 * level's type truncates the chain below it; a "None (stop here)" entry on
 * the trailing type selector lets the chain stop at a valid leaf instead of
 * being extended into its children.
 *
 * Any non-root level whose *parent* is itself a valid stopping point
 * (`parentDef.isValidLeaf`) also gets a "— none —" entry in its own Type
 * select — Ranger allows a policy to stop at that parent, so picking
 * "— none —" truncates the chain there and every level below it.
 */
export function ResourceSelector({
  resourceDefs,
  service,
  value,
  onChange,
  onLeafChange,
  grantedAccessTypes,
}: ResourceSelectorProps) {
  const byName = useMemo(() => {
    const m = new Map<string, ServiceDefResource>();
    for (const r of resourceDefs) m.set(r.name, r);
    return m;
  }, [resourceDefs]);

  const roots = useMemo(() => resourceDefs.filter((r) => !r.parent), [resourceDefs]);
  const childrenOf = useMemo(() => {
    const m = new Map<string, ServiceDefResource[]>();
    for (const r of resourceDefs) {
      if (!r.parent) continue;
      const list = m.get(r.parent) ?? [];
      list.push(r);
      m.set(r.parent, list);
    }
    return m;
  }, [resourceDefs]);
  const childrenOfName = (name: string): ServiceDefResource[] => childrenOf.get(name) ?? [];

  // Derive the ordered chain of resource-type names (root → leaf) from
  // `value`'s keys, by walking the hierarchy and picking whichever sibling
  // at each step is present as a key in `value`. If `value` is empty, start
  // the chain at "catalog" (or the first root) with an empty value.
  const chain: string[] = useMemo(() => {
    const keys = Object.keys(value);
    const result: string[] = [];
    let options = roots;
    while (options.length > 0) {
      let chosen = options.find((o) => keys.includes(o.name));
      if (!chosen && result.length === 0) {
        chosen = options.find((o) => o.name === "catalog") ?? options[0];
      }
      if (!chosen) break;
      result.push(chosen.name);
      options = childrenOfName(chosen.name);
      if (options.length === 0) break;
      if (!options.some((o) => keys.includes(o.name))) break;
    }
    return result;
  }, [value, roots, childrenOf]);

  const leaf = chain.length > 0 ? chain[chain.length - 1] : null;

  useEffect(() => {
    onLeafChange?.(leaf);
  }, [leaf, onLeafChange]);

  const setResourceValue = (name: string, lvl: ResourceLevel) => onChange({ ...value, [name]: lvl });

  // Drops every key in the chain from `index` onward, truncating whatever
  // sub-chain existed at and below that position.
  const truncateAt = (index: number) => {
    const next = { ...value };
    for (let i = index; i < chain.length; i++) delete next[chain[i]];
    onChange(next);
  };

  // Switching the type occupying position `index` drops every key in the
  // chain from `index` onward and installs `newName` with a fresh empty
  // value, truncating whatever sub-chain existed below the old type.
  const switchTypeAt = (index: number, newName: string) => {
    const next = { ...value };
    for (let i = index; i < chain.length; i++) delete next[chain[i]];
    next[newName] = value[newName] ?? emptyLevel();
    onChange(next);
  };

  // Extends the chain by adding the given child type as the new leaf,
  // defaulting to a wildcard so the freshly-added level isn't immediately
  // flagged "Required".
  const extendChain = (childName: string) =>
    onChange({ ...value, [childName]: value[childName] ?? defaultNewLevel() });

  // Parent values (catalog/schema/table) visible to `ResourceLevelEditor`'s
  // lookup autocomplete for the level at chain position `index`.
  const parentsFor = (index: number): { catalog?: string; schema?: string; table?: string } => {
    const parents: { catalog?: string; schema?: string; table?: string } = {};
    for (let j = 0; j < index; j++) {
      const k = chain[j];
      if (k === "catalog" || k === "schema" || k === "table") {
        parents[k] = value[k]?.values[0];
      }
    }
    return parents;
  };

  const leafDef = leaf ? byName.get(leaf) : undefined;
  const leafChildren = leaf ? childrenOfName(leaf) : [];
  const canExtend = leafChildren.length > 0 && (leafDef?.isValidLeaf ?? true);

  const gaps = levelGaps(resourceDefs, value);
  const stoppedAt = grantsSelect(grantedAccessTypes) ? stopsAboveColumn(resourceDefs, value) : null;

  return (
    <div className="space-y-2">
      {chain.map((name, index) => {
        const validOptions = index === 0 ? roots : childrenOfName(chain[index - 1]);
        const parentDef = index > 0 ? byName.get(chain[index - 1]) : undefined;
        // Stopping at the parent is a legal Ranger policy (its def marks it a
        // valid leaf), so this level's Type select gets a "— none —" entry
        // that truncates the chain back up to (and not including) here.
        // Never offered at the root — a policy needs at least one resource.
        const showNone = index > 0 && parentDef?.isValidLeaf === true;
        const def = byName.get(name) ?? { name };
        const empty = (value[name]?.values.length ?? 0) === 0;
        const levelName = labelFor(name).toLowerCase();
        // The root cannot be left empty (`*` would cover every one this
        // profile serves); any deeper level can, and is saved as `*`.
        const rootMessage = `Name a ${levelName}. It is required: * here would cover every ${levelName} this profile serves.`;
        const emptyHint =
          index > 0 && empty
            ? `Empty means every ${levelName} — saved as *.${
                name === "column" && grantsSelect(grantedAccessTypes)
                  ? " Column SELECT is then granted on every column."
                  : ` The policy then covers every ${levelName} under the levels above.`
              }`
            : undefined;
        // One compact row per level: the Type (a select when the level admits
        // more than one type, else a static label) + the value control inline,
        // so a full catalog→schema→table→column chain (and any additional
        // resources) stays short instead of a stack of tall bordered blocks.
        return (
          <div key={`${index}-${name}`} className="flex items-center gap-2">
            {validOptions.length > 1 || showNone ? (
              <Select
                value={name}
                onValueChange={(v) => (v === NONE ? truncateAt(index) : switchTypeAt(index, v))}
              >
                <SelectTrigger
                  size="sm"
                  className="h-9 w-32 shrink-0"
                  aria-label={`Resource type at level ${index + 1}`}
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {validOptions.map((o) => (
                    <SelectItem key={o.name} value={o.name}>
                      {labelFor(o.name)}
                    </SelectItem>
                  ))}
                  {showNone && <SelectItem value={NONE}>— none —</SelectItem>}
                </SelectContent>
              </Select>
            ) : (
              <span className="w-32 shrink-0 text-sm font-medium">{labelFor(name)}</span>
            )}
            <ResourceLevelEditor
              label={labelFor(name)}
              level={name}
              service={service}
              parents={parentsFor(index)}
              value={value[name] ?? emptyLevel()}
              onChange={(lvl) => setResourceValue(name, lvl)}
              excludesSupported={def.excludesSupported}
              recursiveSupported={def.recursiveSupported}
              mandatory={def.mandatory}
              invalid={index === 0 && !!def.mandatory && empty}
              invalidMessage={rootMessage}
              hint={emptyHint}
            />
          </div>
        );
      })}

      {gaps.map((g) => (
        <p key={g.level} role="alert" className="text-xs text-destructive">
          {labelFor(g.level)} is set but {labelFor(g.missing).toLowerCase()} is not, so Ranger cannot
          match this policy. Set {labelFor(g.missing).toLowerCase()} (use * for every value), or clear{" "}
          {labelFor(g.level).toLowerCase()}.
        </p>
      ))}

      {stoppedAt && (
        <p className="flex items-start gap-1.5 text-xs text-muted-foreground">
          <TriangleAlert className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden />
          <span>
            Stops above column: grants no column reads, only table-level checks such as{" "}
            <code>count(*)</code>. Add the column level (use * for every column) to grant SELECT.
          </span>
        </p>
      )}

      {canExtend && (
        <Select value={NONE} onValueChange={(v) => v !== NONE && extendChain(v)}>
          <SelectTrigger
            size="sm"
            className="h-8 w-40 border-dashed text-muted-foreground"
            aria-label="Add resource level"
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NONE}>+ Add level…</SelectItem>
            {leafChildren.map((c) => (
              <SelectItem key={c.name} value={c.name}>
                {labelFor(c.name)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
    </div>
  );
}
