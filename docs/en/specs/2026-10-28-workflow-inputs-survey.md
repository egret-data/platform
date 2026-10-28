# Survey for roadmap package #3, "Inputs checked at start"

Date: 2026-10-28. Read-only survey. Paths:

- `K/` = `/Users/1pro-l4r8/Documents/github/data-portal/kestra` (checkout at `96893b58`, `version=2.0.0-SNAPSHOT`, 2026-08-17)
- `E/` = `/Users/1pro-l4r8/Documents/github/data-portal/egret-inputs` (HEAD `ff25210b`)
- `KC/` = `K/core/src/main/java/io/kestra/core`
- `EW/` = `E/crates/egret-workflow/src`

**Caveat on versions (not verified).** The golden comes from the pinned image
`kestra/kestra@sha256:28f59bf1…` (`E/crates/egret-workflow/oracle-exec/docker-compose.yml:31`).
The Kestra source checkout is a later snapshot. FORM, `ValueOption {label,value}`, `prefill`, the
"defaults ⇒ required" rule, `renderError` and `allowedFileExtensions` may postdate the pinned
image. I did not check which of them the pinned image has.

---

## 1. Kestra: the 20 input types

### 1.0 Fields every type has (`KC/models/flows/Input.java:55-98`)

| field | type | default | notes |
|---|---|---|---|
| `id` | String | — | `@NotNull @NotBlank @Pattern("^[a-zA-Z0-9][.a-zA-Z0-9_-]*")` (:59-62). A dotted id nests (`MapUtils.flattenToNestedMap`, FlowInputOutput:262) |
| `type` | `Type` enum | — | `@NotNull` (:67-69) |
| `description` | String | null | |
| `dependsOn` | `DependsOn(List<String> inputs, String condition)` | null | `KC/models/flows/DependsOn.java:15-17`. There is no top-level `condition` key: the condition lives in `dependsOn.condition` |
| `required` | Boolean | `true` (`@Builder.Default`, :81-82) | |
| `defaults` | `Property<T>` | null | rendered (Pebble) at start |
| `prefill` | `Property<T>` | null | UI hint only. It is never read in `FlowInputOutput` |
| `displayName` | String | null | |

The 20 subtypes are registered at `Input.java:30-53`: ARRAY, BOOL, DATE, DATETIME, DURATION, FILE, FLOAT, INT, JSON,
ION, SECRET, STRING, SELECT, TIME, URI, MULTISELECT, YAML, EMAIL, FORM, REUSABLE_INPUTS.

### 1.1 Validation when a flow is saved (not at run time). For context: some of these checks are absent in egret

| check | message | where |
|---|---|---|
| `defaults` set and `required: false` | `Inputs with a default value must be required, since the default is always applied.` | `KC/validations/validator/InputValidator.java:45-51` |
| both `defaults` and `prefill` | `Inputs with a default value cannot also have a prefill.` | InputValidator.java:53-59 |
| FORM with defaults/prefill; nested FORM | `A FORM input groups other inputs and cannot declare a default value or a prefill.` / `A FORM input cannot contain another FORM input; …` | InputValidator.java:28-43 |
| ARRAY/MULTISELECT `itemType` ∈ {ARRAY, SECRET, MULTISELECT, SELECT} | `` `itemType` cannot be X `` | ArrayInputValidator.java:22-32, MultiselectInputValidator.java:22-32 |
| FILE `defaults` whose scheme is not `file`/`nsfile` | `inputs of type 'FILE' only support \`defaults\` as local files using a file URI or as namespace files using a nsfile URI` (and `… with expression that can be rendered immediately`) | FileInputValidator.java:33-50 |
| STRING/SECRET `validator` does not compile with `java.util.regex` | `invalid pattern [<v>]` | `KC/validations/validator/RegexValidator.java:26-32` (`@Regex` on StringInput.java:22, SecretInput.java:23) |
| duplicate ids / duplicate dotted paths / prefix conflict | `Duplicate input with name [..]`, `Duplicate input path [..]`, `Input path '%s' conflicts with '%s'; …` | `KC/validations/validator/FlowValidator.java:79-98` |
| `dependsOn` names an unknown input, or the dependencies form a cycle | `Input with id '%s' depends on a non-existent input '%s'.` / `Cycle dependency detected for input with id '%s': %s` | FlowValidator.java:241-270 |
| EE-only input type | `Input '<id>' of type REUSABLE_INPUTS is only available in Enterprise Edition.` | FlowValidator.java:195-212 |
| a Schedule leaves out an input that has no default | `Missing inputs for Schedule Trigger '%s', missing inputs: '%s'` | FlowValidator.java:284-312 |
| `{{ inputs.key-name }}` with a dash | `Invalid input reference: use inputs[key-name] …` | FlowValidator.java:121-137 |

### 1.2 What happens at run time: `FlowInputOutput.resolveInputs` (`KC/runners/FlowInputOutput.java`)

For each leaf input, in declared order, after `expandToLeaves` (FORM is flattened and REUSABLE_INPUTS is expanded, :291-293):

