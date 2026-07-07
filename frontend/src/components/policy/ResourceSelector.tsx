import { useEffect, useMemo } from "react";
import { type ResourceLevel, type ServiceDefResource } from "@/api/policies";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ResourceLevelEditor } from "@/components/policy/ResourceLevelEditor";

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
}

const NONE = "__none__";

const emptyLevel = (): ResourceLevel => ({ values: [] });

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
 */
export function ResourceSelector({ resourceDefs, service, value, onChange, onLeafChange }: ResourceSelectorProps) {
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

  // Switching the type occupying position `index` drops every key in the
  // chain from `index` onward and installs `newName` with a fresh empty
  // value, truncating whatever sub-chain existed below the old type.
  const switchTypeAt = (index: number, newName: string) => {
    const next = { ...value };
    for (let i = index; i < chain.length; i++) delete next[chain[i]];
    next[newName] = value[newName] ?? emptyLevel();
    onChange(next);
  };

  // Extends the chain by adding the given child type as the new leaf.
  const extendChain = (childName: string) => onChange({ ...value, [childName]: value[childName] ?? emptyLevel() });

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

  return (
    <div className="space-y-3">
      {chain.map((name, index) => {
        const validOptions = index === 0 ? roots : childrenOfName(chain[index - 1]);
        const def = byName.get(name) ?? { name };
        return (
          <div key={`${index}-${name}`} className="space-y-1.5 rounded-md border p-3">
            {validOptions.length > 1 ? (
              <div className="flex items-center gap-2">
                <span className="text-xs text-muted-foreground">Type</span>
                <Select value={name} onValueChange={(v) => switchTypeAt(index, v)}>
                  <SelectTrigger size="sm" className="h-7 w-44" aria-label={`Resource type at level ${index + 1}`}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {validOptions.map((o) => (
                      <SelectItem key={o.name} value={o.name}>
                        {labelFor(o.name)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            ) : (
              <span className="text-xs text-muted-foreground">{labelFor(name)}</span>
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
              invalid={!!def.mandatory && (value[name]?.values.length ?? 0) === 0}
            />
          </div>
        );
      })}

      {canExtend && (
        <div className="flex items-center gap-2">
          <span className="text-xs text-muted-foreground">Add level</span>
          <Select value={NONE} onValueChange={(v) => v !== NONE && extendChain(v)}>
            <SelectTrigger size="sm" className="h-7 w-44" aria-label="Add resource level">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE}>None (stop here)</SelectItem>
              {leafChildren.map((c) => (
                <SelectItem key={c.name} value={c.name}>
                  {labelFor(c.name)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      )}
    </div>
  );
}
