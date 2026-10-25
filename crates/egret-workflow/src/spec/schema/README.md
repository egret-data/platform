# The flow JSON schema's inputs

`GET /api/ws/{ws}/workflows/schema` (W7a spec §7.1) is assembled by
`mod.rs` from three committed files in this directory. This README records
where each comes from and how to reproduce it.

| File | What it is | Written by |
|---|---|---|
| `kestra-flow-schema.json` | Kestra's flow schema, trimmed to what egret can serve | `trim.py`, from the raw export below |
| `egret-task-schemas.json` | egret's own types, and the script tasks' `taskRunner` | hand |
| `property-treatment.json` | how egret treats each property Kestra documents | `measure.rs` (a test), from egret's own code |

## 1. The Kestra export

| | |
|---|---|
| Kestra version | `2.0.0-rc13` (`GET /api/v1/configs` → `version`; OSS, commit `fde6740`, 2026-09-01) — the programme's pinned artifact |
| Image | `kestra/kestra`, untagged, ID `sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd` (created 2026-09-01) |
| Exported | 2026-10-18 (first 2026-10-10; re-exported from the same image on 2026-10-17 when kill-and-exit Task 3 added `io.kestra.plugin.core.execution.Exit` to `KESTRA_TYPES`, and on 2026-10-18 when group C1 Task 1 added the six group-C types) |
| Raw export | 16,533,865 bytes, 1,850 definitions, draft-07, sha256 `68018b24b699e1ee1e9fe68848195c4aadb913cfed9b9c57a35316c97333556c` |
| Committed after `trim.py` | about 778 KB, 115 definitions |

**The raw export is not byte-stable across runs of the same image.** The
2026-10-10 export had the same size and definition count and the sha256
`f29b9ae6930bd2a09dcf5851a7221d21d35c9910d9dbb34ce98d128a013522f8`; the
2026-10-17 one hashes differently. Trimmed, the new export differs from the
committed 2026-10-10 file only by the added `Exit` definition and
`x-kestra-export`'s `kept_definitions` and `raw_sha256` (compared
definition by definition, 2026-10-17).
The 2026-10-18 export hashes differently again. It differs from the
2026-10-17 file only by the six group-C definitions `OutputValues`,
`Assert`, `PurgeLogs`, `SetVariables`, `UnsetVariables` and `Labels`,
plus the same two `x-kestra-export` fields.
So the difference is in what `trim.py` drops, not in any definition it
kept.

The raw export is over 5 MB, so it is not committed; `trim.py` reproduces
`kestra-flow-schema.json` from it byte for byte (the raw file's hash is
recorded inside, under `x-kestra-export`). The Kestra version, image ID and
export date are recorded inside too, under `x-egret-source`; `mod.rs` serves
the version from there (`x-egret-kestra-version`).

```bash
# A throwaway container on a free port. Kestra 2.0 asks for a first user
# before any API answers; any credentials do.
docker run -d --name kestra-schema-export -p 18093:8080 28f59bf175e1 server local
# ... wait until the log says the server is up (about a minute), then:
curl -s -X POST -H 'Content-Type: application/json' localhost:18093/api/v1/basicAuth \
  -d '{"username":"admin@egret.local","password":"W7aAdmin1234","firstName":"a","lastName":"b"}'
AUTH='admin@egret.local:W7aAdmin1234'
curl -s -u "$AUTH" localhost:18093/api/v1/configs            # records the version
curl -s -u "$AUTH" -o raw-flow-schema.json localhost:18093/api/v1/plugins/schemas/flow

# Outputs are not in the flow schema; each class's own documentation has them.
mkdir -p plugin-docs
for t in $(python3 -c 'import trim; print(" ".join(trim.KESTRA_TYPES))'); do
  curl -s -u "$AUTH" -o "plugin-docs/$t.json" "localhost:18093/api/v1/plugins/$t"
done
docker rm -f kestra-schema-export

python3 trim.py raw-flow-schema.json plugin-docs kestra-flow-schema.json \
  2.0.0-rc13 sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd 2026-10-18
```