1. **dependsOn** (:321-345). The listed inputs are resolved first, recursively (:471-482). An unknown dependency id is silently ignored (:479). The input is *enabled* only when every dependency is enabled **and** non-null (:326-327). If `dependsOn.condition` is set, it is rendered with `renderTyped`, and only `Boolean.TRUE` enables the input (:329-332). A failed render gives the error `Invalid condition: <msg>`, and the input is disabled (:333-338). A disabled input is dropped from the result (`filter(InputAndValue::enabled)`, :239).
2. **Render the definition** (:349-362). For a SELECT or MULTISELECT, `expression` is rendered. A failure is a render error, `Invalid value for input \`id\`. Cause: Cannot render 'expression'. Cause: …`, or `… Invalid expression result. Expected a list of values` (SelectInput.java:108-133, MultiselectInput.java:118-143).
3. **Empty string** (:366-369). `""` is treated as absent for every type except STRING, SELECT, EMAIL and SECRET (`isTextType`, :437-440).
4. **Defaults** (:373-382). These apply only when the value is null. The default is rendered and typed per type (`resolveDefaultValue`, :413-429; ARRAY and MULTISELECT go through `asList`). A failure is a render error. `isDefault` is set to true. **The default then goes through parse and validate like a posted value.**
5. **Required** (:385-390). If the value is null and `required` is true, the error is `Missing required input:<id>`. The message has no space after the colon and no "Invalid value" prefix. If `required` is false, the value is null and the key is still present in the result map.
6. **Parse** with `parseType` (:538-610), then **validate** with `Input.validate(parsed)` (:392-402).

**How error messages are built.** This matters for exact-text parity:

- (a) A violation from `validate()` gives `Invalid value for input \`<id>\`. Cause: <violation message>` (`InputOutputValidationException.of(msg, input)`, `KC/exceptions/InputOutputValidationException.java:35-37`, used at :398-401). A MULTISELECT can produce several violations. They are kept as a Set and joined with the platform line separator, in hash order (`merge`, :55-60).
- (b) An `IllegalArgumentException` or `ConstraintViolationException` thrown inside `parseType` is re-thrown as is (:605-606). That covers `TypeConverter`, URI, array elements and the SECRET validator. It is caught at :404-405 and produces the same prefix, with `Cause: <e.getMessage()>`. For a CVE, Jakarta's message is `<path>: <msg>`, so SECRET reads `Cause: input1: it must match the pattern …` (`K/core/src/test/java/io/kestra/core/runners/InputsTest.java:508`).
- (c) Any other `Throwable` in `parseType` is wrapped as `" errors:\n```\n" + msg + "\n```"` (:607-608). Examples are JSON, YAML and ION parse failures, and SECRET without encryption. It is then caught at :406-407 as `InputOutputValidationException.of(msg)`, **with no "Invalid value for input" prefix**. The corpus records `valids/input-log-secret`: `POST execution -> 422:  errors:\n```\nUnable to use a \`SECRET\` input/output as encryption is not configured\n```` (oracle-exec/corpus.json).
- (d) An undeclared input key causes no error. A WARN is logged, `Input {} was provided for workflow {}.{} but isn't declared in the workflow inputs` (:249-261), and the key is **dropped** from the stored inputs.
- (e) `readExecutionInputs` throws on the **first** input that has errors, in declared order (:240-247, `//TODO check to return all exception at-once`). Only the validate endpoint returns every input's errors.

### 1.3 Per-type table

`TypeConverter` is `KC/utils/TypeConverter.java`. A value that already has the target class passes through. Anything else goes through `toString()` and strict parsing, and a failure throws `TypeConversionException` (an `IllegalArgumentException`) with the message `Cannot convert value '<v>' to <Class>` (:215-225). That message gets the prefix described in (b).

