#!/usr/bin/env python3
"""Trim Kestra's exported flow JSON schema to what egret can serve.

Reproduces `kestra-flow-schema.json` from the raw export; see README.md for
the export commands. Usage:

    python3 trim.py RAW_FLOW_SCHEMA PLUGIN_DOCS_DIR OUT_JSON \
        KESTRA_VERSION IMAGE_ID EXPORT_DATE

* RAW_FLOW_SCHEMA is `GET /api/v1/plugins/schemas/flow` (draft-07, about
  16 MB at 2.0.0-rc13).
* KESTRA_VERSION, IMAGE_ID and EXPORT_DATE are recorded as the file's
  `x-egret-source` (W7a spec §7.1: "Its Kestra version is recorded in the
  file"); `GET /api/v1/configs` gives the version.
* PLUGIN_DOCS_DIR holds `GET /api/v1/plugins/{type}` for each type in
  KESTRA_TYPES, one `{type}.json` per type; only its `schema.outputs` is read,
  for `x-egret-outputs`.

What it does, in order:

1. Every `anyOf` that is a union of task types (its members are all `$ref`s
   to types listed in the `Flow` definition's `tasks`) becomes a `$ref` to
   `#/$defs/egret.task`; the same for trigger unions and `egret.trigger`.
   egret builds those two definitions itself, per workspace feature set
   (`spec::schema`), so a nested `tasks:` offers what egret accepts and not
   Kestra's 1349 classes.
2. Only the `Flow` definition, the definitions of KESTRA_TYPES and what they
   reach through `$ref` are kept.
3. `#/definitions/` becomes `#/$defs/` (draft 2020-12).
4. Each kept type definition gets `x-egret-outputs` from its plugin doc.

No property is removed here. Which properties egret serves is decided in
Rust (`spec::schema`) from `property-treatment.json`, so the filter has one
writer and this file stays a faithful copy of Kestra's definitions.
"""

import hashlib
import json
import os
import sys

# The Kestra classes egret accepts (registry rows, the flowables of
# `models::task::kind_for`, the typed triggers of `models::trigger::kind_for`
# and the nine `plugins/catalog.json` classes). `tests/flow_schema.rs` fails
# if egret accepts a Kestra FQCN that is not kept here.
KESTRA_TYPES = [
    # registry.rs: core leaf tasks
    "io.kestra.plugin.core.log.Log",
    "io.kestra.plugin.core.debug.Return",
    "io.kestra.plugin.core.execution.Fail",
    # registry.rs: executor-level, no handler (kill-and-exit Task 3)
    "io.kestra.plugin.core.execution.Exit",
    "io.kestra.plugin.core.flow.Sleep",
    "io.kestra.plugin.core.storage.Write",
    "io.kestra.plugin.core.storage.Size",
    "io.kestra.plugin.core.storage.Delete",
    "io.kestra.plugin.core.storage.PurgeCurrentExecutionFiles",
    "io.kestra.plugin.core.storage.Concat",
    "io.kestra.plugin.core.storage.Split",
    "io.kestra.plugin.core.storage.Reverse",
    "io.kestra.plugin.core.storage.DeduplicateItems",
    "io.kestra.plugin.core.storage.FilterItems",
    "io.kestra.plugin.core.namespace.UploadFiles",
    "io.kestra.plugin.core.namespace.DownloadFiles",
    "io.kestra.plugin.core.namespace.DeleteFiles",
    "io.kestra.plugin.core.namespace.PurgeFiles",
    "io.kestra.plugin.core.kv.Set",
    "io.kestra.plugin.core.kv.Get",
    "io.kestra.plugin.core.kv.GetKeys",
    "io.kestra.plugin.core.kv.Delete",
    "io.kestra.plugin.core.kv.Put",
    "io.kestra.plugin.core.kv.PurgeKV",
    # registry.rs: the four script tasks
    "io.kestra.plugin.scripts.shell.Commands",
    "io.kestra.plugin.scripts.shell.Script",
    "io.kestra.plugin.scripts.python.Commands",
    "io.kestra.plugin.scripts.python.Script",
    # models/task.rs: flowables
    "io.kestra.plugin.core.flow.Sequential",
    "io.kestra.plugin.core.flow.Parallel",
    "io.kestra.plugin.core.flow.If",
    "io.kestra.plugin.core.flow.Switch",
    "io.kestra.plugin.core.flow.Dag",
    "io.kestra.plugin.core.flow.Loop",
    "io.kestra.plugin.core.flow.LoopUntil",
    "io.kestra.plugin.core.flow.Subflow",
    "io.kestra.plugin.core.flow.AllowFailure",
    "io.kestra.plugin.core.flow.WorkingDirectory",
    "io.kestra.plugin.core.flow.Pause",
    # models/trigger.rs: typed triggers
    "io.kestra.plugin.core.trigger.Schedule",
    "io.kestra.plugin.core.trigger.Webhook",
    "io.kestra.plugin.core.trigger.Flow",
    # plugins/catalog.json
    "io.kestra.plugin.jdbc.mysql.Batch",
    "io.kestra.plugin.jdbc.mysql.Queries",
    "io.kestra.plugin.jdbc.mysql.Query",
    "io.kestra.plugin.jdbc.postgresql.Batch",
    "io.kestra.plugin.jdbc.postgresql.CopyIn",
    "io.kestra.plugin.jdbc.postgresql.CopyOut",
    "io.kestra.plugin.jdbc.postgresql.Queries",
    "io.kestra.plugin.jdbc.postgresql.Query",
    "io.kestra.plugin.jdbc.trino.Query",
]