What `trim.py` does is in its docstring: task and trigger unions become
`#/$defs/egret.task` / `#/$defs/egret.trigger` (which `mod.rs` builds per
workspace), only the flow, the classes egret accepts and what they reference
are kept, `#/definitions/` becomes `#/$defs/`, and each class gets its outputs
as `x-egret-outputs`. It removes no property; that is `mod.rs`'s job.

`trim.py`'s `KESTRA_TYPES` lists the Kestra classes egret accepts.
`tests/flow_schema.rs` fails if egret starts accepting a Kestra class that is
not there.

## 2. The property-treatment measurement (W7a Task 1 Step 2, fix round 1)

Every object the served schema can offer is a *row*: each Kestra class egret
accepts (50), the flow document (`Flow`), and every nested Kestra definition
those reach that egret parses -- the 20 input definitions, `DependsOn`, the
three retry definitions, `Concurrency`, `Check`, `Quota`, both SLA
definitions, `Output`, `Label`, the Flow trigger's `Flow-Dependency` and
`Window`, `Dag-DagTask`, `LoopUntil-CheckFrequency`, `NamespaceFiles`, and the
purge tasks' `behavior` (`kv.Key-2`, `kv.Version-2`, `namespace.Version-2`).

`measure.rs` holds the instrument and its doc has the full rule table. It runs
as the test `spec::schema::measure::the_committed_property_treatment_is_what_egret_does_today`,
which fails when `property-treatment.json` differs from a fresh measurement by
one byte; `EGRET_WRITE_PROPERTY_TREATMENT=1` rewrites the file.

For each property, a minimal flow holding the row's object is built with and
without it and put through three stages:

1. **parse + validate** -- a parse failure or a new error;
2. **the later stage that refuses by name**, where there is one:
   `ScheduleTrigger::resolve` (what `scheduler::sync` runs), and for a
   Webhook `trigger::webhook::declared_unsupported` and `build_output` (the
   409 every firing answers). These are the refusal sites found by grepping
   `trigger/`, `webhook_routes.rs` and the worker handlers for "not
   supported" / "unsupported" / "not implemented" errors. The other hits refuse
   *values* of a property egret honours (cron syntax, window durations,
   `outputFiles` globs, the run-time `secret()` refusal), which a schema cannot
   express, and are not counted;