| type | own fields (name: type = default) | parse (`FlowInputOutput.parseType`) | validate (message) | file:line |
|---|---|---|---|---|
| STRING | `validator: String` (`@Regex`) | `current.toString()` (:541). A JSON number posted gives `"5"`, a map gives Java's `{a=1}` | full-match regex (`RegexUtils.matches` → `Matcher.matches`, 10 s timeout, RegexUtils.java:22,73-89): ``it must match the pattern `<v>` ``. On timeout: ``the validator pattern `<v>` timed out — it may be too complex`` | StringInput.java:22-48 |
| INT | `min: Integer`, `max: Integer` | `TypeConverter.toInteger`, i.e. `Integer.valueOf(toString())` (:552). A Long or Double such as `1.0` **fails**; `"+5"` passes | ``it must be more than `<min>` `` / ``it must be less than `<max>` ``. The bounds are inclusive (`compareTo < 0`) | IntInput.java:17-44 |
| FLOAT | `min: Float`, `max: Float` | `Float.valueOf(toString())` (:554). Java syntax, so `"1e3"`, `"NaN"`, `"Infinity"` and `"1.5f"` are accepted | same wording as INT | FloatInput.java:17-44 |
| BOOL | — | `Boolean.parseBoolean(toString())` (:555). **It never fails**: anything that is not `true`, case-insensitive, becomes `false`, including `1` and `"yes"` | none | BoolInput.java:15; TypeConverter.java:33-39 |
| DATETIME | `after: Instant`, `before: Instant` | `Instant.parse` (strict ISO instant) | ``it must be after `<after>` `` / ``it must be before `<before>` `` | DateTimeInput.java:19-46 |
| DATE | `after: LocalDate`, `before: LocalDate` | `LocalDate.parse` | same as DATETIME | DateInput.java:19-46 |
| TIME | `after: LocalTime`, `before: LocalTime` | `LocalTime.parse` | same as DATETIME | TimeInput.java:19-46 |
| DURATION | `min: Duration`, `max: Duration` | `Duration.parse` (ISO, e.g. `PT1H`; `P1M` is rejected) | ``It must be more than `<min>` `` / ``It must be less than `<max>` ``. **Capital I**, unlike INT and FLOAT | DurationInput.java:19-46 |
| FILE | `allowedFileExtensions: List<String>` | `URI.create`. If the scheme is kestra, file or nsfile it passes through (`URIFetcher.supports`, URIFetcher.java:19,73-75). Otherwise it is treated as a **local path** and uploaded with `storageInterface.from(..., new File(current))` (:560-568) | `File type not allowed. Accepted extensions: a, b`. It is thrown as `new ConstraintViolationException(msg, Set.of())`, so it does not go through path (a). It is caught by the generic catch, which gives the prefixed form (b) (unverified). The extension is taken from the last `.` in the URI path, lower-cased, and must match exactly (case-sensitive against the list as written) | FileInput.java:27-52 |
| JSON | `jsonSchema: String` | a Map or Collection passes through; otherwise `JacksonMapper.toObject(string)` (:569). A parse error goes through path (c), with no prefix | draft 2020-12 schema: `it must match the json schema: [errors]`; `Invalid JSON content or schema: …`; `Invalid JSON schema: …` | JsonInput.java:33-75 |
| YAML | — | a Map or Collection passes through; otherwise the YAML mapper (:571). A scalar such as `abc` parses to the string `abc` | none | YamlInput.java:15 |
| ION | — | a Map or Collection passes through; otherwise the ION mapper (:570) | none | IonInput.java:18 |
| URI | — | `URI.create(s)`. A syntax error is an IAE. A URI with no scheme gives `IllegalArgumentException("Invalid URI format.")` (:572-578). The stored value is the **string** | none | URIInput.java:15; InputsTest.java:357 |
| SECRET | `validator: String` (`@Regex`) | (:542-551) with no encryption key: `Exception("Unable to use a \`SECRET\` input/output as encryption is not configured")`, path (c). Then `validate(String)` with the regex, **before** encryption (path (b), so the cause starts `<id>: `). Then `EncryptedString` | same wording as STRING's `validator` | SecretInput.java:23-53 |
| SELECT | `values: List<ValueOption>`, `expression: String`, `allowCustomValue: Boolean = false`, `isRadio: Boolean = false`, `autoSelectFirst: Boolean = false` | `toString()` | ``it must match the values `[a, b]` ``. **This check only runs when `required` is true** (SelectInput.java:72). A non-required SELECT accepts any value. `allowCustomValue` turns the error off. `values == null` would throw an NPE, which lands in the generic catch with a null message (unverified) | SelectInput.java:32-133. `autoSelectFirst` with no `defaults` makes the first value the default (:62-68) |
| MULTISELECT | `values`, `expression`, `itemType: Type = STRING`, `allowCustomValue = false`, `autoSelectFirst = false` | a List passes through; otherwise `JacksonMapper.toList(string)`, so **only a JSON array string** is accepted (`"A,B"` fails through path (c)). Each element is parsed with `itemType` | runs whether or not the input is required, unless `allowCustomValue`: ``value `<x>` doesn't match the values `[A, B, C]` ``, one violation per bad element, joined (path a) | MultiselectInput.java:33-143 |
| ARRAY | `itemType: Type` (`@NotNull`) | as MULTISELECT. A failing element gives ``IllegalArgumentException("Unable to parse array element as `<T>` on `<el>`")`` (:579-601) | none (ArrayInput.java:29-31) | ArrayInput.java:26-31; InputsTest.java:368 |
| EMAIL | — | `toString()` | `Pattern.matches("^$\|^[a-zA-Z0-9_!#$%&’*+/=?\`{\|}~^.-]+@[a-zA-Z0-9.-]+$")`. The pattern **accepts `""`**, and because EMAIL is a text type an empty value also passes `required`: `The input must be a valid email` | EmailInput.java:18-31 |
| FORM (open source) | `inputs: List<Input>` (`@NotNull`) | expanded into dotted leaves before resolution. Reaching parse throws `IllegalStateException` | no-op | FormInput.java:28-33; Input.java:114-146 |
| REUSABLE_INPUTS (EE, `@EeOnly`) | `ref: String` (`@NotBlank`), `namespace: String`, `revision: Integer` | expanded by `ReusableInputsExpander`. Open source refuses it at save | no-op | ReusableInputsInput.java:35-53; FlowValidator.java:195-212 |

`ValueOption` is a plain scalar or `{label, value}`. An object without `value` throws ``Select option object must define a `value` field`` (ValueOption.java:41).

Types of the stored values: Instant, LocalDate, LocalTime and Duration serialise to ISO strings. A FLOAT is stored as a Java `Float`. SECRET is stored as an `EncryptedString` and RunVariables decrypts it for rendering (RunVariables.java:439-480).

### 1.4 Where the errors surface in Kestra

