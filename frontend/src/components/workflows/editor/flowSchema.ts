/**
 * Reading the flow schema `GET …/workflows/schema` serves (spec §7.1) for the
 * Docs tab. The shape is `crates/egret-workflow/src/spec/schema/mod.rs`'s:
 * - task and trigger types are `$defs` entries, listed by the unions
 *   `$defs["egret.task"].oneOf` and `$defs["egret.trigger"].oneOf`;
 * - a type's spellings (canonical first, then aliases) are the `const`s of
 *   its `properties.type`;
 * - a type requires `id` and `type` outright and the rest in `then.required`
 *   (behind `if: type is a string`, so that `monaco-yaml` completes every
 *   `type:` spelling);
 * - outputs are `x-egret-outputs: [{ name, type, description }]`.
 */
export type JsonSchema = { [key: string]: unknown };
export type FlowSchema = JsonSchema;

export type PropertyDoc = { name: string; type: string; required: boolean; default: string; description: string };
export type OutputDoc = { name: string; type: string; description: string };
export type TypeDoc = {
  /** The canonical type name, or "Flow" for the flow itself. */
  name: string;
  /** The alias the source used, when it is not the canonical name. */
  spelledAs: string | null;
  title: string | null;
  description: string | null;
  properties: PropertyDoc[];
  /** Null for the flow, which has no outputs of its own to document here. */
  outputs: OutputDoc[] | null;
};

const asObject = (v: unknown): JsonSchema | null =>
  v !== null && typeof v === "object" && !Array.isArray(v) ? (v as JsonSchema) : null;
const asArray = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const asString = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);

const defsOf = (schema: FlowSchema) => asObject(schema.$defs) ?? {};
const refTarget = (ref: unknown) => (typeof ref === "string" ? ref.replace(/^#\/\$defs\//, "") : null);

/** `#/$defs/io.kestra.core.models.tasks.retrys.Constant-2` → `Constant`. */
function refLabel(ref: string): string {
  const last = ref.split("/").pop() ?? ref;
  return (last.split(".").pop() ?? last).replace(/-\d+$/, "");
}

/** The spellings a type definition accepts for `type:`. */
function spellings(def: JsonSchema): string[] {
  const type = asObject(asObject(def.properties)?.type);
  if (!type) return [];
  if (typeof type.const === "string") return [type.const];
  return asArray(type.anyOf).flatMap((m) => {
    const c = asObject(m)?.const;
    return typeof c === "string" ? [c] : [];
  });
}

/** The task or trigger definition `spelling` names, by canonical name or alias. */
export function findTypeDef(schema: FlowSchema, spelling: string): { name: string; def: JsonSchema } | null {
  const defs = defsOf(schema);
  for (const union of ["egret.task", "egret.trigger"]) {
    for (const member of asArray(asObject(defs[union])?.oneOf)) {
      const name = refTarget(asObject(member)?.$ref);
      const def = name ? asObject(defs[name]) : null;
      if (name && def && spellings(def).includes(spelling)) return { name, def };
    }
  }
  return null;
}

/** A short type label for a property schema: `string`, `string | array`, `Constant`. */
export function typeLabel(prop: JsonSchema): string {
  if (typeof prop.$ref === "string") return refLabel(prop.$ref);
  if (typeof prop.type === "string") return prop.type;
  if (Array.isArray(prop.type)) return prop.type.join(" | ");
  if (prop.const !== undefined) return JSON.stringify(prop.const);
  for (const key of ["anyOf", "oneOf"]) {
    const members = asArray(prop[key]).map((m) => asObject(m)).filter((m): m is JsonSchema => m !== null);
    if (members.length > 0) return [...new Set(members.map(typeLabel))].join(" | ");
  }
  const parts = asArray(prop.allOf).map((m) => asObject(m)).filter((m): m is JsonSchema => m !== null);
  const labelled = parts.find((p) => p.$ref !== undefined || p.type !== undefined || p.anyOf !== undefined);
  if (labelled) return typeLabel(labelled);
  if (asObject(prop.properties)) return "object";
  return "any";
}

const firstString = (prop: JsonSchema, keys: string[]) => {
  for (const k of keys) {
    const s = asString(prop[k]);
    if (s) return s;
  }
  return null;
};

/** A property's one-line description; its `allOf` wrapper's title counts. */
function propertyDescription(prop: JsonSchema): string {
  const own = firstString(prop, ["title", "description", "markdownDescription"]);
  if (own) return own;
  for (const key of ["allOf", "anyOf", "oneOf"]) {
    for (const m of asArray(prop[key])) {
      const inner = asObject(m);
      if (inner) {
        const d = propertyDescription(inner);
        if (d) return d;
      }
    }
  }
  return "";
}

function properties(def: JsonSchema): PropertyDoc[] {
  const required = new Set(
    [...asArray(def.required), ...asArray(asObject(def.then)?.required)].filter(
      (r): r is string => typeof r === "string",
    ),
  );
  const props = Object.entries(asObject(def.properties) ?? {}).map(([name, raw]): PropertyDoc => {
    const prop = asObject(raw) ?? {};
    return {
      name,
      type: typeLabel(prop),
      required: required.has(name),
      default:
        prop.default === undefined
          ? ""
          : typeof prop.default === "string"
            ? prop.default
            : JSON.stringify(prop.default),
      description: propertyDescription(prop),
    };
  });
  // Required first; within each group, by name.
  return props.sort((a, b) => Number(b.required) - Number(a.required) || a.name.localeCompare(b.name));
}

function outputs(def: JsonSchema): OutputDoc[] {
  return asArray(def["x-egret-outputs"]).flatMap((o) => {
    const out = asObject(o);
    const name = out && asString(out.name);
    return name
      ? [{ name, type: asString(out.type) ?? "", description: asString(out.description) ?? "" }]
      : [];
  });
}

/** The documentation of the type `spelling` names, or null when the schema does not know it. */
export function typeDoc(schema: FlowSchema, spelling: string): TypeDoc | null {
  const hit = findTypeDef(schema, spelling);
  if (!hit) return null;
  const { name, def } = hit;
  return {
    name,
    spelledAs: spelling === name ? null : spelling,
    title: asString(def.title),
    description: firstString(def, ["markdownDescription", "description"]),
    properties: properties(def),
    outputs: outputs(def),
  };
}

/** The flow's own documentation: the document's top-level properties. */
export function flowDoc(schema: FlowSchema): TypeDoc {
  return {
    name: "Flow",
    spelledAs: null,
    title: asString(schema.title),
    description: firstString(schema, ["markdownDescription", "description"]),
    properties: properties(schema),
    outputs: null,
  };
}
