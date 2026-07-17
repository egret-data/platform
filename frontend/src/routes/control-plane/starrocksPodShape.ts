/**
 * Pure converters bridging the stored `starrocksConfig.pod` JSON (k8s-flavored)
 * to/from the structured editor shapes reused from egret's Trino config
 * primitives (`Kv`, `TolerationForm`, `ContainerForm`). No React here — these
 * are called by the PodContainersSection to prefill editors and to serialize
 * back for Save.
 *
 * Degradation contract (requirement: never throw, never silently corrupt):
 *   - If a whole value is malformed (e.g. `pod.env` is a string, `pod.tolerations`
 *     is null), the *ToRows/*ToKv reader returns `[]`. The CALLER is responsible
 *     for keeping the original raw value when the reader yields no rows (so a
 *     malformed block isn't clobbered by an empty serialize). These readers only
 *     promise to not throw and to lose no data through THIS layer.
 *   - Container unknown/extra keys (anything the ContainerForm doesn't model) are
 *     round-tripped through the `rawOverride` field, so container blocks survive
 *     verbatim. A non-object list entry is preserved whole via `rawOverride`.
 *   - Serializers omit empty fields (empty string / empty array / empty record)
 *     so a seed -> no-edit -> serialize cycle is equal under canonical key order
 *     (configDiff compares key-order-insensitively) and isn't spuriously dirty.
 */

import type { Kv } from "./EngineResourceGroups";
// `kvToRecord` is the single source of truth for Kv -> Record (trim key, drop
// blank keys); re-exported so there's exactly one implementation.
import { kvToRecord } from "./EngineResourceGroups";
import type { ContainerForm } from "./config/ContainersMountsSection";
import type { TolerationForm } from "./useEngineConfigForm";

export { kvToRecord };

// ── narrowing helpers ────────────────────────────────────────────────────────

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function asString(v: unknown): string {
  return typeof v === "string" ? v : "";
}

function isStringArray(v: unknown): v is string[] {
  return Array.isArray(v) && v.every((x) => typeof x === "string");
}

/**
 * True when every entry is a *simple* k8s env var — `{name, value}` strings and
 * nothing else. A `valueFrom` (secretKeyRef, etc.) or any extra key means the
 * array can't be losslessly folded into a `Record<string,string>`, so such an
 * array is preserved verbatim via the container `rawOverride` instead.
 */
function isSimpleEnvArray(v: unknown): boolean {
  return (
    Array.isArray(v) &&
    v.every(
      (e) =>
        isPlainObject(e) &&
        typeof e.name === "string" &&
        typeof e.value === "string" &&
        Object.keys(e).length === 2,
    )
  );
}

// ── env: [{name,value}] <-> Kv[] (Kv.key = name, Kv.value = value) ────────────

/**
 * k8s `env` array -> `Kv[]`. Preserves order. Returns `[]` for any non-array
 * (malformed) input; the caller keeps the raw value in that case.
 *
 * NOTE this pod-level Environment editor is name/value only by design — a k8s
 * `valueFrom` (secretKeyRef, etc.) is not representable here and is not handled
 * at this level. (Container env, by contrast, preserves `valueFrom` verbatim via
 * the container `rawOverride`; see `jsonToContainerForm`.)
 */
export function envJsonToKv(env: unknown): Kv[] {
  if (!Array.isArray(env)) return [];
  const out: Kv[] = [];
  for (const e of env) {
    if (!isPlainObject(e)) continue;
    out.push({ key: asString(e.name), value: asString(e.value) });
  }
  return out;
}

/**
 * `Kv[]` -> k8s `env` array. Trims keys and drops blank-key rows (matches
 * `kvToRecord`). Empty in -> `[]` out.
 */
export function kvToEnvJson(rows: Kv[]): Array<{ name: string; value: string }> {
  const out: Array<{ name: string; value: string }> = [];
  for (const row of rows) {
    const name = row.key.trim();
    if (name) out.push({ name, value: row.value });
  }
  return out;
}

// ── record <-> Kv[] (nodeSelector / podLabels / annotations) ─────────────────

/**
 * `Record<string,string>` -> `Kv[]`, iterating `Object.entries` insertion order
 * (matches `EngineResourceGroups.recordToKv`; no sort). Returns `[]` for any
 * non-object (malformed) input rather than throwing.
 */
export function recordToKv(rec: unknown): Kv[] {
  if (!isPlainObject(rec)) return [];
  return Object.entries(rec).map(([key, value]) => ({
    key,
    value: asString(value),
  }));
}

// `kvToRecord` re-exported above from EngineResourceGroups (single source).

// ── tolerations: [{...}] <-> TolerationForm[] ────────────────────────────────

/**
 * k8s `tolerations` array -> `TolerationForm[]`. `tolerationSeconds` (a JSON
 * number, or absent) becomes a form string ("" when absent). Preserves order.
 * Returns `[]` for any non-array input.
 */