| entry | runs `FlowInputOutput`? | what a bad input does | ref |
|---|---|---|---|
| API `POST /api/v1/{tenant}/executions/{ns}/{id}` (multipart only) | `readExecutionInputs(flow, executionId, inputs)` | an `InputOutputValidationException` goes to `ErrorController`, **HTTP 422**. The body is a Micronaut `JsonError`: `{"message":"Invalid entity: <msg>","_links":{"self":{"href":…}}}`. Nothing is created | `K/webserver/.../controllers/api/ExecutionController.java:999-1013`; `K/webserver/.../controllers/ErrorController.java:45-47,245-256` |
| validate endpoint | see §2 | errors are returned inside a 200 body | ExecutionController.java:903-927 |
| Webhook trigger (`trigger.inputs`) | rendered, then `readExecutionInputs` | the exception is wrapped as `WebhookInputRenderException("Unable to render the webhook inputs")`, which gives **HTTP 422 with no body** (`HttpResponse.of(UNPROCESSABLE_ENTITY)`). Stored files are deleted and no execution is created | `KC/services/WebhookService.java:168-180`; `KC/../plugin/core/trigger/Webhook.java:235-238` |
| Schedule / ScheduleOnDates / backfill | `getInputs` merges the trigger's `inputs` with the backfill's, renders them, then calls `readInputs`. `readInputs` is called **twice**, at :63 and :72 | the exception escapes `Schedule.evaluate`. `SchedulableEvaluator` logs WARN `[date: …] Evaluate Failed with error '…'` and returns `Optional.empty()`: no execution, no FAILED record. Whether the tick advances is unverified | `KC/../plugin/core/trigger/SchedulableExecutionFactory.java:60-87`; `K/scheduler/.../internals/SchedulableEvaluator.java:44-85` |
| Flow trigger (`trigger.Flow.inputs`) | **No.** The inputs are only rendered and stored raw | a render failure gives an execution created FAILED (`Failed to trigger flow … invalid inputs`). There is **no type, required or values check**. Defaults are filled at render time by `RunVariables`, "useful for triggers" | `KC/../plugin/core/trigger/Flow.java:336-357`; `KC/runners/RunVariables.java:451-466`. The executor uses `inputAndOutput` only for outputs (`K/executor/.../ExecutorService.java:461-478`) |
| Subflow task | `Execution.newExecution(flow, (f,e) -> readInputs(f, e, inputs))` | the exception is caught in the executor: the parent's Subflow task run is FAILED and no child is created | `KC/runners/ExecutableUtils.java:234-240`; ExecutorService.java:1339-1346 |
| `subflow()` Pebble function | `readExecutionInputs` | the exception reaches the expression | `KC/runners/pebble/functions/SubflowFunction.java:174` |
| Replay (plain) | no: the source's stored inputs are copied | — | `K/executor/.../handler/ExecutionCommandMessageHandler.java:166-176` |
| Replay with inputs | `readExecutionInputs(flow, current, inputs)` | 422, as for the API | ExecutionController.java:1457-1498 |
| Resume with `onResume` | `validateExecutionInputs` / `readExecutionInputs` on the Pause's `onResume` | resume-validate endpoint at :1854-1866 | `KC/services/ExecutionService.java:796-839` |
| CLI `flow test`, MCP tool | `readExecutionInputs` / `resolveInputs` | — | `K/cli/.../FlowTestCommand.java:109`; `K/webserver/.../mcp/McpToolService.java:156` |

---

## 2. The validate-inputs endpoint

`POST /api/v1/{tenant}/executions/{namespace}/{id}/validate`, `consumes = multipart/form-data`. Query parameters: `labels` (multi `key:value`) and `revision` (optional) (ExecutionController.java:903-927).

- The flow comes from `getFlowIfExecutableOrThrow`, which answers 404 or 409 for a missing or disabled flow (`@ApiResponse 409 "if the flow is disabled"`). An `Execution.newExecution(flow, labels)` is built but **not persisted**.
- It calls `validateExecutionInputs`, which is `readData(..., uploadFiles=false)`. A file part is **not stored**: its value becomes the would-be `kestra://…/inputs/<id>/<filename>` URI, and the upload is discarded (FlowInputOutput.java:147-208, 168-175). It then calls `resolveInputs(..., decryptSecrets=false)`.
- It also evaluates the flow's `checks` (`flowService.getFailedChecks`, which is egret package #16) against the nested values.
- **Response 200, even when inputs are invalid.** The body is `ApiValidateExecutionInputsResponse` (:2941-3001):

```json
{
  "id": "<flowId>", "namespace": "<ns>",
  "inputs": [
    { "input": { /* the full (rendered) Input definition */ },
      "value": "<typed value or null>",
      "enabled": true,
      "isDefault": false,
      "errors": [ { "message": "Invalid value for input `x`. Cause: …", "renderError": false } ] }
  ],
  "checks": [ { "message": "…", "style": "…", "behavior": "BLOCK_EXECUTION|FAIL_EXECUTION|…" } ]
}
```

- Every input appears, disabled ones included (`enabled: false`). `renderError: true` marks a broken field: an expression or default that failed to render (InputOutputValidationException.java:15-44).
- The Kestra UI calls it on form edits (`K/ui/src/stores/executions.ts:323-331`).
- Other statuses come from the global handlers: 404 when the flow is missing, 409 when it is disabled, and 422 for an `IllegalArgumentException`, for example bad labels.

---

## 3. Egret: every path that starts an execution with inputs

### 3.1 What egret stores and coerces today