3. **what egret keeps**:
   * a type row: `models::Task`'s base keys plus the handler's spec struct
     (serde's own field list, `wire_fields_of`), the typed model struct
     (flowables, triggers), `ScriptKind::implemented_properties` (scripts) or
     the catalog (plugins). For a trigger, only the base fields its firing
     path reads: `id`, `type`, `description` and `disabled` everywhere, plus
     `when` and `labels` on Flow. A Schedule refuses `when` (stage 2) and a
     Webhook reads neither, so `labels`/`when` are `ignored` there;
   * a nested row egret models with a struct: whether the key survives
     `parse` and re-serialising the parsed `Flow` -- serde drops every key
     the struct lacks, which is exactly the silent drop to rule out;
   * the purge `behavior` rows, which a handler deserializes
     `deny_unknown_fields` at run time: their fields, anything else refused.

| stage 1 | stage 2 | kept | treatment | served |
|---|---|---|---|---|
| any | new refusal | any | `refused` | no |
| fails | -- | no | `refused` | no |
| new error refusing the property by name (`unsupported_pause_property`) | -- | any | `refused` | no |
| new `not_yet_enforced` warning | -- | any | `warned` | yes |
| clean | clean | yes | `honoured` | yes |
| clean | clean | no | `ignored` | no |

The by-name row (2026-10-11 ruling) exists because a read property whose
sample draws a validation error is otherwise kept `honoured` as a bad sample.
`models::Pause` reads `behavior`, `onPause` and `errors`, but `spec::validate`
refuses each of them whatever its value (`executor::pause_gap` names them), so
they are `refused` and the editor no longer offers them.

**Enum values.** Each served property's `enum` values are tried one by one;
a value that does not parse or that stage 2 refuses is removed from the served
enum, and the property's description says so ("**Not accepted by egret:**").

Two adjustments: the script tasks' `warningOnStdErr` is read and has no effect
(Kestra's own "not used anymore"), and a retry's `warningOnRetry` is read and
nothing in egret acts on it (KI-121), so both are `ignored`.

`warned` is served because it is not silent: the editor shows the warning.
`ignored` is not served, and since KI-121 it is not silent either: a draft
that writes an `ignored` property, or a key its object's row does not list
at all, gets a warning (`spec::property_warnings`: `ignored_property`,
`unknown_property`).

### Passed through unfiltered

* unions (`KvPurgeBehavior`, `FilesPurgeBehavior`, `ReusableInputsInput-2`)
  -- each member is a row of its own;
* `io.kestra.core.models.tasks.common.EncryptedString`, one shape of a
  SECRET input's `defaults`, which egret keeps as an opaque value;
* `java.lang.Void`, empty;
* `ValueOption` and the other definitions reachable only from a property
  that is no longer served (SELECT `values`, the *base 7*, Docker's runner
  internals) are pruned from the document with it.

### The type set and the deployment

Which gated types are offered is decided by the two publish gates
themselves, with the workspace's features **and the deployment's**
`workflow.plugin_configurations` and default runner (`routes::WorkflowState::scripts`):
a script type needs `scripts::gate::check_unrendered` to pass with the
default runner (a runner, and an `allowed-images` entry for it); a plugin
class needs `plugins::gate::check` (both features, a runner, its FQCN in
`allowed-types`). The cache is keyed by that outcome.

### Result

1625 properties over 97 rows: 1119 honoured, 4 warned, 380 ignored, 122 refused (counted from `property-treatment.json`, 2026-10-25, after KI-121 measured the three retry rows' `warningOnRetry` `ignored`; the line said 1512 / 1051 / 335 before, counted 2026-10-17, while the committed file already held 1625 / 1122 / 377).

*base 7* = `assets`, `logLevel`, `logToFile`, `policyRefs`, `taskCache`, `version`, `workerSelector`: `ignored` on core tasks and flowables, `refused` on script and plugin tasks.

| Row | honoured | warned | ignored | refused |
|---|---|---|---|---|
| `Flow` | 14 | `checks`, `concurrency`, `quotas`, `sla` | `deleted`, `draft`, `policyRefs`, `revision`, `updated`, `workerSelector` | — |
| `io.kestra.plugin.core.debug.Return` | 10 | — | *base 7* | — |
| `io.kestra.plugin.core.execution.Exit` | 10 | — | *base 7* | — |
| `io.kestra.plugin.core.execution.Fail` | 11 | — | *base 7* | — |
| `io.kestra.plugin.core.flow.AllowFailure` | 10 | — | *base 7*, `errors`, `finally` | — |
| `io.kestra.plugin.core.flow.Dag` | 12 | — | *base 7*, `finally`, `onChildFailure` | — |
| `io.kestra.plugin.core.flow.If` | 13 | — | *base 7*, `finally` | — |
| `io.kestra.plugin.core.flow.Loop` | 13 | — | *base 7*, `errors`, `fetchType`, `finally`, `outputs` | — |
| `io.kestra.plugin.core.flow.LoopUntil` | 13 | — | *base 7*, `errors`, `finally` | — |
| `io.kestra.plugin.core.flow.Parallel` | 12 | — | *base 7*, `finally`, `onChildFailure` | — |
| `io.kestra.plugin.core.flow.Pause` | 10 | — | *base 7*, `finally`, `onResume` | `behavior`, `errors`, `onPause` |
| `io.kestra.plugin.core.flow.Sequential` | 11 | — | *base 7*, `finally` | — |
| `io.kestra.plugin.core.flow.Sleep` | 10 | — | *base 7* | — |
| `io.kestra.plugin.core.flow.Subflow` | 14 | — | *base 7*, `inheritLabels`, `labels`, `restartBehavior`, `revision`, `scheduleDate` | — |
| `io.kestra.plugin.core.flow.Switch` | 13 | — | *base 7*, `finally` | — |
| `io.kestra.plugin.core.flow.WorkingDirectory` | 11 | — | *base 7*, `cache`, `errors`, `finally`, `inputFiles`, `outputFiles` | — |
| `io.kestra.plugin.core.kv.Delete` | 12 | — | *base 7* | — |
| `io.kestra.plugin.core.kv.Get` | 12 | — | *base 7* | — |
| `io.kestra.plugin.core.kv.GetKeys` | 11 | — | *base 7* | — |
| `io.kestra.plugin.core.kv.PurgeKV` | 14 | — | *base 7* | — |
| `io.kestra.plugin.core.kv.Put` | 13 | — | *base 7* | — |
| `io.kestra.plugin.core.kv.Set` | 16 | — | *base 7* | — |
| `io.kestra.plugin.core.log.Log` | 11 | — | *base 7* | — |
| `io.kestra.plugin.core.namespace.DeleteFiles` | 12 | — | *base 7* | — |
| `io.kestra.plugin.core.namespace.DownloadFiles` | 12 | — | *base 7* | — |
| `io.kestra.plugin.core.namespace.PurgeFiles` | 14 | — | *base 7* | — |
| `io.kestra.plugin.core.namespace.UploadFiles` | 14 | — | *base 7* | — |
| `io.kestra.plugin.core.storage.Concat` | 12 | — | *base 7* | — |
| `io.kestra.plugin.core.storage.DeduplicateItems` | 11 | — | *base 7* | — |
| `io.kestra.plugin.core.storage.Delete` | 11 | — | *base 7* | — |
| `io.kestra.plugin.core.storage.FilterItems` | 13 | — | *base 7* | — |
| `io.kestra.plugin.core.storage.PurgeCurrentExecutionFiles` | 9 | — | *base 7* | — |
| `io.kestra.plugin.core.storage.Reverse` | 12 | — | *base 7* | — |
| `io.kestra.plugin.core.storage.Size` | 10 | — | *base 7* | — |
| `io.kestra.plugin.core.storage.Split` | 15 | — | *base 7* | — |
| `io.kestra.plugin.core.storage.Write` | 11 | — | *base 7* | — |
| `io.kestra.plugin.core.trigger.Flow` | 12 | — | `allowConcurrent`, `assets`, `failOnTriggerError`, `logLevel`, `logToFile`, `policyRefs`, `stopAfter`, `version`, `workerSelector` | — |
| `io.kestra.plugin.core.trigger.Schedule` | 8 | — | `allowConcurrent`, `assets`, `failOnTriggerError`, `labels`, `logLevel`, `logToFile`, `policyRefs`, `recoverMissedSchedules`, `stopAfter`, `version`, `workerSelector` | `lateMaximumDelay`, `when` |
| `io.kestra.plugin.core.trigger.Webhook` | 7 | — | `allowConcurrent`, `assets`, `failOnTriggerError`, `labels`, `logLevel`, `logToFile`, `policyRefs`, `stopAfter`, `version`, `when`, `workerSelector` | `responseCode`, `responseContentType`, `returnOutputs`, `wait` |
| `io.kestra.plugin.scripts.python.Commands` | 20 | — | `warningOnStdErr` | *base 7*, `dependencies`, `dependencyCacheEnabled`, `packageManager`, `pythonVersion`, `runner`, `targetOS`, `uvAutoInstallEnabled`, `uvInstallerSha256`, `uvInstallerVersion` |
| `io.kestra.plugin.scripts.python.Script` | 20 | — | `warningOnStdErr` | *base 7*, `dependencies`, `dependencyCacheEnabled`, `packageManager`, `pythonVersion`, `runner`, `targetOS`, `uvAutoInstallEnabled`, `uvInstallerSha256`, `uvInstallerVersion` |
| `io.kestra.plugin.scripts.shell.Commands` | 20 | — | `warningOnStdErr` | *base 7*, `runner`, `targetOS` |
| `io.kestra.plugin.scripts.shell.Script` | 20 | — | `warningOnStdErr` | *base 7*, `runner`, `targetOS` |
| `kestra.io.kestra.plugin.jdbc.mysql.Batch` | 26 | — | — | *base 7* |
| `kestra.io.kestra.plugin.jdbc.mysql.Queries` | 26 | — | — | *base 7* |
| `kestra.io.kestra.plugin.jdbc.mysql.Query` | 25 | — | — | *base 7* |
| `kestra.io.kestra.plugin.jdbc.postgresql.Batch` | 32 | — | — | *base 7* |
| `kestra.io.kestra.plugin.jdbc.postgresql.CopyIn` | 34 | — | — | *base 7* |
| `kestra.io.kestra.plugin.jdbc.postgresql.CopyOut` | 34 | — | — | *base 7* |
| `kestra.io.kestra.plugin.jdbc.postgresql.Queries` | 31 | — | — | *base 7* |
| `kestra.io.kestra.plugin.jdbc.postgresql.Query` | 30 | — | — | *base 7* |
| `kestra.io.kestra.plugin.jdbc.trino.Query` | 24 | — | — | *base 7* |
| `io.kestra.core.models.Label` | 2 | — | — | — |
| `io.kestra.core.models.flows.Concurrency` | 2 | — | — | — |
| `io.kestra.core.models.flows.DependsOn` | 2 | — | — | — |
| `io.kestra.core.models.flows.Output` | 6 | — | — | — |
| `io.kestra.core.models.flows.check.Check` | 4 | — | — | — |
| `io.kestra.core.models.flows.input.ArrayInput-2` | 9 | — | — | — |
| `io.kestra.core.models.flows.input.BoolInput-2` | 8 | — | — | — |
| `io.kestra.core.models.flows.input.DateInput-2` | 8 | — | `after`, `before` | — |
| `io.kestra.core.models.flows.input.DateTimeInput-2` | 8 | — | `after`, `before` | — |
| `io.kestra.core.models.flows.input.DurationInput-2` | 8 | — | `max`, `min` | — |
| `io.kestra.core.models.flows.input.EmailInput-2` | 8 | — | — | — |
| `io.kestra.core.models.flows.input.FileInput-2` | 9 | — | — | — |
| `io.kestra.core.models.flows.input.FloatInput-2` | 8 | — | `max`, `min` | — |
| `io.kestra.core.models.flows.input.FormInput-2` | 8 | — | `inputs` | — |
| `io.kestra.core.models.flows.input.IntInput-2` | 8 | — | `max`, `min` | — |
| `io.kestra.core.models.flows.input.IonInput-2` | 8 | — | — | — |
| `io.kestra.core.models.flows.input.JsonInput-2` | 8 | — | `jsonSchema` | — |
| `io.kestra.core.models.flows.input.MultiselectInput-2` | 9 | — | `allowCustomValue`, `autoSelectFirst`, `expression`, `values` | — |
| `io.kestra.core.models.flows.input.ReusableInputsInput-1` | 8 | — | `namespace`, `ref`, `revision` | — |
| `io.kestra.core.models.flows.input.SecretInput-2` | 8 | — | `validator` | — |
| `io.kestra.core.models.flows.input.SelectInput-2` | 8 | — | `allowCustomValue`, `autoSelectFirst`, `expression`, `isRadio`, `values` | — |
| `io.kestra.core.models.flows.input.StringInput-2` | 8 | — | `validator` | — |
| `io.kestra.core.models.flows.input.TimeInput-2` | 8 | — | `after`, `before` | — |
| `io.kestra.core.models.flows.input.URIInput-2` | 8 | — | — | — |
| `io.kestra.core.models.flows.input.YamlInput-2` | 8 | — | — | — |
| `io.kestra.core.models.flows.quota.Quota` | 3 | — | — | — |
| `io.kestra.core.models.flows.sla.types.ExecutionAssertionSLA-2` | 5 | — | — | — |
| `io.kestra.core.models.flows.sla.types.MaxDurationSLA-2` | 5 | — | — | — |
| `io.kestra.core.models.tasks.NamespaceFiles` | 6 | — | — | — |
| `io.kestra.core.models.tasks.retrys.Constant-2` | 5 | — | `warningOnRetry` | — |
| `io.kestra.core.models.tasks.retrys.Exponential-2` | 7 | — | `warningOnRetry` | — |
| `io.kestra.core.models.tasks.retrys.Random-2` | 6 | — | `warningOnRetry` | — |
| `io.kestra.core.models.triggers.Window` | 7 | — | — | — |
| `io.kestra.plugin.core.flow.Dag-DagTask` | 2 | — | — | — |
| `io.kestra.plugin.core.flow.LoopUntil-CheckFrequency` | 3 | — | — | — |
| `io.kestra.plugin.core.kv.Key-2` | 2 | — | — | — |
| `io.kestra.plugin.core.kv.Version-2` | 3 | — | — | — |
| `io.kestra.plugin.core.namespace.Version-2` | 3 | — | — | — |
| `io.kestra.plugin.core.trigger.Flow-Dependency` | 5 | — | — | — |

**Refused at a later stage** (`refused_downstream`; parse and validate accept these):

| Row | Property or value | Site |
|---|---|---|
| `io.kestra.plugin.core.trigger.Schedule` | `lateMaximumDelay` | `trigger::schedule::ScheduleTrigger::resolve` |
| `io.kestra.plugin.core.trigger.Schedule` | `when` | `trigger::schedule::ScheduleTrigger::resolve` |
| `io.kestra.plugin.core.trigger.Schedule` | `timezone` = 38 of Kestra's Java short ids (`ACT`, `AET`, … `VST`) | `trigger::schedule::ScheduleTrigger::resolve` (`unknown timezone`) |
| `io.kestra.plugin.core.trigger.Webhook` | `fetchType="STORE"` | `trigger::webhook::build_output` (409 at firing) |
| `io.kestra.plugin.core.trigger.Webhook` | `responseCode` | `trigger::webhook::declared_unsupported` (409 at firing) |
| `io.kestra.plugin.core.trigger.Webhook` | `responseContentType` | `trigger::webhook::declared_unsupported` (409 at firing) |
| `io.kestra.plugin.core.trigger.Webhook` | `returnOutputs` | `trigger::webhook::declared_unsupported` (409 at firing) |
| `io.kestra.plugin.core.trigger.Webhook` | `wait` | `trigger::webhook::declared_unsupported` (409 at firing) |

**Refused enum values** (`refused_values`, removed from the served enum and named in its description): `Schedule.timezone`: 38 values; `Webhook.fetchType`: `STORE`.

## 3. `egret-task-schemas.json`

Hand-written, for what the export cannot describe:

* `egret.sql`, `egret.dataapi.call`, `egret.dataset.refresh`,
  `egret.report.send`, `core.http`, `core.slack` -- egret's own registry
  types. `mod.rs` adds Kestra's schemas of the base task properties
  (`id`, `type`, `description`, `retry`, `timeout`, `disabled`, `runIf`,
  `allowFailure`, `allowWarning`). `egret.sql`'s properties are its handler's
  struct; the other five have no handler in this build, so each offers only the
  field publish resolves (none for `core.http`/`core.slack`), and says so.
* `taskRunner` on the four script tasks: Kestra's definition offers
  `io.kestra.plugin.core.runner.Process` (refused by egret) and lacks
  Kubernetes (Enterprise-only in Kestra, implemented by egret), so it is
  replaced by the two runners and the properties egret implements on each
  (`scripts::DOCKER_IMPLEMENTED`, `scripts::KUBERNETES_IMPLEMENTED`).

## 4. Plugin classes

The nine `plugins/catalog.json` classes are served as `kestra.<FQCN>`. The
catalog decides which properties exist and which are required; Kestra's
definition gives their titles and descriptions; for any property whose `type`
or `enum` differs between the two, the catalog's schema is served
(W7a spec §7.1, *Precedence*). At 2.0.0-rc13 none differs: compared after
removing annotations, the only difference is how `retry`'s three subtypes are
wrapped.

## 5. The served shape of `required` (W7a Task 4)

Every served task and trigger type requires only `id` and `type` outright.
The rest of its required properties sit in `then.required`, behind
`if: { required: [type], properties: { type: { type: string } } }`
(`defer_required_to_type` in `mod.rs`). The documents it accepts are the
same: a type only matches when `type` is one of its string `const`s, and then
the guard holds.

The reason is the editor. `monaco-yaml`'s language service (yaml-language-server)
completes `type: |` from the best-scoring `oneOf` members only, and a missing
required property lowers a member's score. With the plain `required` list,
`type:` completion offered 10 of the 54 task spellings and 2 of the 6 trigger
spellings; with the guard it offers all of them, and "Missing property"
still appears once `type` has a value. The measurement drove `monaco-yaml`
5.5.1's own worker (`yaml.worker.js`) in Node against the served document;
the W7a Task 4 report has the script and both runs.

Everything else in the served document already works with that language
service: draft 2020-12, `$defs`, and `allOf` wrappers (it has no `$ref` with
sibling keywords; the language service would rewrite one into `allOf` itself).