export function tolerationsJsonToRows(tol: unknown): TolerationForm[] {
  if (!Array.isArray(tol)) return [];
  const out: TolerationForm[] = [];
  for (const t of tol) {
    if (!isPlainObject(t)) continue;
    out.push({
      key: asString(t.key),
      operator: asString(t.operator),
      value: asString(t.value),
      effect: asString(t.effect),
      tolerationSeconds:
        typeof t.tolerationSeconds === "number"
          ? String(t.tolerationSeconds)
          : "",
    });
  }
  return out;
}

/**
 * `TolerationForm[]` -> k8s `tolerations` array. Omits empty fields (so an
 * unset `value` isn't written as `value:""`) and only emits a numeric
 * `tolerationSeconds` when the form string parses to a finite number. Keeps the
 * canonical key order (key, operator, value, effect, tolerationSeconds).
 */
export function rowsToTolerationsJson(rows: TolerationForm[]): unknown[] {
  return rows.map((row) => {
    const out: Record<string, unknown> = {};
    if (row.key) out.key = row.key;
    if (row.operator) out.operator = row.operator;
    if (row.value) out.value = row.value;
    if (row.effect) out.effect = row.effect;
    const secs = row.tolerationSeconds.trim();
    if (secs !== "") {
      const n = Number(secs);
      if (Number.isFinite(n)) out.tolerationSeconds = n;
    }
    return out;
  });
}

// ── containers: [k8s Container] <-> ContainerForm[] ──────────────────────────

function jsonToContainerForm(c: unknown): ContainerForm {
  // Non-object entry: preserve whole thing verbatim via rawOverride.
  if (!isPlainObject(c)) {
    return { name: "", image: "", rawOverride: c };
  }

  const rest: Record<string, unknown> = { ...c };
  const form: ContainerForm = {
    name: asString(rest.name),
    image: asString(rest.image),
  };
  delete rest.name;
  delete rest.image;

  if (isStringArray(rest.command)) {
    form.command = rest.command;
    delete rest.command;
  }
  if (isStringArray(rest.args)) {
    form.args = rest.args;
    delete rest.args;
  }
  // env: only fold into the editable Record when EVERY entry is a simple
  // name/value pair. An array containing `valueFrom` (secretKeyRef, etc.) or any
  // extra key is left in `rest` so it round-trips verbatim via rawOverride.
  if (isSimpleEnvArray(rest.env)) {
    form.env = kvToRecord(envJsonToKv(rest.env));
    if (Object.keys(form.env).length === 0) delete form.env;
    delete rest.env;
  }
  // volumeMounts: pass the element objects through verbatim (preserves any
  // extra per-mount keys) when it's an array of objects.
  if (Array.isArray(rest.volumeMounts) && rest.volumeMounts.every(isPlainObject)) {
    form.volumeMounts = rest.volumeMounts as unknown as ContainerForm["volumeMounts"];
    delete rest.volumeMounts;
  }

  // Whatever survives in `rest` is either unmodeled OR a malformed/unconsumed
  // modeled key (e.g. `env: "x"`, `command: ["sh", 1]`). Both flow to
  // rawOverride so no container data is dropped.
  if (Object.keys(rest).length > 0) form.rawOverride = rest;

  return form;
}

function containerFormToJson(f: ContainerForm): unknown {
  // A non-object override means the source entry wasn't an object; return it
  // verbatim ("raw override wins").
  if (f.rawOverride !== undefined && !isPlainObject(f.rawOverride)) {
    return f.rawOverride;
  }

  const out: Record<string, unknown> = isPlainObject(f.rawOverride)
    ? { ...f.rawOverride }
    : {};
  out.name = f.name;
  out.image = f.image;
  if (f.command && f.command.length) out.command = f.command;
  if (f.args && f.args.length) out.args = f.args;
  if (f.env) {
    const env = Object.entries(f.env).map(([name, value]) => ({ name, value }));
    if (env.length) out.env = env;
  }
  if (f.volumeMounts && f.volumeMounts.length) out.volumeMounts = f.volumeMounts;
  return out;
}

/**
 * k8s init/sidecar container array -> `ContainerForm[]`. Converts each
 * container's k8s `env` array to the form's `Record<string,string>` and routes
 * unmodeled keys through `rawOverride`. Returns `[]` for a non-array input.
 */
export function containersJsonToForms(list: unknown): ContainerForm[] {
  if (!Array.isArray(list)) return [];
  return list.map(jsonToContainerForm);
}

/**
 * `ContainerForm[]` -> k8s container array. Re-merges `rawOverride` (unmodeled
 * keys) with the structured fields, converts `env` back to `[{name,value}]`,
 * and omits empty command/args/env/volumeMounts.
 */
export function formsToContainersJson(rows: ContainerForm[]): unknown[] {
  return rows.map(containerFormToJson);
}