- `models::Input` (`EW/models/input.rs:52-96`) is one flat struct: `id, type, description, required(=true), defaults, prefill, displayName, dependsOn{inputs, condition}, itemType, allowedFileExtensions`. **None of these are modelled:** `validator, min, max, after, before, values, expression, allowCustomValue, isRadio, autoSelectFirst, jsonSchema`, and FORM's `inputs` / REUSABLE `ref`. At save they are `not_honoured` warnings naming "roadmap package #3" (`EW/spec/keys.rs:192-204`, emitted at :881-915). FORM and REUSABLE are marked Enterprise.
- Save-time checks egret already has: `input_item_type_invalid` (`EW/spec/validate.rs:1131-1170`), `input_file_extension_invalid` (:1189-1196), `duplicate_input_id` (:1208-1210), `schedule_missing_inputs` (:1219-1270), the dashed reference check (:1280+). The input id pattern is probably enforced by the embedded JSON schema (`EW/spec/schema/kestra-flow-schema.json:174`); I did not verify that. **Missing at save:** dependsOn naming an unknown input, dependsOn cycles, `defaults` with `required: false`, `defaults` together with `prefill`, FILE `defaults` scheme, regex syntax (`invalids/inputs-bad-validator-syntax` is baselined).
- **The stored inputs are what was posted, uncoerced and without defaults.** They are coerced at **every render**: `Surface::load` → `with_declared_defaults` (`EW/worker/render.rs:198`, :930-976). For INT, FLOAT, BOOL, JSON, ARRAY and MULTISELECT, a posted string is parsed as JSON **only if it parses**. If it does not, the string is kept silently. All other types keep the raw string. Defaults are the raw `Property::source()` text; whether an expression default is ever rendered is not verified, and the frontend says it is not filled in (`E/frontend/src/components/workflows/run/inputModel.ts:68-69`).
- **What a bad input does today:** the execution starts, and the run fails later at render or produces a wrong result. Examples: `{{ inputs.n + 1 }}` on the string `"abc"`, or BOOL `"yes"` staying a string. No start path checks `required`, type, values, validator, bounds or SECRET/URI/EMAIL/YAML/ION. The only exception is **backfill**, which checks `required` (`EW/triggers/backfill.rs:309-316`: `"<id> is required"`, plus FILE refused and templated values refused).
- The checks that do run at start are the FILE ones: `check_file_inputs` in the start transaction (`EW/worker/claim.rs:3983-3994`; `EW/storage/inputs.rs:240-290`, ``Invalid value for input `x`: …``), and the extension allowlist on upload (`storage/inputs.rs:70-84`). SECRET `split` refuses a SECRET value when there is no master key (`EW/secret_inputs.rs:80-115`).

### 3.2 Start paths

