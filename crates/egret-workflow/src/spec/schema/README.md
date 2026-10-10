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
| Exported | 2026-10-10 |
| Raw export | 16,533,865 bytes, 1,850 definitions, draft-07, sha256 `f29b9ae6930bd2a09dcf5851a7221d21d35c9910d9dbb34ce98d128a013522f8` |
| Committed after `trim.py` | about 715 KB, 108 definitions |

The raw export is over 5 MB, so it is not committed; `trim.py` reproduces
`kestra-flow-schema.json` from it byte for byte (the raw file's hash is
recorded inside, under `x-kestra-export`).

```bash
# A throwaway container on a free port. Kestra 2.0 asks for a first user
# before any API answers; any credentials do.
docker run -d --name w7a-kestra-t1 -p 18093:8080 28f59bf175e1 server local
# ... wait until the log says the server is up (about a minute), then:
curl -s -X POST -H 'Content-Type: application/json' localhost:18093/api/v1/basicAuth \
  -d '{"username":"admin@w7a.local","password":"W7aAdmin1234","firstName":"a","lastName":"b"}'
AUTH='admin@w7a.local:W7aAdmin1234'
curl -s -u "$AUTH" localhost:18093/api/v1/configs            # records the version
curl -s -u "$AUTH" -o raw-flow-schema.json localhost:18093/api/v1/plugins/schemas/flow

# Outputs are not in the flow schema; each class's own documentation has them.
mkdir -p plugin-docs
for t in $(python3 -c 'import trim; print(" ".join(trim.KESTRA_TYPES))'); do
  curl -s -u "$AUTH" -o "plugin-docs/$t.json" "localhost:18093/api/v1/plugins/$t"
done
docker rm -f w7a-kestra-t1

python3 trim.py raw-flow-schema.json plugin-docs kestra-flow-schema.json
```

What `trim.py` does is in its docstring: task and trigger unions become
`#/$defs/egret.task` / `#/$defs/egret.trigger` (which `mod.rs` builds per
workspace), only the flow, the classes egret accepts and what they reference
are kept, `#/definitions/` becomes `#/$defs/`, and each class gets its outputs
as `x-egret-outputs`. It removes no property; that is `mod.rs`'s job.

`trim.py`'s `KESTRA_TYPES` lists the Kestra classes egret accepts.
`tests/flow_schema.rs` fails if egret starts accepting a Kestra class that is
not there.

## 2. The property-treatment measurement (W7a Task 1 Step 2)

For every type egret accepts that has a Kestra definition (50 classes), and
for the flow document itself, each property Kestra documents is classified
by egret's own code. `measure.rs` holds the instrument; it runs as the test
`spec::schema::measure::the_committed_property_treatment_is_what_egret_does_today`,
which fails when `property-treatment.json` no longer matches, and rewrites it
with `EGRET_WRITE_PROPERTY_TREATMENT=1`.

**What egret reads (the read set).** The base properties `models::Task`'s
deserializer takes; for a core leaf task, the fields of the struct its handler
deserializes the leaf spec into, listed by serde itself (a deserializer that
records the field list `deserialize_struct` receives); for a flowable or a
trigger, the fields of its typed model struct the same way, plus the trigger
base fields something outside `models/` reads; for a script task
`ScriptKind::implemented_properties`; for a plugin class, its catalog
properties minus the refused base ones; for the flow, `models::Flow`'s fields.
None of those structs denies unknown fields, so for leaf tasks this is the
complete answer to "honoured or ignored": a key outside the handler's struct
is dropped by serde, without a word.

**What parse + validate say.** A minimal flow carrying the property (with a
sample value) is parsed and validated, and compared with the same flow
without it.

| parse + validate | in the read set | treatment | served |
|---|---|---|---|
| parse fails, or a new error | no | `refused` | no |
| a new `not_yet_enforced` warning | either | `warned` | yes |
| clean | yes | `honoured` | yes |
| clean | no | `ignored` | no |

Two adjustments, both named in `measure.rs`: the script tasks'
`warningOnStdErr` is read and has no effect (Kestra's own "not used anymore"),
so it is `ignored`; and the four trigger base fields `models::Trigger` parses
that nothing reads (`stopAfter`, `logToFile`, `failOnTriggerError`,
`allowConcurrent`) are outside the read set, so they come out `ignored`.

`warned` is served because it is not silent: the editor shows the warning
beside the property. `refused` is not served because completion would
suggest a save error.

### Result

1223 properties over 51 rows (50 classes and the flow document): 799 honoured, 8 warned, 303 ignored, 113 refused.

The seven base properties egret's `Task` model does not read (`assets`, `logLevel`, `logToFile`, `policyRefs`, `taskCache`, `version`, `workerSelector`) are `ignored` on every core task and flowable and `refused` on script and plugin tasks; the table lists them as *base 7*.

| Type | honoured | warned | ignored | refused |
|---|---|---|---|---|
| `Flow` | 13 | `checks`, `concurrency`, `quotas`, `sla`, `triggers` | `deleted`, `draft`, `policyRefs`, `revision`, `updated`, `workerSelector` | — |
| `io.kestra.plugin.core.debug.Return` | 10 | — | *base 7* | — |
| `io.kestra.plugin.core.execution.Fail` | 11 | — | *base 7* | — |
| `io.kestra.plugin.core.flow.AllowFailure` | 10 | — | *base 7*, `errors`, `finally` | — |
| `io.kestra.plugin.core.flow.Dag` | 12 | — | *base 7*, `finally`, `onChildFailure` | — |
| `io.kestra.plugin.core.flow.If` | 13 | — | *base 7*, `finally` | — |
| `io.kestra.plugin.core.flow.Loop` | 13 | — | *base 7*, `errors`, `fetchType`, `finally`, `outputs` | — |
| `io.kestra.plugin.core.flow.LoopUntil` | 13 | — | *base 7*, `errors`, `finally` | — |
| `io.kestra.plugin.core.flow.Parallel` | 12 | — | *base 7*, `finally`, `onChildFailure` | — |
| `io.kestra.plugin.core.flow.Pause` | 10 | `behavior`, `errors`, `onPause` | *base 7*, `finally`, `onResume` | — |
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
| `io.kestra.plugin.core.trigger.Schedule` | 11 | — | `allowConcurrent`, `assets`, `failOnTriggerError`, `logLevel`, `logToFile`, `policyRefs`, `recoverMissedSchedules`, `stopAfter`, `version`, `workerSelector` | — |
| `io.kestra.plugin.core.trigger.Webhook` | 13 | — | `allowConcurrent`, `assets`, `failOnTriggerError`, `logLevel`, `logToFile`, `policyRefs`, `stopAfter`, `version`, `workerSelector` | — |
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