FLOW = "io.kestra.core.models.flows.Flow"
REF = "#/definitions/"


def ref_name(node):
    """The definition a union member names: a bare `$ref`, or Kestra's
    `{"allOf": [{"$ref": ..}, {annotations}]}` wrapper around one."""
    if not isinstance(node, dict):
        return None
    if set(node) == {"$ref"} and node["$ref"].startswith(REF):
        return node["$ref"][len(REF):]
    parts = node.get("allOf")
    if set(node) == {"allOf"} and isinstance(parts, list) and len(parts) == 2:
        name = ref_name(parts[0])
        if name and isinstance(parts[1], dict) and "$ref" not in json.dumps(parts[1]):
            return name
    return None


def union_refs(node, key):
    members = node.get(key)
    if not isinstance(members, list) or len(members) < 2:
        return None
    names = [ref_name(m) for m in members]
    if any(n is None for n in names):
        return None
    return set(names)


def collapse(node, tasks, triggers, stats):
    """Step 1, recursively."""
    if isinstance(node, list):
        return [collapse(v, tasks, triggers, stats) for v in node]
    if not isinstance(node, dict):
        return node
    for key in ("anyOf", "oneOf"):
        names = union_refs(node, key)
        if names is None:
            continue
        for target, universe in (("egret.task", tasks), ("egret.trigger", triggers)):
            if names <= universe:
                stats.setdefault(target, []).append(len(names))
                rest = {k: collapse(v, tasks, triggers, stats) for k, v in node.items() if k != key}
                rest["$ref"] = "#/$defs/" + target
                return rest
    return {k: collapse(v, tasks, triggers, stats) for k, v in node.items()}


def refs_in(node, out):
    if isinstance(node, dict):
        target = node.get("$ref")
        if isinstance(target, str) and target.startswith(REF):
            out.add(target[len(REF):])
        for v in node.values():
            refs_in(v, out)
    elif isinstance(node, list):
        for v in node:
            refs_in(v, out)


def rename_refs(node):
    if isinstance(node, dict):
        return {
            k: (
                "#/$defs/" + v[len(REF):]
                if k == "$ref" and isinstance(v, str) and v.startswith(REF)
                else rename_refs(v)
            )
            for k, v in node.items()
        }
    if isinstance(node, list):
        return [rename_refs(v) for v in node]
    return node


def outputs_of(docs_dir, type_name):
    path = os.path.join(docs_dir, type_name + ".json")
    if not os.path.exists(path):
        return None
    with open(path) as f:
        doc = json.load(f)
    props = ((doc.get("schema") or {}).get("outputs") or {}).get("properties") or {}
    out = []
    for name in sorted(props):
        p = props[name]
        entry = {"name": name}
        if isinstance(p.get("type"), str):
            entry["type"] = p["type"]
        text = p.get("title") or p.get("description")
        if text:
            entry["description"] = text
        out.append(entry)
    return out


def main(raw_path, docs_dir, out_path, version, image_id, export_date):
    with open(raw_path, "rb") as f:
        raw_bytes = f.read()
    raw = json.loads(raw_bytes)
    defs = raw["definitions"]
    flow = defs[FLOW]
    tasks = {ref_name(m) for m in flow["properties"]["tasks"]["items"]["anyOf"]}
    triggers = {ref_name(m) for m in flow["properties"]["triggers"]["items"]["anyOf"]}

    missing = [t for t in KESTRA_TYPES if t not in defs]
    if missing:
        sys.exit("not in the export: " + ", ".join(missing))

    stats = {}
    collapsed = {name: collapse(d, tasks, triggers, stats) for name, d in defs.items()}

    keep = set()
    todo = [FLOW] + KESTRA_TYPES
    while todo:
        name = todo.pop()
        if name in keep:
            continue
        keep.add(name)
        found = set()
        refs_in(collapsed[name], found)
        todo.extend(n for n in found if n not in keep and n in collapsed)

    kept = {}
    for name in sorted(keep):
        if name == FLOW:
            continue
        d = rename_refs(collapsed[name])
        if name in KESTRA_TYPES:
            outs = outputs_of(docs_dir, name)
            if outs is not None:
                d["x-egret-outputs"] = outs
        kept[name] = d

    def type_names(universe):
        names = set()
        for n in universe:
            t = defs[n].get("properties", {}).get("type", {})
            if "const" in t:
                names.add(t["const"])
            names.update(t.get("enum", []))
        return sorted(names)

    out = {
        "$comment": "Generated by trim.py from Kestra's flow schema export; see README.md. "
        "Do not edit by hand.",
        "x-egret-source": {
            "kestra_version": version,
            "image_id": image_id,
            "exported_at": export_date,
            "export": "GET /api/v1/plugins/schemas/flow",
        },
        "x-kestra-export": {
            "raw_sha256": hashlib.sha256(raw_bytes).hexdigest(),
            "raw_bytes": len(raw_bytes),
            "raw_definitions": len(defs),
            "kept_definitions": len(kept),
            "collapsed_unions": {k: len(v) for k, v in sorted(stats.items())},
        },
        "x-kestra-types": {"tasks": type_names(tasks), "triggers": type_names(triggers)},
        "flow": rename_refs(collapsed[FLOW]),
        "$defs": kept,
    }
    with open(out_path, "w") as f:
        json.dump(out, f, indent=1, sort_keys=True, ensure_ascii=False)
        f.write("\n")


if __name__ == "__main__":
    if len(sys.argv) != 7:
        sys.exit(__doc__)
    main(*sys.argv[1:])