| path | file:line | where the inputs come from | goes through `start_execution_in_on`? | what a refusal becomes today |
|---|---|---|---|---|
| API JSON `POST /api/ws/{ws}/workflows/flows/{id}/executions` | `EW/routes.rs:2553` (handler), `start_inputs` :2445-2461, call :2625 → `worker::start_execution` (`claim.rs:3691`) → `start_execution_with_trigger` (:3748) | JSON object, values already typed by the client | yes | `Error::InvalidInput` → **400** `{"error": "..."}` (`E/crates/egret-core/src/http.rs:17,36`) |
| API multipart (same route) | `routes.rs:2676` `start_multipart`, parts loop :2753-2805, call :2812 `start_execution_in_with` | every value is a **string** part; a FILE part is uploaded first. An unknown part name or a duplicate part is already refused with 400 (Kestra only warns and drops it) | yes | 400, plus best-effort deletion of the upload prefix |
| Webhook | `EW/webhook_routes.rs:638-675` (`render_inputs` → `start_execution_with_trigger` :663) | `trigger.inputs` rendered with **no** `trigger` in the context (`EW/trigger/webhook.rs:987`) | yes | 400 (an unrenderable value) or 409 (needs the trigger context); start errors arrive as 400 by `?`. Kestra answers 422 with an empty body |
| Schedule fire and backfill tick | `EW/scheduler/fire.rs:470-505` (`start`, :477) | `schedule.inputs` overlaid with the backfill's (:264-270), **passed verbatim**: templated values are refused (`refuse_templated`, :591-598) | yes (in a savepoint) | `classify` (`EW/scheduler/start.rs:100-103`) turns `InvalidInput` into `Started::Refused`. The trigger is taken off the queue (`fire.rs:331-334`), or a backfill tick ends the backfill (:327-330). Kestra only logs WARN and retries the next evaluation |
| Flow trigger | `EW/flow_trigger/process.rs:973-1030` (`start_fire`, :1001) | `trigger.inputs` rendered against the source's context. If the render fails, `inputs = {}` and `born_failed = true` | yes | `failed_start_fate`: `Refused` → `Fate::Broken` (:952-958). **Note:** a born-FAILED fire carries `{}`, so a `required` check would refuse it unless it is skipped when `born_failed` |
| Backfill create | `EW/triggers/backfill.rs:285-317` (`inputs_refusal`) | the user's inputs overlaid on the schedule's | no: this is a pre-check. The fire later goes through the schedule path | refused with `"<id> is required"` etc. (see the backfill routes for the status, not checked) |
| Replay (user, whole execution) | `EW/routes.rs:3039` handler, inputs `secret_inputs::reveal` :3091-3094, call :3100 | the source execution's **stored** inputs with secrets unsealed | yes | 400. Kestra does **not** re-validate a plain replay |
| Subflow child | `EW/worker/subflow.rs:479-545` (`rendered_inputs` :1364, `secret_inputs::split` :497, own `INSERT` :514) | the `Subflow.inputs` map rendered against the parent | **no**: it has its own INSERT plus `check_file_inputs` (:535) | `fail_run(ctx, r.run, "subflow inputs: …")` fails the parent's run (:493,500). This matches Kestra's "Subflow task FAILED" |
| Loop / ForEach iteration | `EW/worker/spawn.rs:405-420` | copies the parent row's `inputs` and `secret_inputs` in SQL | no | — (same flow, already checked) |
| Retry `CREATE_NEW_EXECUTION` | `EW/worker/replay.rs:233-260` | copies the parent's inputs. It deliberately skips the start gate (:236-240) | no | — |
| Resume | `routes.rs:2929` | no inputs (`onResume` is package #9) | — | — |
| Tests and oracle | `oracle-exec/driver.rs:1175` → `worker::start_execution` | the harvest's recovered inputs | yes | `Ran::Broken("start_execution: …")`, **not** `Refused` |

### 3.3 The chokepoint

The best single point is **`start_execution_in_on`** (`EW/worker/claim.rs:3896-4027`). It is private. The public entry points are `start_execution_in_with` (:3886) for production and `start_execution_in` (:3875) for tests only. Every caller in the table that goes through it already parses the flow there (:3920) and runs the other start gates (`unrunnable` :3928). It also already holds the place where inputs are split. The check belongs **between :3936 (normalising to an object) and :3943 (`secret_inputs::split`)**, because SECRET values are masked after `split`, so the SECRET `validator` has to run before it.

What callers pass through `NewExecution` (:3777-3846): `inputs` (a JSON object, typed for the JSON API, strings for multipart, rendered for webhook and flow trigger, verbatim for schedule, stored for replay), `born_failed` (flow trigger render failure), `replayed_from_execution_id` (replay), `input_files` (multipart uploads), `trigger`, and `backfill_id`.

Points the design has to decide:

1. Skip validation when `born_failed`, or the flow trigger's Kestra-measured FAILED birth becomes a refusal.
2. Whether a replay re-validates. Kestra does not. A flow edited since then is irrelevant, because egret replays the frozen source. Re-validating is idempotent only if the stored form re-validates, so storing coerced values is the safer choice.
3. The Subflow child does not go through the chokepoint. Either call the same validator from `subflow.rs` before :497 (its failure path, `fail_run`, already matches Kestra), or route the child through it.
4. Whether to **store the coerced and defaulted values**, as Kestra does (FlowInputOutput:237-262), or keep "raw stored, coerced at render" (`render.rs:930`). Storing them changes what the API returns and what a replay re-posts.
5. Schedule refusals currently take the trigger off the queue. Kestra only skips the evaluation.

---

## 4. Golden oracle

Of the 354 corpus flows, 67 declare `inputs:` (`grep -l '^inputs:'` under `oracle-exec/corpus`). In `oracle-exec/golden.json`, 273 are `runs`, 27 `refused/invalid-flow` and 1 `refused-by-both`. Golden entries record states, not values, so input values are compared only through the run states they produce.

**Input-related entries:**

| id | Kestra (golden / corpus.json) | egret today |
|---|---|---|
| `invalids/inputs-with-multiple-constraint-violations` | publish 200. `POST execution` with `multi: ["F","H"]` (harvest.py:399-400) gives **422** ``Invalid value for input `multi`. Cause: value `H` doesn't match the values `[A, B, C]` `` + newline + the same for `F` (order is HashSet order) | **baselined** (`oracle-exec/baseline.txt:577`). egret runs it. Banking it needs (i) the MULTISELECT check at start and (ii) the harness to accept a refusal at **start**. Today `driver.rs:1175-1178` turns a start error into `Ran::Broken`, and `tests/executor.rs:6655-6663` has only `Seam::Run` and `Seam::Save`, so a third `Start` seam (or equivalent) is needed. README notes: `oracle-exec/README.md:565-571, 3578-3581` |
| `invalids/inputs-bad-validator-syntax` | publish 422 `inputs[badValidatorSyntax].validator: invalid pattern [\]` | **baselined** (baseline.txt:576). Needs a save-time check with the Java regex dialect. The `regex` crate would over-refuse (README.md:3573-3577) |
| `invalids/inputs-validation` | 422: nine `itemType` errors plus the FILE `defaults` error | not baselined: egret refuses it through `input_item_type_invalid`. egret has no FILE-defaults check |
| `invalids/duplicate-inputs`, `inputs`, `inputs-bad-type`, `inputs-key-with-subtraction-symbol-validation`, `scheduled-missing-inputs-defaults(-2)` | 422 at publish | not baselined (matched) |
| `valids/json-input-schema-validation` | runs SUCCESS with a schema-conforming value (harvest.py:332-335) | matches. `jsonSchema` is a `not_honoured` warning. No golden entry has a failing schema |
| `valids/input-log-secret` | `not-standalone`: harvest got 422 "encryption is not configured" | — |
| `valids/secret-input-validation` | `not-standalone`: the Java test asserts that `readExecutionInputs` throws (InputsTest.java:493-508) | no golden |
| `valids/inputs` | `not-standalone`: the richest fixture (validatedString/Int/Date/DateTime/Duration/Float/Time, array of INT, YAML defaults, `defaults: ''`, dotted ids) | no golden. InputsTest.java:146-368 holds the expected messages |
| `valids/flow-trigger-invalid-inputs-flow-a` / `-listen` | the listener is born FAILED with no task runs (W13 json) | matches through `born_failed` |
| `valids/subflow-nullable-input-parent` | SUCCESS: an unposted non-required INT in the child | matches. This is a regression guard for "not required means null is fine" |
| `valids/empty-variables` | JSON input posted as a string, coerced to an object | matches (render.rs comment :921-928) |

Other baselined flows that declare inputs, for causes **other than inputs** (my reading of roadmap §3; I did not check each in detail): `valids/if`, `valids/if-in-flowable` (order pairs a clockless driver cannot reach), `valids/parallel-fail-with-flowable`, `valids/pause-behavior` (Pause semantics), `valids/task-runif-workingdirectory` (WorkingDirectory and runIf). The roadmap's "Invalids over-accepted (4)" (`docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md:284-285`) has two input entries, the two above.

The renderer oracle (`crates/egret-workflow/oracle/`) is about Pebble expressions. Its `filter-string-undefined-input` notes concern undefined variables, not flow inputs.

---

## 5. Frontend

- **Run form:** `E/frontend/src/components/workflows/run/RunFlow.tsx` (used by `routes/workflows/FlowDetail.tsx`), with `inputModel.ts` and `inputControls.tsx`. The inputs are read from the **YAML** (`inputsFromSource`, inputModel.ts:98-136), because the plan has no `values`.
- **Client-side checks** (`validateForm`, inputModel.ts:266-317): required (only when there is no default), INT whole number, FLOAT number, DATETIME parses, EMAIL regex, URI `new URL`, DURATION ISO regex, JSON parses, ARRAY items by itemType (INT, FLOAT, BOOL, JSON). **Not checked:** SELECT/MULTISELECT membership (the user picks from a list), `allowCustomValue`, `validator`, `min`/`max`, `after`/`before`, `jsonSchema`, DATE/TIME formats, dependsOn (shown as a note only). The module comment (:11-19) says plainly that "No type, `required` or SELECT membership is checked server side, so this form is the only place those are checked". **That comment becomes false with package #3 and must be rewritten.**
- **Unsupported types** (the run is disabled): FORM, REUSABLE_INPUTS, ION and unknown types (:22-40, :138-141).
- **Wire format** (`requestFromForm`, :358-382): JSON unless a FILE is chosen. In JSON, INT and FLOAT are sent as `Number`, BOOL as a boolean, DATETIME as `toISOString()`, TIME padded to `HH:MM:SS`, **JSON as the typed text** (a string, which the backend parses once, see :338-342), MULTISELECT as `string[]`, ARRAY as typed items. In multipart, every non-file value is a string or `JSON.stringify(v)`.
- **Server errors** (`RunFlow.tsx:139-151`): any non-201 status throws `res.data.error`. `serverErrorInput` (inputModel.ts:388-393) finds the **first** ``input `id` `` in backticks that names a declared input and shows the whole message under that field. Otherwise it shows a form-level error, `The run could not be started: <msg>. Fix it, then run again.` So a server message in the ``Invalid value for input `x`… `` shape is attributed to its field automatically. A multi-input message would be attached to the first id only. There is no call to a validate endpoint (egret has none; roadmap line 490 says "missing").
- **Backfill dialog** (`E/frontend/src/components/workflows/triggers/BackfillDialog.tsx:44-50`) reuses the same model. It looks like a stray bug: `fromSchedule` tests `input.type === "BOOLEAN"`, but the wire spelling is `BOOL`, so a BOOL schedule value is never prefilled. I noticed this in passing and did not test it.

---

## 6. Ambiguities and likely deliberate divergences

1. **Flow-trigger inputs.** Kestra does *not* validate them: it renders only, and defaults are filled at render time (Flow.java:336-357; RunVariables.java:451-466). The roadmap row asks for checking "at … flow-trigger start", which goes beyond Kestra. Choose between parity (no check) and stricter (check, and then decide whether a bad input means refusal/`Broken` or a born-FAILED execution).
2. **Schedule.** Kestra logs WARN, creates nothing and retries next time. egret's `Refused` takes the trigger off the queue or ends the backfill. Since `schedule_missing_inputs` already exists at save, a run-time refusal here can only come from a type or values problem in static schedule inputs, which a **save-time** check of the schedule's literal inputs could catch instead.
3. **Webhook status.** Kestra answers 422 with an empty body. egret answers 400 or 409 with a message. Probably keep 400 with a message, since egret maps `InvalidInput` to 400 everywhere, and document the divergence. The same 400-vs-422 question applies to the API start: Kestra is 422 with `{"message":"Invalid entity: …"}`, egret is 400 with `{"error": …}`.
4. **Error aggregation.** Kestra's start fails on the first bad input only. Its validate endpoint lists all of them. Egret could list all at start; that is a harmless superset, but the oracle compares message text only in the README.
5. **Exact message text.** There are three formats: (a) and (b) are prefixed, (c) has no prefix and the ```` ``` ```` wrapper, (d) is `Missing required input:x` with no space. Also INT/FLOAT say `it must…` while DURATION says `It must…`, and the SECRET cause gets `<id>: `. Decide whether to copy these verbatim. Recommendation: reproduce the prefixed form and the specific causes, and do not reproduce the (c) wrapper.
6. **SELECT that is not required** accepts any value (SelectInput.java:72). This is a Kestra quirk, probably a bug. MULTISELECT always checks. Parity or fix?
7. **BOOL never fails** (`parseBoolean`): `"yes"` and `1` become `false`. The frontend only sends booleans. Keep parity or refuse anything other than `true`/`false`?
8. **INT from a JSON number**: `1.0` fails in Kestra, while Jackson gives an Integer for `1`. egret receives typed JSON from its own API (Kestra's API is multipart only, so it always gets strings). Need a rule for `1.0`, for `"1"` as a string, and for numbers outside the 32-bit range (Kestra's INT is a Java `Integer`). FLOAT accepts `"NaN"`, `"Infinity"` and `"1.5f"` in Kestra, and is stored as a 32-bit float.
9. **Empty string.** For text types (STRING, SELECT, EMAIL, SECRET) `""` satisfies `required`, and EMAIL's regex accepts `""`. For every other type `""` is absent. egret's multipart sends `""` for empty text fields? Not verified: the frontend omits empty values (`requestFromForm` :365).
10. **Undeclared keys.** Kestra warns and *drops* them. egret JSON stores them. egret multipart **refuses** them (400). Pick one rule.
11. **Store typed and defaulted values or not.** Kestra stores the resolved map, defaults included, rendered (Pebble defaults such as `{{ now() }}` are evaluated once at start). egret stores what was posted and fills raw default text at every render. Validating defaults at start (Kestra validates them too) needs rendered defaults. Expression defaults that depend on runtime context will fail in egret's start context, so decide which context to render them in (Kestra: flow, execution, and the dependency inputs only).
12. **dependsOn / condition.** Kestra drops disabled inputs and skips their required check. egret parses `dependsOn` and ignores it, so a required input behind an unmet `dependsOn` would be refused wrongly unless dependsOn is evaluated. `condition` needs Pebble with the dependency values in the context.
13. **SELECT/MULTISELECT `expression`** needs a render at start (Kestra renders it per validation). `isRadio` and `autoSelectFirst` affect the UI, except that **`autoSelectFirst` changes the default** (SelectInput.java:62-68).
14. **JSON `jsonSchema`** needs draft 2020-12 validation. Package #3 lists it in `lift_of` (keys.rs:200). It is the only check that needs a new dependency (a JSON Schema crate); egret already ships the flow JSON schema, so a validator crate may already be in the tree (not checked).
15. **Regex dialect.** Run-time `validator` matching with the Rust `regex` crate differs from `java.util.regex` (lookaround, backreferences, possessive quantifiers). Kestra uses full-match semantics (`Matcher.matches`) with a 10 s timeout. The README's earlier concern about over-refusing at save applies here too.
16. **FILE.** egret is already stricter (only readable `kestra://` URIs, and the extension list is enforced). Kestra also accepts `file://`, `nsfile://` and **plain local paths**, which it uploads from the server's own disk (FlowInputOutput.java:560-568). That last one is deliberately not ported.
17. **SECRET.** Kestra encrypts with `kestra.encryption.secret-key` and validates before encryption. egret seals into `secret_inputs` with `secrets.master_key`. Validation must happen before `split`. Error wording differs: egret says `…set secrets.master_key` (secret_inputs.rs:105-110), Kestra says "encryption is not configured". SECRET `defaults` are not split in egret (defaults are not stored); not verified.
18. **FORM, REUSABLE_INPUTS and dotted ids.** FORM is open source in the Kestra checkout (`FormInput` is not `@EeOnly`, and FlowValidator only rejects `@EeOnly`). The roadmap and keys.rs call FORM "Enterprise". **That conflicts with the source; check against the pinned image.** Dotted ids such as `nested.string` in `valids/inputs` are nested by Kestra (`flattenToNestedMap`). Whether egret's render nests them is not verified.
19. **Validate endpoint.** Kestra's is multipart, does not persist, does not upload files (it fabricates a URI), and also evaluates `checks` (package #16). Egret will need a JSON variant, since its UI starts with JSON. The response shape could mirror Kestra's `{id, namespace, inputs[{input, value, enabled, isDefault, errors[{message, renderError}]}], checks[]}`.
20. **Save-time gaps** belong next to package #3's run-time work: regex syntax, dependsOn unknown or cyclic, defaults with `required: false`, defaults with prefill, FILE defaults scheme. Whether the `defaults ⇒ required` rule exists in the pinned image is unverified. If it does not, adding it would over-refuse flows like `valids/inputs` (`empty` has `defaults: ''` with `required: true`, so that one is fine; no corpus flow has `defaults` with `required: false`).

### Not verified

- Which of the newer Kestra features are in the pinned image (see the caveat at the top).
- The exact JSON body Micronaut writes for a 422 (`message` / `_links`), taken from reading `ErrorController`.
- Whether the Kestra schedule tick advances after an input failure.
- How the FILE `ConstraintViolationException(msg, Set.of())` message comes out once wrapped.
- Whether egret renders Pebble in a default's source, and whether egret nests dotted input ids.
- The backfill route's HTTP status for `inputs_refusal`.
- The `BackfillDialog` BOOL prefill bug: found by reading the code, not tested.
