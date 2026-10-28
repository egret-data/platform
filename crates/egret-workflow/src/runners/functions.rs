// kestra: core/src/main/java/io/kestra/core/runners/pebble/Extension.java
// kestra: core/src/main/java/io/kestra/core/runners/pebble/functions/*.java

//! Kestra's Pebble **functions**: all 34 the built engine registers, plus the
//! four it does not.
//!
//! W2 Task 7 took the nineteen that reach no service; **W2 Task 8 took the
//! rest, and seven of those turned out not to reach a service either.**
//!
//! # The wire name is a `NAME` constant, not a file name
//!
//! `Extension.getFunctions()` registers each class under its own
//! `XxxFunction.NAME`, and eight of them do not match the class: `range` is
//! `BoundedRangeFunction`, `read` is `ReadFileFunction`, `id`/`ksuid`/`uuid`
//! are `IDFunction`/`KSUIDFunction`/`UUIDFunction`, `nanoId` is
//! `NanoIDFunction` (lower-case `d`), `fileURI` is `FileURIFunction`, and
//! `FetchContextFunction` is registered **twice**, as `fetchContext` and as
//! `printContext`. Read out of the artifact's bytecode
//! (`javap -p -c -constants`) and cross-checked against `host.Oracle dump`,
//! which reports the 34 names the built engine actually carries.
//!
//! ```text
//! Task 7, the pure nineteen:
//!   now uuid id ksuid nanoId randomInt randomPort range
//!   fromJson fromIon yaml
//!   dayOfMonth dayOfWeek hourOfDay monthOfYear
//!   isWeekend isPublicHoliday isLastWorkingDay isDayWeekInMonth
//!
//! Task 8, seven more that are also pure:
//!   env encrypt decrypt fetchContext printContext tasksWithState loopOutputs
//!
//! Task 8, eight registered with a null backing -- refusals:
//!   secret kv read fileURI fileSize fileExists isFileEmpty http
//!
//! Task 8, four the built engine does not register at all -- refusals:
//!   render renderOnce errorLogs subflow
//!
//! W9a Task 4 implemented four of the null-backed eight over the render's
//! file scope: read fileSize fileExists isFileEmpty
//!
//! W9b Task 4 implemented fileURI over namespace files, and W9c Task 4 kv
//! over the KV store, both through the same scope
//! ```
//!
//! # Seven of Task 8's "service-backed" functions reach no service
//!
//! The plan and the task brief both describe Task 8 as "the service-backed
//! functions, ported as explicit refusals", and name nineteen. **Seven of
//! them answer in the standalone harness**, with ordinary values in
//! `golden.json`, and were baselined as unported work rather than marked
//! error-class:
//!
//! | function | what it actually reads |
//! |---|---|
//! | `env` | the `envs` **context variable** — never `System.getenv` |
//! | `encrypt` / `decrypt` | `EncryptionService`, which is pure AES-GCM over a base64 key argument |
//! | `fetchContext` / `printContext` | the render context itself |
//! | `tasksWithState` | the `tasks` context variable |
//! | `loopOutputs` | its own `outputs` argument |
//!
//! So they are ported here, and what they depend on is W3 putting `envs` and
//! `tasks` into the context — not a service this crate would have to call.
//!
//! # A refusal is a feature
//!
//! The eight that are refused (twelve until W9a Task 4) say so in a sentence
//! naming the function, what it needs and the package that will implement it
//! (`secret`, Confined, and `http`, Unsupported, name no package). **The oracle cannot check
//! any of that**: their corpus entries are `compare: "error-class"`, which any
//! failure satisfies — including `unknown function: secret is unknown`, which
//! is what egret said before this task. See [`not_implemented`] and
//! `the_eight_refusals_name_themselves_and_their_owner`.
//!
//! # `range` was already bound, and it did not error
//!
//! **This is the trap that has now caught three tasks.** `indent` and
//! `string` are minijinja filters Task 4 overrode; `range` is a minijinja
//! **global**, and it resolves, runs and answers minijinja's way. Derived
//! from the lock file rather than from a list: minijinja 2.24.0's
//! `defaults::build_globals` inserts exactly four names under the `builtins`
//! feature — `range`, `dict`, `debug`, `namespace` — and `Environment` keeps
//! one map, so `add_function` **replaces** silently. Of this module's 19
//! names, `range` is the only collision; `debug` is already removed in
//! [`super::env::environment`].
//!
//! And the two disagree about the answer, not merely about the spelling:
//!
//! | | minijinja's `range` | Kestra's `BoundedRangeFunction` |
//! |---|---|---|
//! | `range(1, 3)` | `[1, 2]` — **exclusive** | `[1,2,3]` — **inclusive** |
//! | arguments | positional only | `start`/`end`/`increment`, either way |
//! | `range('a', 'e')` | an error | `["a","b","c","d","e"]` |
//! | size | unbounded | `RenderLimits.ensureAtMost(…, 1 000 000, …)` |
//!
//! # Seven of these are non-deterministic, and a shape check cannot see what
//! they guarantee
//!
//! `now`, `uuid`, `id`, `ksuid`, `nanoId`, `randomInt` and `randomPort` are
//! `tests/oracle.rs`'s `NON_DETERMINISTIC` set: compared for shape, never for
//! value. A shape check sees a length. It does not see a version nibble, an
//! alphabet, or which end of a range is inclusive — so each of those is
//! pinned here instead, and four of them are pinned **by exact value through
//! the oracle**, because the arithmetic allows it:
//!
//! - `randomInt(lower=5, upper=6)` is `5`, always —
//!   `floor(random() * (upper - lower)) + lower` over a one-wide range. That
//!   is lower-inclusive and upper-EXCLUSIVE, by value, in the differential;
//! - `nanoId(length=6, alphabet='x')` is `xxxxxx`, always, and
//!   `alphabet='xxx'` exercises the other selection path (a non-power-of-two
//!   alphabet is rejection-sampled) to the same deterministic answer;
//! - `uuid() | regexMatch(regex='…-7[0-9a-f]{3}-[89ab]…')` is `true`, always,
//!   which pins the version and variant nibbles and the lower-case hex that
//!   no length check can;
//! - `ksuid()`, `id()` and `nanoId()`'s alphabets are pinned the same way.
//!
//! # Argument defaults are documentation, not behaviour
//!
//! `KestraFunction.getArgumentDefaults()` is Kestra's own interface method,
//! not Pebble's, and nothing on the render path reads it — `RandomIntFunction`
//! declares `lower=0, upper=10` and `{{ randomInt() }}` still throws
//! `The 'randomIn()' function expects an argument lower`. Measured through the
//! harness. So an absent argument is absent here too, exactly as
//! [`super::filters::Args::get`] reports it.

use std::net::TcpListener;

use base64::Engine as _;
use chrono::{Datelike, Days, Months, NaiveDate, NaiveDateTime, Timelike, Utc};
use minijinja::value::{Rest, Value as MjValue, ValueKind};
use minijinja::{Environment, Error as MjError, State};
use rand::RngCore;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};

use super::env::{self, OrderedMap};
use super::filters::{is_java_null, java_class_name, pebble_error, Args, DateArgs};
use super::ionfmt::read_ion;
use super::javafmt::{parse_iso, Zdt, Zone};
use super::jqsandbox::MAX_VALUE_DEPTH;
use crate::storage::kv::value::{KvData, KvNumber};

/// Registers the 19 pure functions onto `env`.
///
/// **`range` is an override, not an addition** — see this module's doc.
pub(super) fn register(env: &mut Environment<'static>) {
    env.add_function("dayOfMonth", day_of_month);
    env.add_function("dayOfWeek", day_of_week);
    env.add_function("fromIon", from_ion);
    env.add_function("fromJson", from_json);
    env.add_function("hourOfDay", hour_of_day);
    env.add_function("id", id);
    env.add_function("isDayWeekInMonth", is_day_week_in_month);
    env.add_function("isLastWorkingDay", is_last_working_day);
    env.add_function("isPublicHoliday", is_public_holiday);
    env.add_function("isWeekend", is_weekend);
    env.add_function("ksuid", ksuid);
    env.add_function("monthOfYear", month_of_year);
    env.add_function("nanoId", nano_id);
    env.add_function("now", now);
    env.add_function("randomInt", random_int);
    env.add_function("randomPort", random_port);
    // Overrides minijinja's own `range` global, which is exclusive and
    // positional-only.
    env.add_function("range", range);
    env.add_function("uuid", uuid);
    env.add_function("yaml", yaml);

    // W2 Task 8's seven ports. `fetchContext` and `printContext` are ONE
    // class registered under two keys -- a class-per-name reading loses the
    // alias, and nothing would notice.
    env.add_function("decrypt", decrypt);
    env.add_function("encrypt", encrypt);
    env.add_function("env", env_function);
    env.add_function("fetchContext", fetch_context);
    env.add_function("loopOutputs", loop_outputs);
    env.add_function("printContext", fetch_context);
    env.add_function("tasksWithState", tasks_with_state);

    // W9a Task 4: the four file functions, over the render's file scope.
    env.add_function("read", |state: &State, rest: Rest<MjValue>| {
        file_function(FileFunction::Read, state, rest)
    });
    env.add_function("fileSize", |state: &State, rest: Rest<MjValue>| {
        file_function(FileFunction::Size, state, rest)
    });
    env.add_function("fileExists", |state: &State, rest: Rest<MjValue>| {
        file_function(FileFunction::Exists, state, rest)
    });
    env.add_function("isFileEmpty", |state: &State, rest: Rest<MjValue>| {
        file_function(FileFunction::IsEmpty, state, rest)
    });
    // W9b Task 4: `fileURI`, beside the four, over the same scope.
    env.add_function("fileURI", |state: &State, rest: Rest<MjValue>| {
        file_function(FileFunction::Uri, state, rest)
    });
    // W9c Task 4: `kv`, over the KV store, through the same scope.
    env.add_function("kv", kv_function);

    // W2 Task 8's refusals, six since W9c Task 4. See [`not_implemented`]
    // for the message shape and why the oracle cannot check it.
    for (name, refusal) in REFUSED {
        env.add_function(
            *name,
            move |_args: Rest<MjValue>| -> Result<MjValue, MjError> {
                Err(match refusal {
                    Refusal::NotYet { needs, package } => not_implemented(name, needs, package),
                    Refusal::Confined(reason) | Refusal::Unsupported(reason) => {
                        pebble_error(*reason)
                    }
                })
            },
        );
    }
}

/// Why a [`REFUSED`] name refuses.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    /// Nothing in egret provides it yet: [`not_implemented`]'s sentence.
    NotYet {
        needs: &'static str,
        package: &'static str,
    },
    /// egret provides it in ONE place and refuses it everywhere else, on
    /// purpose. The message is the whole sentence.
    Confined(&'static str),
    /// egret does not provide it and will not: the reason is where it would
    /// run, not a missing package. The message is the whole sentence.
    Unsupported(&'static str),
}

/// `http()`'s refusal (2026-10-17, spec `2026-10-17-workflow-starrocks-timeout-and-http`
/// §2). Not "not yet": an expression renders inside the egret server, so a
/// request from it would leave from the control plane's network, which is a
/// server-side request forgery for any flow author. A script task makes the
/// same request from its own container.
pub(crate) const HTTP_REFUSAL: &str = "The 'http' function is not supported: an expression \
     renders inside egret's control plane, so a request made from it would leave from \
     egret's own network. Make the request in a script task, which runs in the task's \
     container.";

/// `secret()`'s refusal on the shared environment (W5b Task 4, spec §6).
///
/// **`secret()` is implemented, but only for `kestra.*` plugin tasks**, where
/// `runners::secrets::render_with_secrets` registers a resolving `secret` over
/// this one on a copy of the environment. Every other render -- script tasks,
/// inputs, `runIf`, `Log`, triggers -- goes through this environment and meets
/// this sentence. The script task is the one an author will try, so the
/// reason is written for it: its rendered commands and env are fields of the
/// pod spec, readable by anyone with `get pods` in the task namespace.
pub(crate) const SECRET_REFUSAL: &str = "The 'secret' function resolves only on `kestra.*` \
     plugin tasks. It is refused here: on a script task the rendered commands and env are \
     fields of the pod spec, readable by anyone with `get pods` in the task namespace, and \
     no other render (inputs, runIf, Log, triggers) resolves secrets.";

/// The six names egret registers only to refuse, with what each is waiting
/// for and who owns it.
///
/// **Two different kinds of absence, deliberately treated the same.** The
/// first eight are *registered* on the built engine with a `null` `Function`
/// behind them, because `Extension.getFunctions()` `put`s their `NAME` keys
/// unguarded while Micronaut never fills the `@Inject` fields; the last four
/// are `@Inject @Nullable` and are guarded by `if (x != null)`, so they are
/// **not in the map at all** — `host.Oracle dump` reports 34 names and none of
/// these four is among them. Pebble answers
/// `Function or Macro [x] does not exist.` for **both** groups, so from a flow
/// author's seat they are one behaviour, and egret gives them one message.
///
/// The package column is this task's assignment, not the programme spec's:
/// `subflow` is named in W6's contents and the KV store and namespace files in
/// W9's, but `secret` and `http` are in no package's contents at all. They are
/// assigned to W9 here so the message can name an owner, and the task report
/// says so — a wrong owner in a message is correctable, a message with no
/// owner is not.
///
/// **Updated 2026-09-14 (W10 Task 1).** `errorLogs` moved from `W3` to `W11`:
/// W3 is merged and ships no log store, and the programme spec's §9 now gives
/// `workflow.task_logs` to W11. `secret` and `http` are still in no package's
/// contents — the same residue of the finding W10 §1 records, left open here
/// deliberately rather than reassigned a second time.
///
/// **Updated 2026-09-29 (W5b Task 4).** `secret` is no longer waiting for a
/// package: it resolves on `kestra.*` plugin tasks and is
/// [`Refusal::Confined`] everywhere else, with [`SECRET_REFUSAL`]'s reason.
///
/// **Updated 2026-10-01 (W9a Task 4): eight names now.** `read`, `fileSize`,
/// `fileExists` and `isFileEmpty` are implemented ([`file_function`]) over
/// the render's file scope; W9 split, so `fileURI` names W9b (namespace
/// files) and `kv` W9c (the KV store).
///
/// **Updated 2026-10-05 (W9b Task 4): seven names now.** `fileURI` is
/// implemented ([`file_function`]) over namespace files, beside the four,
/// which reach namespace files too; `kv` still names W9c.
///
/// **Updated 2026-10-07 (W9c Task 4): six names now.** `kv` is implemented
/// ([`kv_function`]) over the KV store.
///
/// **Updated 2026-10-17: `http` is `Unsupported`, not waiting for a package.**
const REFUSED: &[(&str, Refusal)] = &[
    // Registered with a null backing on the built engine.
    ("secret", Refusal::Confined(SECRET_REFUSAL)),
    ("http", Refusal::Unsupported(HTTP_REFUSAL)),
    // Not registered at all on the built engine: `@Inject @Nullable`, guarded.
    (
        "render",
        not_yet("an execution context to render against", "W3"),
    ),
    (
        "renderOnce",
        not_yet("an execution context to render against", "W3"),
    ),
    ("errorLogs", not_yet("the running execution's logs", "W11")),
    ("subflow", not_yet("subflow execution", "W6")),
];

const fn not_yet(needs: &'static str, package: &'static str) -> Refusal {
    Refusal::NotYet { needs, package }
}

// ---------------------------------------------------------------------------
// W9a Task 4: the file functions
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum FileFunction {
    Read,
    Size,
    Exists,
    IsEmpty,
    /// W9b Task 4: `fileURI`, namespace files only (an execution's
    /// `kestra://` URI is returned as given, once readable).
    Uri,
}

impl FileFunction {
    fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Size => "fileSize",
            Self::Exists => "fileExists",
            Self::IsEmpty => "isFileEmpty",
            Self::Uri => "fileURI",
        }
    }

    /// `read` and `fileURI` declare `revision` (`ReadFileFunction.java`,
    /// `FileURIFunction.java`: `path, namespace, revision`); the other three
    /// declare `path, namespace` only.
    fn takes_revision(self) -> bool {
        matches!(self, Self::Read | Self::Uri)
    }
}

/// `ReadFileFunction`, `FileSizeFunction`, `FileExistsFunction`,
/// `IsFileEmptyFunction` and (W9b) `FileURIFunction`.
///
/// They answer from the render's [`FileScope`](super::files::FileScope),
/// bound as the [`FILE_SCOPE`](super::files::FILE_SCOPE) global -- the pattern
/// [`fetch_context`] uses for `CONTEXT_SCOPE_NAMES`. No scope means no running
/// execution (a Flow trigger's `conditions`, a render outside a task run), and
/// all five refuse by name.
///
/// [`classify`](super::files::classify) picks the route.
///
/// **An execution file** (`kestra:///…/executions/…`, W9a spec §7.2):
///
/// | the URI | `read` | `fileSize` | `fileExists` | `isFileEmpty` |
/// |---|---|---|---|---|
/// | readable, indexed | its text | its size | `true` | `size == 0` |
/// | readable execution, no such file | error | error | **`false`** | error |
/// | anything else | error | error | error | error |
///
/// `fileURI` returns it as given when it is readable. A `namespace` or
/// `revision` argument is ignored there, as Kestra's `kestra` branch ignores
/// both (`AbstractFileFunction.java:84-87`, `ReadFileFunction.java:50-55`, by
/// the code; Task 1 did not measure it).
///
/// **A namespace file** (W9b spec §6): a bare path, `nsfile:///p`,
/// `nsfile://other.ns/p` or `kestra:///…/_files/p[.vN]`, plus the `namespace`
/// argument (with an authority, an error) and, on `read` and `fileURI`,
/// `revision`. A missing path is `fileExists` `false` and an error naming it
/// for the other four; a live directory answers `fileSize` 0, `fileExists`
/// true, `isFileEmpty` true, and `read` and `fileURI` of it are errors naming
/// the path (R3).
fn file_function(
    which: FileFunction,
    state: &State,
    rest: Rest<MjValue>,
) -> Result<MjValue, MjError> {
    use super::files::{classify, FileTarget, FileUnavailable, NsFileArgs};

    let name = which.name();
    let args = Args::new(rest);
    let Some(path) = args.get("path", 0) else {
        return Err(pebble_error(format!(
            "The '{name}' function expects an argument 'path'."
        )));
    };
    let Some(path) = path.as_str() else {
        return Err(pebble_error(format!(
            "The '{name}' function expects an argument 'path' with type string."
        )));
    };
    let scope = state.lookup(super::files::FILE_SCOPE).and_then(|v| {
        v.downcast_object_ref::<super::files::FileScopeObject>()
            .map(|held| held.0.clone())
    });
    let Some(scope) = scope else {
        return Err(pebble_error(format!(
            "The '{name}' function needs a running execution: it reads the execution's files, \
             and only a task run's render has them (a Flow trigger's conditions and inputs, \
             for one, do not)."
        )));
    };
    let failed = |message: &str| pebble_error(format!("The '{name}' function failed: {message}"));
    // M1 (follow Kestra): `fileSize`, `fileExists` and `isFileEmpty` declare
    // `path, namespace` only, and Pebble refuses an undeclared named argument
    // with this sentence (`ArgumentsNode`) -- on either route.
    if !which.takes_revision() && args.named_keys().any(|k| k == "revision") {
        return Err(pebble_error(format!(
            "The following named argument does not exist: revision (the '{name}' function \
             takes path and namespace)"
        )));
    }

    if classify(path) == FileTarget::Execution {
        let meta = match scope.0.lookup(path) {
            Ok(meta) => meta,
            Err(FileUnavailable::Missing(_)) if matches!(which, FileFunction::Exists) => {
                return Ok(MjValue::from(false));
            }
            Err(why) => return Err(failed(why.message())),
        };
        return Ok(match which {
            FileFunction::Read => MjValue::from(scope.0.read_utf8(path).map_err(|m| failed(&m))?),
            FileFunction::Size => MjValue::from(meta.size),
            FileFunction::Exists => MjValue::from(true),
            FileFunction::IsEmpty => MjValue::from(meta.size == 0),
            FileFunction::Uri => MjValue::from(path),
        });
    }

    // A namespace file. `none` is Kestra's absent argument.
    let namespace = match args.get("namespace", 1).filter(|v| !is_java_null(v)) {
        None => None,
        Some(v) => match v.as_str() {
            Some(s) => Some(s.to_string()),
            None => {
                return Err(pebble_error(format!(
                    "The '{name}' function expects the 'namespace' argument to be a string."
                )))
            }
        },
    };
    let revision = if which.takes_revision() {
        args.get("revision", 2)
            .filter(|v| !is_java_null(v))
            .map(|v| v.to_string())
    } else {
        None
    };
    // `AbstractFileFunction.java:181-191`: Kestra's sentence, before anything
    // is parsed. Storage takes the authority when there is one.
    let authority = path
        .strip_prefix(crate::runners::files::NSFILE_SCHEME)
        .is_some_and(|rest| !rest.starts_with('/'));
    if authority && namespace.is_some() {
        return Err(failed(&format!(
            "You cannot set a namespace both as the function argument and inside the URI \
             ({path})"
        )));
    }
    let ns_args = NsFileArgs {
        path,
        namespace: namespace.as_deref(),
        revision,
    };
    if matches!(which, FileFunction::Read) {
        return scope
            .0
            .read_namespace_file(&ns_args)
            .map(MjValue::from)
            .map_err(|m| failed(&m));
    }
    let meta = match scope.0.namespace_file(&ns_args) {
        Ok(meta) => meta,
        Err(FileUnavailable::Missing(_)) if matches!(which, FileFunction::Exists) => {
            return Ok(MjValue::from(false));
        }
        Err(why) => return Err(failed(why.message())),
    };
    Ok(match which {
        FileFunction::Size => MjValue::from(meta.size),
        FileFunction::Exists => MjValue::from(true),
        FileFunction::IsEmpty => MjValue::from(meta.size == 0),
        // R3 (the user's ruling, 2026-10-05): a directory has no file URI.
        FileFunction::Uri if meta.is_directory => {
            return Err(failed(&format!(
                "'{}' is a directory: it has no file URI",
                meta.path
            )))
        }
        // §6: a `kestra://` argument is returned as given, once it parsed
        // and resolved.
        FileFunction::Uri if path.starts_with("kestra://") => MjValue::from(path),
        FileFunction::Uri => MjValue::from(meta.uri),
        FileFunction::Read => unreachable!("answered above"),
    })
}

// ---------------------------------------------------------------------------
// W9c Task 4: kv
// ---------------------------------------------------------------------------

/// `KvFunction` (W9c spec §6): `kv(key, namespace?, errorOnMissing?)`,
/// positional or named (`KvFunction.java:34-36`).
///
/// - `key` is required: absent is Kestra's sentence; `null` is the empty key,
///   which the key rule refuses with Kestra's sentence (w9c
///   `kv_function.null_key`); any other non-string is refused naming it.
/// - `namespace` absent or `null`: the running flow's namespace, walking up
///   its parents on a dot boundary; given: that namespace only, no walk,
///   even when it is the flow's own (`KvFunction.java:61-66`).
/// - `errorOnMissing` defaults to **true** (`:53`); absent or `null` is the
///   default; a non-boolean is refused naming the argument (w9c
///   `kv_function.eom_*`, where Kestra throws a raw `ClassCastException`),
///   before anything is looked up.
///
/// **The two `null` answers are read from Kestra's source, not chosen** (the
/// artifact does not measure them; fix round 1, review concern 5), at
/// `KvFunction.java` (`96893b589`):
/// - `:51` `String namespace = (String) args.get(NAMESPACE_ARG);` and `:61`
///   `if (namespace == null)` take the walk, so a `null` namespace walks
///   exactly as an absent one;
/// - `:53` `Optional.ofNullable((Boolean) args.get(ERROR_ON_MISSING_ARG))
///   .orElse(true)`, so a `null` `errorOnMissing` is `true`.
///
/// It answers from the render's [`FileScope`](super::files::FileScope), as
/// [`file_function`] does: no scope means no running execution (a Flow
/// trigger's `conditions`, a corpus render), refused by name. A key that is
/// missing (never written, soft-deleted, expired) is Kestra's sentence when
/// `errorOnMissing`, else `null`. The value reaches the template exactly
/// ([`kv_value`]).
fn kv_function(state: &State, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    use super::files::{KvAnswer, KvArgs};

    const DECLARED: [&str; 3] = ["key", "namespace", "errorOnMissing"];
    let args = Args::new(rest);
    // Pebble's own argument checks (`ArgumentsNode`), with Kestra's sentences
    // (w9c `kv_function.too_many_args`, `unknown_named`).
    if args.positional_only().len() > DECLARED.len() {
        return Err(pebble_error(format!(
            "The argument at position {} is not allowed. Only {} argument(s) are allowed.",
            DECLARED.len() + 1,
            DECLARED.len()
        )));
    }
    if let Some(unknown) = args.named_keys().find(|k| !DECLARED.contains(k)) {
        return Err(pebble_error(format!(
            "The following named argument does not exist: {unknown}"
        )));
    }
    let Some(key) = args.get("key", 0) else {
        return Err(pebble_error("The 'kv' function expects an argument 'key'."));
    };
    let key = if is_java_null(&key) {
        String::new()
    } else {
        match key.as_str() {
            Some(k) => k.to_string(),
            None => {
                return Err(pebble_error(format!(
                    "The 'kv' function expects the 'key' argument to be a string, and {} is not.",
                    shown_argument(&key)
                )))
            }
        }
    };
    let namespace = match args.get("namespace", 1).filter(|v| !is_java_null(v)) {
        None => None,
        Some(v) => match v.as_str() {
            Some(ns) => Some(ns.to_string()),
            None => {
                return Err(pebble_error(format!(
                    "The 'kv' function expects the 'namespace' argument to be a string, and {} \
                     is not.",
                    shown_argument(&v)
                )))
            }
        },
    };
    let error_on_missing = match args.get("errorOnMissing", 2).filter(|v| !is_java_null(v)) {
        None => true,
        Some(v) if v.kind() == ValueKind::Bool => v.is_true(),
        Some(v) => {
            return Err(pebble_error(format!(
                "The 'kv' function expects the 'errorOnMissing' argument to be a boolean (true \
                 or false), and {} is not.",
                shown_argument(&v)
            )))
        }
    };
    let scope = state.lookup(super::files::FILE_SCOPE).and_then(|v| {
        v.downcast_object_ref::<super::files::FileScopeObject>()
            .map(|held| held.0.clone())
    });
    let Some(scope) = scope else {
        return Err(pebble_error(
            "The 'kv' function needs a running execution: it reads the KV store as the running \
             flow, and only a task run's render has one (a Flow trigger's conditions and \
             inputs, for one, do not).",
        ));
    };
    let answer = scope
        .0
        .kv(&KvArgs {
            key: &key,
            namespace: namespace.as_deref(),
        })
        .map_err(pebble_error)?;
    match answer {
        KvAnswer::Value(data) => kv_value(&data),
        KvAnswer::Missing(message) if error_on_missing => Err(pebble_error(message)),
        KvAnswer::Missing(_) => Ok(MjValue::from(())),
    }
}

/// An argument as a refusal names it: a string quoted, anything else as it
/// renders.
fn shown_argument(v: &MjValue) -> String {
    match v.as_str() {
        Some(s) => format!("'{s}'"),
        None => v.to_string(),
    }
}

/// The value `kv()` hands the template: the KV store's [`KvData`] converted
/// leaf by leaf, **never through `to_json()`** (which would make a decimal
/// an `f64`; the user's ruling, 2026-10-07, decimals preserved exactly):
///
/// - a decimal is a `java.math.BigDecimal` [`filters::big_decimal`]: a bare
///   `{{ x }}` prints its plain digits, a serialiser `BigDecimal.toString()`;
/// - an int within `i64`/`u64`/`i128`/`u128` is that integer
///   ([`env::integer_from_literal`]), beyond it a `java.math.BigInteger`
///   [`filters::big_integer`];
/// - a float is an `f64`; a string, a boolean and null are themselves; a list
///   a list; a map an ordered map, its fields in their stored order.
///
/// Neither `BigNumber` is arithmetic, so `{{ kv('d') + 1 }}` fails the render
/// naming the operator (W9c spec §15), where Kestra computes in `BigDecimal`.
/// Both COMPARE exactly (fix round 1, the user's ruling of 2026-10-07):
/// `kv('d') > 0.9`, `== 1.5`, `in [1.5]` go through `super::compare`.
///
/// A value nesting containers deeper than [`crate::storage::ion::MAX_DEPTH`]
/// is refused before anything is built: the check walks with an explicit
/// stack, so the conversion's recursion is bounded by it (Task 3 re-review
/// 2, H1). The store never hands out a deeper one; this guards the seam.
fn kv_value(data: &KvData) -> Result<MjValue, MjError> {
    let limit = crate::storage::ion::MAX_DEPTH;
    if data.nests_deeper_than(limit) {
        return Err(pebble_error(format!(
            "The 'kv' function failed: the value nests containers deeper than {limit} levels, \
             which egret does not render"
        )));
    }
    kv_value_bounded(data)
}

/// [`kv_value`] once the depth is checked.
fn kv_value_bounded(data: &KvData) -> Result<MjValue, MjError> {
    Ok(match data {
        KvData::Null => MjValue::from(()),
        KvData::Bool(b) => MjValue::from(*b),
        KvData::Number(KvNumber::Int(digits)) => {
            env::integer_from_literal(digits).unwrap_or_else(|| super::filters::big_integer(digits))
        }
        KvData::Number(KvNumber::Decimal(plain)) => {
            super::filters::big_decimal(plain).ok_or_else(|| {
                pebble_error(format!(
                    "The 'kv' function failed: the stored decimal '{plain}' is not a decimal"
                ))
            })?
        }
        KvData::Number(KvNumber::Float(f)) => MjValue::from(*f),
        KvData::String(s) => MjValue::from(s.clone()),
        KvData::List(items) => MjValue::from(
            items
                .iter()
                .map(kv_value_bounded)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        KvData::Struct(fields) => MjValue::from_object(env::OrderedMap::new(
            fields
                .iter()
                .map(|(name, item)| Ok((name.clone(), kv_value_bounded(item)?)))
                .collect::<Result<Vec<_>, MjError>>()?,
        )),
    })
}

// ---------------------------------------------------------------------------
// The structured-data readers
// ---------------------------------------------------------------------------

/// `FromJsonFunction` — `JacksonMapper.ofJson().readValue(json, Object.class)`.
///
/// Three branches, and they are not the same as `yaml`'s two lines below:
/// an **absent** `json` throws, a `json` that is Java `null` returns `null`
/// (which renders as nothing), and a non-`String` throws.
///
/// **Trailing content is not an error.** `ObjectMapper.readValue(String, …)`
/// does not enable `FAIL_ON_TRAILING_TOKENS`, so `fromJson('1 2')` reads the
/// first value and stops — measured, `1`. That is the same fact
/// `super::filters::jq`'s `readTree` branch records, and `serde_json`'s
/// `from_str` is the opposite, so this goes through `StreamDeserializer`.
fn from_json(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let Some(value) = args.get("json", 0) else {
        return Err(pebble_error(
            "The 'fromJson' function expects an argument 'json'.",
        ));
    };
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let Some(text) = value.as_str() else {
        return Err(pebble_error(
            "The 'fromJson' function expects an argument 'json' with type string.",
        ));
    };
    // No `de.end()`: reading one value and leaving the rest is exactly what
    // Jackson does, and calling it would turn `fromJson('1 2')` into an error.
    let mut de = serde_json::Deserializer::from_str(text);
    Depth(1)
        .deserialize(&mut de)
        .map_err(|e| pebble_error(format!("Invalid json: {e}")))
}

/// `FromIonFunction` — `FileSerde.readAll(...)`, a Jackson `MappingIterator`
/// over a stream of top-level Ion values.
///
/// `allRows` picks between `toList()` and `findFirst().orElse(null)`, so the
/// default reads the **first value and drops the rest** rather than failing.
/// A `null` input returns `null`; anything that is not a `String` (or, in
/// Kestra, a `byte[]`, which no Pebble context can carry) throws.
///
/// **`fromIon('null')` throws**, which is upstream defect 6's last row and is
/// reachable without writing it by hand: `{{ nothing | toIon }}` is the
/// four-character string `null` and this function then refuses it. Both
/// halves are corpus entries.
fn from_ion(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let Some(value) = args.get("ion", 0) else {
        return Err(pebble_error(
            "The 'fromIon' function expects an argument 'ion'.",
        ));
    };
    if is_java_null(&value) {
        return Ok(MjValue::from(()));
    }
    let Some(text) = value.as_str() else {
        return Err(pebble_error(
            "The 'fromIon' function expects an argument 'ion' with type string or byte[].",
        ));
    };
    let all_rows = args
        .get("allRows", 1)
        .is_some_and(|flag| flag.kind() == ValueKind::Bool && flag.is_true());

    // **`all_rows` decides how much of the document is even PARSED.**
    // `findFirst()` is `take(1)`, so Kestra never looks past the first value:
    // `fromIon('1 /*c')` and `fromIon('1 +5')` are both `1` there, and were
    // errors here the moment this reader grew refusals. `toList()` does read
    // the tail, and every one of those documents throws under `allRows=true`.
    // Measured, both ways, over fourteen documents.
    let rows = read_ion(text, all_rows).map_err(|e| pebble_error(format!("Invalid ion: {e}")))?;
    // `sink.next(t)` refuses a null element. Over a lazily-limited `rows` this
    // is exactly "the first value is null", which is what `fromIon('null 1')`
    // throws on and `fromIon('1 null')` does not.
    if rows.iter().any(is_java_null) {
        return Err(pebble_error("Invalid ion: t is null in sink.next(t)"));
    }
    if all_rows {
        return Ok(MjValue::from(rows));
    }
    Ok(rows.into_iter().next().unwrap_or_else(|| MjValue::from(())))
}

/// `YamlFunction` — `new ObjectMapper(new YAMLFactory()).readValue(yaml,
/// Object.class)`.
///
/// **It has no null branch, and `fromJson` does.** `yaml(yaml=nothing)` is
/// `null instanceof String`, which is false, so it reaches
/// `The 'yaml' function expects an argument 'yaml' with type string.` while
/// `fromJson(json=nothing)` returns null and renders as nothing. Two
/// functions in the same batch, two answers to the same input; both are
/// corpus entries.
///
/// # The plain-scalar resolver is YAML 1.2 here and YAML 1.1 in Kestra
///
/// **A recorded divergence, not a defect of this port, and it is the reason
/// the crate underneath is called `serde_norway`.** SnakeYAML resolves the
/// YAML 1.1 schema and `serde_norway` resolves the 1.2 core schema, and the
/// two disagree about a closed set of *plain* (unquoted) spellings — measured
/// through the harness:
///
/// | plain scalar | Kestra | egret |
/// |---|---|---|
/// | `yes`, `on` | `true` | the string |
/// | `y`, `n` | the string | the string (they agree) |
/// | `010` | `8` — a leading zero is octal | the string |
/// | `0o10` | the string | `8` |
/// | `.inf`, `.nan` | **throws** | **refused**, see [`BoundedVisitor::visit_f64`] |
///
/// It is recorded rather than reproduced because the fix is not available:
/// reproducing it needs to know whether a scalar was **quoted**, and
/// `serde_norway` hands a visitor `"yes"` and `yes` identically —
/// `visit_untagged_scalar` funnels both into `visit_str`. Re-resolving on the
/// string alone would turn `a: "010"` into `8`, which is a *new* silent wrong
/// answer traded for an old one. `function-yaml-yes-is-a-boolean-or-not` and
/// `function-yaml-octal-looking` are baselined with this reason.
///
/// # This is the READ half of the filter `super::yamlfmt` writes
///
/// Three of upstream defect 6's rows only exist end to end, and all three are
/// pinned as round trips through this function: `'42' | yaml` writes the
/// unquoted scalar `42` and this reads it back as an **integer**, `'1.5'`
/// comes back a double, `'1e5'` comes back `100000.0`, and `'.inf'` is
/// written as `.inf` and then **refused** here.
fn yaml(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let Some(value) = args.get("yaml", 0) else {
        return Err(pebble_error(
            "The 'yaml' function expects an argument 'yaml'.",
        ));
    };
    let Some(text) = value.as_str() else {
        return Err(pebble_error(
            "The 'yaml' function expects an argument 'yaml' with type string.",
        ));
    };
    // **The FIRST document, and an empty stream is an error.** Measured:
    // `yaml('a: 1\n---\nb: 2')` is `{"a":1}` in Kestra and was
    // `deserializing from YAML containing more than one document is not
    // supported` here; `yaml('')`, `yaml('   ')` and `yaml('# comment')` all
    // throw `No content to map due to end-of-input` there and answered
    // **null** here, which renders as nothing — a wrong value that renders
    // successfully.
    if is_empty_yaml_document(text) {
        return Err(pebble_error(
            "Invalid yaml: No content to map due to end-of-input",
        ));
    }
    let Some(document) = serde_norway::Deserializer::from_str(text).next() else {
        return Err(pebble_error(
            "Invalid yaml: No content to map due to end-of-input",
        ));
    };
    Depth(1)
        .deserialize(document)
        .map_err(|e| pebble_error(format!("Invalid yaml: {e}")))
}

/// Is there no content at all? — every line blank or a comment.
///
/// `serde_norway` deserialises such a source to a **unit**, which renders as
/// nothing; Jackson throws `No content to map due to end-of-input`. So the
/// question has to be asked of the source, before the parser, and it is asked
/// the narrowest way that answers it: a document with any content has at least
/// one line whose first non-space character is not `#`.
///
/// It cannot mis-fire on a real document, and it must not be confused with a
/// document whose content IS null: `yaml('~')` and `yaml('a: null')` are
/// `null` and `{}` on both engines, and neither is empty by this rule.
fn is_empty_yaml_document(text: &str) -> bool {
    text.lines()
        .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
}

/// A serde seed that carries the nesting depth, so **both** readers above are
/// bounded by one number.
///
/// # Why a seed rather than a plain `Deserialize`
///
/// `super::env::OrderedJson` is the same visitor without a depth, and it is
/// enough for a context value because `serde_json`'s parser has a recursion
/// limit of its own (128, the number [`MAX_VALUE_DEPTH`] is taken from). YAML
/// has no such limit anywhere: `serde_norway`'s parser is an event stream, so
/// the only recursion is this visitor's, and `a: {a: {a: …` is a filter
/// argument a flow author writes. A `DeserializeSeed` is the one serde shape
/// that can carry state into `next_element`/`next_value`, which is what makes
/// the depth a parameter rather than a thread-local.
///
/// The bound is the crate's existing value-depth invariant rather than a new
/// number: *no value anywhere in this crate is more than `MAX_VALUE_DEPTH`
/// deep*, which is what lets `env::write_json`, `ionfmt::write_ion`,
/// `yamlfmt::to_yaml` and `env::kestra_to_string` walk anything they are
/// handed on a 2 MiB worker.
struct Depth(usize);

impl<'de> DeserializeSeed<'de> for Depth {
    type Value = MjValue;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<MjValue, D::Error> {
        if self.0 > MAX_VALUE_DEPTH {
            return Err(de::Error::custom(format!(
                "the value nests more than {MAX_VALUE_DEPTH} levels deep, which is what every \
                 writer in this crate can walk"
            )));
        }
        d.deserialize_any(BoundedVisitor(self.0))
    }
}

struct BoundedVisitor(usize);

impl<'de> Visitor<'de> for BoundedVisitor {
    type Value = MjValue;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("any value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<MjValue, E> {
        Ok(MjValue::from(()))
    }

    fn visit_none<E: de::Error>(self) -> Result<MjValue, E> {
        self.visit_unit()
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<MjValue, E> {
        Ok(MjValue::from(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<MjValue, E> {
        Ok(MjValue::from(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<MjValue, E> {
        Ok(MjValue::from(v))
    }

    /// **A non-finite float is a refusal, not a value**, and that is fidelity
    /// rather than caution.
    ///
    /// YAML 1.1 resolves `.inf`, `-.inf` and `.nan`; Jackson's own numeric
    /// coercion, one layer above SnakeYAML, then rejects them —
    /// `Malformed numeric value '.inf'`. That is upstream defect 6's sharpest
    /// row: the `yaml` FILTER writes `.inf` unquoted and this function, its
    /// own reader, refuses to read it back.
    ///
    /// **This arm is exact in effect, and the first version of this sentence
    /// gave the wrong reason for it.** It said "nothing else in either format
    /// produces a non-finite `f64`". What is actually true, measured: the only
    /// other candidate is a huge exponent, and `serde_norway` leaves `1e400`
    /// a **string** rather than resolving it to infinity — so this arm is
    /// reached only by `.inf`/`.nan`, but by luck of the other resolver rather
    /// than by construction. `1e400` is a separate, recorded divergence
    /// (Kestra answers `Infinity`; egret answers the string `1e400`, and
    /// `fromJson('1e400')` errors where Kestra answers `Infinity` because
    /// `serde_json` rejects it in the parser).
    ///
    /// The message names the canonical spelling, so `.Inf` and `.NaN` are
    /// reported as `.inf` and `.nan` where Kestra echoes the source — the raw
    /// text does not reach a `visit_f64`.
    /// `function-yaml-refuses-what-its-own-writer-wrote` is the entry.
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<MjValue, E> {
        finite_or_malformed(v)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<MjValue, E> {
        Ok(MjValue::from(v.to_string()))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<MjValue, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(Depth(self.0 + 1))? {
            items.push(item);
        }
        Ok(MjValue::from(items))
    }

    /// Insertion order, not sorted: `{"b":1,"a":2}` reads back `{"b":1,"a":2}`
    /// through both readers, which `function-fromjson-key-order` and
    /// `function-yaml-key-order` pin.
    ///
    /// **And a number, when the map came from `serde_json`.** This is the
    /// second of the crate's two readers that `arbitrary_precision` reroutes
    /// — `env::OrderedJsonVisitor::visit_map` is the other, and its
    /// doc carries the mechanism. `fromJson` is a JSON reader, so with the
    /// feature on every float and every past-64-bit integer in its input
    /// arrives here as a one-entry map under
    /// `env::ARBITRARY_PRECISION_TOKEN`;
    /// `function-fromjson-round-trip` is the entry that caught it. The `yaml`
    /// reader shares this visitor and is unaffected — `serde_norway` has no
    /// such feature and calls [`BoundedVisitor::visit_f64`] directly.
    ///
    /// Recognition is narrow and falls back rather than failing — see
    /// `env::OrderedJsonVisitor::visit_map`, which carries the
    /// reasoning and the test that forced it.
    ///
    /// One thing is not shared with that method: **a sentinel costs no
    /// depth.** It is a scalar wearing a map's shape, so its payload is read
    /// at this map's own depth. Seeding it one level down would make every
    /// float in a document cost a level of [`MAX_VALUE_DEPTH`], quietly
    /// moving a bound the crate's writers depend on.
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<MjValue, A::Error> {
        let mut entries: Vec<(String, MjValue)> = Vec::new();

        while let Some(key) = map.next_key::<String>()? {
            let sentinel = entries.is_empty() && key == env::ARBITRARY_PRECISION_TOKEN;
            let depth = if sentinel { self.0 } else { self.0 + 1 };
            let value = map.next_value_seed(Depth(depth))?;
            entries.push((key, value));
        }

        if let [(key, payload)] = entries.as_slice() {
            if key == env::ARBITRARY_PRECISION_TOKEN {
                if let Some(literal) = payload.as_str() {
                    if let Some(exact) = env::integer_from_literal(literal) {
                        return Ok(exact);
                    }
                    // Not an integer, so if it is a number at all it is a
                    // float — and it goes through the same non-finite refusal
                    // `visit_f64` applies, because `serde_json` no longer
                    // rejects an out-of-range exponent in its own parser once
                    // `arbitrary_precision` is on. `1e400` reaches this line
                    // as five characters. See
                    // `a_huge_exponent_is_refused_by_this_crate_now_and_not_by_serde_json`.
                    if let Ok(float) = literal.parse::<f64>() {
                        return finite_or_malformed(float);
                    }
                }
            }
        }

        Ok(MjValue::from_object(OrderedMap::new(entries)))
    }
}

/// A finite `f64` as a value; a non-finite one as Jackson's own complaint.
///
/// Extracted from [`BoundedVisitor::visit_f64`] so the
/// `arbitrary_precision` sentinel path in
/// [`BoundedVisitor::visit_map`] applies the identical rule — see that
/// method's doc. The message names the canonical spelling, so `.Inf` and
/// `.NaN` are reported as `.inf` and `.nan`.
fn finite_or_malformed<E: de::Error>(v: f64) -> Result<MjValue, E> {
    if !v.is_finite() {
        return Err(de::Error::custom(format!(
            "Malformed numeric value '{}'",
            if v.is_nan() {
                ".nan"
            } else if v > 0.0 {
                ".inf"
            } else {
                "-.inf"
            }
        )));
    }
    Ok(MjValue::from(v))
}

// ---------------------------------------------------------------------------
// range
// ---------------------------------------------------------------------------

/// `RenderLimits.ensureAtMost`'s ceiling for `range`, read off
/// `BoundedRangeFunction.MAX_RANGE_SIZE` in the artifact: `ConstantValue:
/// 1000000l`.
///
/// This is Kestra's own bound, ported rather than invented — one of the only
/// two `RenderLimits` call sites in the whole jar (the other is
/// `AbstractIndent`).
const MAX_RANGE_SIZE: i128 = 1_000_000;

/// `BoundedRangeFunction` — Pebble's `RangeFunction` with a size guard in
/// front of it.
///
/// The guard is `|end - start| / |increment| + 1`, **not** `|end - start|`,
/// and it runs only when `start` and `end` are both numbers and `increment`
/// is not zero. `function-range-counts-by-increment` is the entry that
/// separates the two: it spans two million and produces two elements.
///
/// # Two ways Kestra's own loop does not terminate, and egret refuses both
///
/// Neither has a golden, for the reason C7 states: at the sizes that matter
/// Kestra hangs rather than answering.
///
/// - **The guard's arithmetic is `Math.abs(end - start)` on Java longs, which
///   wraps.** `range(start=-9223372036854775808, end=9223372036854775807)` is
///   `abs(-1)` = 1, so the guard sees two elements and the loop then runs
///   1.8 × 10^19 times. The count here is computed in `i128`, so it cannot
///   wrap, and the call is refused.
/// - **A character range's increment is truncated to an `int` by the loop's
///   own `(int)((long) i + increment)`.** An increment of 2^32 therefore
///   leaves `i` unchanged and the loop never ends. Refused, by name.
fn range(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let start = args.get("start", 0).unwrap_or_default();
    let end = args.get("end", 1).unwrap_or_default();
    let increment = match args.get("increment", 2) {
        None => 1i64,
        Some(value) if is_java_null(&value) => 1i64,
        Some(value) if value.kind() == ValueKind::Number => number_as_long(&value)?,
        Some(value) => {
            // The trailing space is Pebble's own constant, proven by the
            // golden: `must be a number x`, not `numberx`. One character, and
            // the test that was meant to hold it pinned the wrong sentence
            // instead -- exactly what error-class comparison hides.
            return Err(pebble_error(format!(
                "The increment of the range function must be a number {}",
                super::env::kestra_to_string(&value)
            )));
        }
    };

    if start.kind() == ValueKind::Number && end.kind() == ValueKind::Number {
        let from = number_as_long(&start)?;
        let to = number_as_long(&end)?;
        if increment != 0 {
            let count = (i128::from(to) - i128::from(from)).abs() / i128::from(increment).abs() + 1;
            if count > MAX_RANGE_SIZE {
                return Err(pebble_error(format!(
                    "The range function cannot produce more than {MAX_RANGE_SIZE} elements, \
                     but this call would produce {count}."
                )));
            }
        }
        return numeric_range(from, to, increment);
    }

    let (Some(from), Some(to)) = (start.as_str(), end.as_str()) else {
        return Err(pebble_error(
            "Arguments of range function must be of type Number or String with a length of 1",
        ));
    };
    let (from, to) =
        match (one_char(from), one_char(to)) {
            (Some(from), Some(to)) => (from, to),
            _ => return Err(pebble_error(
                "Arguments of range function must be of type Number or String with a length of 1",
            )),
        };
    char_range(from, to, increment)
}

/// `Number.longValue()` on a range bound.
fn number_as_long(value: &MjValue) -> Result<i64, MjError> {
    if value.is_integer() {
        return i64::try_from(value.clone()).map_err(|_| pebble_error("long overflow"));
    }
    f64::try_from(value.clone())
        .map(|f| f as i64)
        .map_err(|_| pebble_error("long overflow"))
}

/// One UTF-16 code unit, which is what `String.charAt(0)` reads and what the
/// `length() != 1` guard measures.
fn one_char(s: &str) -> Option<u32> {
    let mut units = s.encode_utf16();
    let first = units.next()?;
    units.next().is_none().then_some(u32::from(first))
}

fn numeric_range(from: i64, to: i64, increment: i64) -> Result<MjValue, MjError> {
    if increment == 0 {
        return Err(pebble_error(
            "The increment of the range function must be different than 0",
        ));
    }
    let mut items = Vec::new();
    let mut at = from;
    loop {
        if increment > 0 && at > to {
            break;
        }
        if increment < 0 && at < to {
            break;
        }
        items.push(MjValue::from(at));
        // Java's `long` addition wraps and its loop then never ends; a
        // checked add turns that into a finite range instead.
        match at.checked_add(increment) {
            Some(next) => at = next,
            None => break,
        }
    }
    Ok(MjValue::from(items))
}

fn char_range(from: u32, to: u32, increment: i64) -> Result<MjValue, MjError> {
    if increment == 0 {
        return Err(pebble_error(
            "The increment of the range function must be different than 0",
        ));
    }
    // `(int)((long) i + increment)`: an increment whose low 32 bits are zero
    // does not move the loop variable at all.
    let step = increment as i32;
    if step == 0 {
        return Err(pebble_error(
            "the range function's increment truncates to 0 as a 32-bit character step, \
             which never advances the range",
        ));
    }
    let mut items = Vec::new();
    let mut at = from as i32;
    loop {
        if step > 0 && at > to as i32 {
            break;
        }
        if step < 0 && at < to as i32 {
            break;
        }
        let unit = u16::try_from(at & 0xffff).expect("masked to 16 bits");
        items.push(MjValue::from(String::from_utf16_lossy(&[unit])));
        match at.checked_add(step) {
            Some(next) => at = next,
            None => break,
        }
    }
    Ok(MjValue::from(items))
}

// ---------------------------------------------------------------------------
// The calendar functions
// ---------------------------------------------------------------------------

/// `(String) args.get("date")`, then `DateUtils.parseLocalDate`.
///
/// The cascade is `LocalDate.parse` -> `ZonedDateTime.parse().toLocalDate()`
/// -> `LocalDateTime.parse().toLocalDate()`, and every one of them reads the
/// **local** date, so `2024-01-06T14:30:00Z` is the 6th whatever zone anyone
/// asks about it. [`parse_iso`] is that grammar.
fn local_date(args: &Args, function: &str) -> Result<NaiveDate, MjError> {
    Ok(date_time(args, function)?.date())
}

fn date_time(args: &Args, function: &str) -> Result<NaiveDateTime, MjError> {
    let value = args.get("date", 0).filter(|v| !is_java_null(v));
    let Some(value) = value else {
        return Err(pebble_error(format!(
            "The '{function}()' function expects a 'date' argument."
        )));
    };
    let text = super::env::kestra_to_string(&value);
    let zone = Zone::resolve(None).map_err(pebble_error)?;
    let zdt: Zdt = parse_iso(&text, &zone).map_err(|e| {
        pebble_error(format!(
            "The '{function}()' function could not parse 'date': {e}"
        ))
    })?;
    Ok(zdt.local())
}

/// `DayOfMonthFunction` — `parseLocalDate(date).getDayOfMonth()`.
fn day_of_month(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    Ok(MjValue::from(local_date(&args, "dayOfMonth")?.day()))
}

/// `MonthOfYearFunction` — `parseLocalDate(date).getMonthValue()`.
fn month_of_year(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    Ok(MjValue::from(local_date(&args, "monthOfYear")?.month()))
}

/// `DayOfWeekFunction` — `parseLocalDate(date).getDayOfWeek().name()`, so the
/// answer is the enum's own English name and no locale reaches it.
fn day_of_week(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let date = local_date(&args, "dayOfWeek")?;
    Ok(MjValue::from(day_name(date)))
}

/// `HourOfDayFunction` — `ZonedDateTime.parse(...).getHour()`, falling back to
/// `LocalDateTime.parse(...).getHour()`.
///
/// **This is the one calendar function that does not use
/// `DateUtils.parseLocalDate`**, and the difference shows: a date with no time
/// at all is an error here (`Text '2024-01-06' could not be parsed at index
/// 10`) where every sibling accepts it. And `getHour()` is the hour of the
/// **parsed offset**, not of UTC — `2024-01-06T14:30:00+07:00` is 14.
fn hour_of_day(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let value = args.get("date", 0).filter(|v| !is_java_null(v));
    let Some(value) = value else {
        return Err(pebble_error(
            "The 'hourOfDay()' function expects a 'date' argument.",
        ));
    };
    let text = super::env::kestra_to_string(&value);
    // Neither `ZonedDateTime.parse` nor `LocalDateTime.parse` accepts a
    // date-only string; only `LocalDate.parse`, which this function does not
    // call, does.
    if !text.contains(['T', 't']) {
        return Err(pebble_error(format!(
            "The 'hourOfDay()' function could not parse 'date': Text '{text}' could not be \
             parsed: it carries no time"
        )));
    }
    Ok(MjValue::from(date_time(&args, "hourOfDay")?.hour()))
}

/// `IsWeekendFunction` — `SATURDAY` or `SUNDAY`, spelled out in the bytecode.
///
/// **No `WeekFields`, so no locale.** The four-letter enum constants are
/// compared directly, which is why this one is answerable where `w`, `W`,
/// `Y`, `e` and `c` are refused in [`super::javafmt`]: those resolve through
/// `WeekFields.of(locale)` and this does not.
fn is_weekend(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let date = local_date(&args, "isWeekend")?;
    Ok(MjValue::from(is_weekend_day(date)))
}

fn is_weekend_day(date: NaiveDate) -> bool {
    matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun)
}

fn day_name(date: NaiveDate) -> &'static str {
    match date.weekday() {
        chrono::Weekday::Mon => "MONDAY",
        chrono::Weekday::Tue => "TUESDAY",
        chrono::Weekday::Wed => "WEDNESDAY",
        chrono::Weekday::Thu => "THURSDAY",
        chrono::Weekday::Fri => "FRIDAY",
        chrono::Weekday::Sat => "SATURDAY",
        chrono::Weekday::Sun => "SUNDAY",
    }
}

/// `IsLastWorkingDayFunction`.
///
/// `workingDays` is split on `[,\s]+`, upper-cased and read through
/// `DayOfWeek.valueOf`; a blank or absent argument is Monday-to-Friday. The
/// answer is then: the date is a working day, **and** walking back from the
/// last day of its month over non-working days lands on it.
///
/// A blank `workingDays` is `isBlank()`, not `isEmpty()`, so `'   '` is the
/// default set rather than an error — measured.
fn is_last_working_day(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let date = local_date(&args, "isLastWorkingDay")?;
    let working = working_days(&args)?;

    // **This guard is an EQUIVALENT MUTANT and it is kept on purpose.**
    // Deleting it changes no answer, because the walk below always ends on a
    // working day, so `last == date` already implies that `date` is one. A
    // mutation run replaced the condition with `false` and nothing failed;
    // that is the implementation being redundant, not the tests being weak,
    // and Kestra's own code has the same shape. Recorded here so the next
    // reader does not re-derive it or "fix" the tests that cannot kill it.
    if !working.contains(&day_name(date)) {
        return Ok(MjValue::from(false));
    }
    let mut last = last_day_of_month(date);
    while !working.contains(&day_name(last)) {
        let Some(previous) = last.checked_sub_days(Days::new(1)) else {
            return Ok(MjValue::from(false));
        };
        last = previous;
    }
    Ok(MjValue::from(last == date))
}

const DEFAULT_WORKING_DAYS: &[&str] = &["MONDAY", "TUESDAY", "WEDNESDAY", "THURSDAY", "FRIDAY"];

fn working_days(args: &Args) -> Result<Vec<&'static str>, MjError> {
    let raw = match args.get("workingDays", 1) {
        None => None,
        Some(value) if is_java_null(&value) => None,
        Some(value) => Some(super::env::kestra_to_string(&value)),
    };
    let Some(raw) = raw.filter(|text| !text.trim().is_empty()) else {
        return Ok(DEFAULT_WORKING_DAYS.to_vec());
    };
    let mut days: Vec<&'static str> = Vec::new();
    for token in raw.split(|c: char| c == ',' || c.is_whitespace()) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let Some(day) = DAY_NAMES
            .iter()
            .find(|name| name.eq_ignore_ascii_case(token))
        else {
            return Err(pebble_error(format!(
                "The 'isLastWorkingDay()' function received an invalid 'workingDays' value: \
                 '{token}'. Expected day names like MONDAY, TUESDAY, etc."
            )));
        };
        if !days.contains(day) {
            days.push(day);
        }
    }
    if days.is_empty() {
        return Err(pebble_error(
            "The 'isLastWorkingDay()' function requires at least one valid day in 'workingDays'.",
        ));
    }
    Ok(days)
}

const DAY_NAMES: &[&str] = &[
    "MONDAY",
    "TUESDAY",
    "WEDNESDAY",
    "THURSDAY",
    "FRIDAY",
    "SATURDAY",
    "SUNDAY",
];

fn last_day_of_month(date: NaiveDate) -> NaiveDate {
    let first = NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap_or(date);
    first
        .checked_add_months(Months::new(1))
        .and_then(|next| next.pred_opt())
        .unwrap_or(date)
}

/// `IsDayWeekInMonthFunction` — `TemporalAdjusters.firstInMonth(day)` plus 0,
/// 1, 2 or 3 weeks, or `lastInMonth(day)`, compared with `isEqual`.
///
/// **`FOURTH` is not `LAST`.** `firstInMonth(...).plusWeeks(3)` can leave the
/// month entirely when the first occurrence falls late, so a five-occurrence
/// month has a `FOURTH` and a `LAST` that are different days.
/// `function-isdayweekinmonth-fourth-is-not-last` is the entry.
fn is_day_week_in_month(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let date = local_date(&args, "isDayWeekInMonth")?;

    let day = args.get("dayOfWeek", 1).filter(|v| !is_java_null(v));
    let Some(day) = day else {
        return Err(pebble_error(
            "The 'isDayWeekInMonth()' function expects a 'dayOfWeek' argument.",
        ));
    };
    let position = args.get("position", 2).filter(|v| !is_java_null(v));
    let Some(position) = position else {
        return Err(pebble_error(
            "The 'isDayWeekInMonth()' function expects a 'position' argument.",
        ));
    };

    let day_text = super::env::kestra_to_string(&day).to_uppercase();
    let Some(day) = DAY_NAMES.iter().find(|name| **name == day_text) else {
        return Err(pebble_error(format!(
            "The 'isDayWeekInMonth()' function received an invalid 'dayOfWeek' value: \
             '{day_text}'. Expected one of: MONDAY, TUESDAY, WEDNESDAY, THURSDAY, FRIDAY, \
             SATURDAY, SUNDAY."
        )));
    };
    let position_text = super::env::kestra_to_string(&position).to_uppercase();
    let weeks = match position_text.as_str() {
        "FIRST" => 0u32,
        "SECOND" => 1,
        "THIRD" => 2,
        "FOURTH" => 3,
        "LAST" => {
            let target = last_in_month(date, day);
            return Ok(MjValue::from(target == Some(date)));
        }
        other => {
            return Err(pebble_error(format!(
                "The 'isDayWeekInMonth()' function received an invalid 'position' value: \
                 '{other}'. Expected one of: FIRST, SECOND, THIRD, FOURTH, LAST."
            )))
        }
    };
    let target = first_in_month(date, day)
        .and_then(|first| first.checked_add_days(Days::new(u64::from(weeks) * 7)));
    Ok(MjValue::from(target == Some(date)))
}

fn first_in_month(date: NaiveDate, day: &str) -> Option<NaiveDate> {
    let mut at = NaiveDate::from_ymd_opt(date.year(), date.month(), 1)?;
    for _ in 0..7 {
        if day_name(at) == day {
            return Some(at);
        }
        at = at.checked_add_days(Days::new(1))?;
    }
    None
}

fn last_in_month(date: NaiveDate, day: &str) -> Option<NaiveDate> {
    let mut at = last_day_of_month(date);
    for _ in 0..7 {
        if day_name(at) == day {
            return Some(at);
        }
        at = at.checked_sub_days(Days::new(1))?;
    }
    None
}

/// `IsPublicHolidayFunction` — **refused**, and refused by name.
///
/// Kestra answers this out of `de.focus_shift.jollyday` 2.19.0, an XML
/// holiday calendar for ~70 countries with per-subdivision rules. There is no
/// such database in this tree and no Rust crate carrying jollyday's data, so
/// egret has exactly two options: answer a different question, or say so.
///
/// It says so. This is the same call [`super::javafmt`] makes for `z`, `v`
/// and `B` — the three pattern letters whose text lives in a locale database
/// — and for the same reason: a plausible answer that is wrong on a date a
/// flow branches on is worse than a refusal an author can read. Supporting
/// one country and refusing the rest would be worse still, because an author
/// cannot predict which.
///
/// `function-ispublicholiday` stays in `oracle/baseline.txt` with this
/// reason.
fn is_public_holiday(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    // The two argument checks first, so an author with a broken call reads
    // Kestra's own message rather than the refusal.
    if args.get("date", 0).filter(|v| !is_java_null(v)).is_none() {
        return Err(pebble_error(
            "The 'isPublicHoliday()' function expects a 'date' argument.",
        ));
    }
    // **`countryCode` is only NULL-checked.** The `isBlank()` in the bytecode
    // is on `subDivision`, not here: measured,
    // `isPublicHoliday(date='2024-01-01', countryCode='  ')` answers `true` in
    // Kestra, and an earlier cut of this refused it.
    if args
        .get("countryCode", 1)
        .filter(|v| !is_java_null(v))
        .is_none()
    {
        return Err(pebble_error(
            "The 'isPublicHoliday()' function expects a 'countryCode' argument.",
        ));
    }
    Err(pebble_error(
        "egret cannot answer isPublicHoliday(): Kestra reads it out of the jollyday holiday \
         calendar (de.focus_shift.jollyday 2.19.0), a per-country, per-subdivision database \
         this port does not carry. Refusing rather than answering a date a flow may branch on.",
    ))
}

// ---------------------------------------------------------------------------
// now
// ---------------------------------------------------------------------------

/// `NowFunction` — `AbstractDate.format(ZonedDateTime.now(), args, ctx)`, so
/// it takes the same four arguments the `date` filter does and goes through
/// the same formatter.
///
/// **The default zone is UTC here and `ZoneId.systemDefault()` in Kestra**,
/// the divergence [`super::javafmt::Zone::resolve`] already records for
/// `date`. No corpus entry can hold it: an entry that reached it would not be
/// reproducible on another machine.
fn now(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let date_args = DateArgs::read(&args, 0)?;
    let instant = Utc::now();
    let zone = Zone::resolve(None).map_err(pebble_error)?;
    let zdt = Zdt::from_epoch(instant.timestamp(), instant.timestamp_subsec_nanos(), zone)
        .ok_or_else(|| pebble_error("egret's dates are chrono's"))?;
    Ok(MjValue::from(date_args.render(&zdt)?))
}

// ---------------------------------------------------------------------------
// The identifier generators
// ---------------------------------------------------------------------------

/// `UUIDFunction` — `Generators.timeBasedEpochRandomGenerator().generate()`,
/// i.e. **UUIDv7**, rendered by `java.util.UUID.toString()` (lower-case hex).
///
/// A shape check sees 36 characters and four hyphens. What it cannot see, and
/// what this pins, is the layout: 48 bits of Unix milliseconds, the version
/// nibble `7`, 12 random bits, the variant bits `10`, and 62 more random
/// bits. `function-uuid-is-a-version-7-uuid` asserts exactly that through the
/// oracle, as a `regexMatch` whose answer is the deterministic `true`.
///
/// `TimeBasedEpochRandomGenerator` is the fully-random variant of v7, not the
/// counter-based one, so two UUIDs minted inside the same millisecond are not
/// ordered relative to each other on either engine.
fn uuid(_rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let millis = Utc::now().timestamp_millis().max(0) as u64 & 0x0000_ffff_ffff_ffff;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[0..6].copy_from_slice(&millis.to_be_bytes()[2..8]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(MjValue::from(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )))
}

/// The base-62 digits `friendly-id` and `ksuid` both use. Same alphabet, same
/// order, read out of both jars' `static {}` blocks.
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// `IDFunction` — `IdUtils.create()`, which is
/// `FriendlyId.createFriendlyId()`: a random **UUIDv4**, read as an unsigned
/// 128-bit integer and written in base 62 with **no padding**.
///
/// # The length is 21 or 22, and `tests/oracle.rs` used to insist on 22
///
/// `Base62.encode` loops while the value is positive and pads nothing, so the
/// digit count is `ceil(log62(value))`. `62^21` is 4.37 × 10^37 and a UUIDv4
/// spans 2^128 = 3.40 × 10^38, so **12.8 % of ids are 21 characters**, 0.2 %
/// are 20, and none is ever more than 22.
///
/// This is not a deduction: the golden file's own previous value for
/// `function-now`'s neighbour `function-id` was `iUBruItK7qj9tMOMNVPAr`, 21
/// characters, generated by Kestra itself. The shape check read
/// `value.len() == 22` and would have failed roughly one run in eight against
/// a faithful port.
fn id(_rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(MjValue::from(base62(u128::from_be_bytes(bytes), 0)))
}

/// `KSUIDFunction` — `new KsuidGenerator(new SecureRandom()).newKsuid()`.
///
/// 20 bytes: a big-endian `int` of `epochSecond - 1_400_000_000` followed by
/// 16 random ones, base-62 encoded and **left-padded to 27**
/// (`Ksuid.PAD_TO_LENGTH`). The padding is what makes the length constant,
/// unlike `id` above; and the timestamp prefix is what makes two ksuids
/// minted a second apart sort in the order they were minted, which no length
/// check can see.
fn ksuid(_rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let mut bytes = [0u8; 20];
    let stamp = (Utc::now().timestamp() - 1_400_000_000) as i32;
    bytes[0..4].copy_from_slice(&stamp.to_be_bytes());
    rand::thread_rng().fill_bytes(&mut bytes[4..20]);
    // 20 bytes do not fit in a `u128`, so the value is divided as a
    // big-endian digit string rather than as one integer.
    Ok(MjValue::from(base62_bytes(&bytes, 27)))
}

/// `NanoIDFunction`.
///
/// The default alphabet is 64 characters — read out of the class's `static {}`
/// block, `0-9A-Za-z-_` in that order — and the default length is 21. Both
/// arguments are read with `instanceof`, so a `length` that is not a `Long`
/// and an `alphabet` that is not a `String` are **silently ignored** rather
/// than refused; `function-nanoid-length-is-ignored-unless-it-is-a-long` is
/// the entry, and it answers 21 `x`s.
///
/// # Two selection paths, and the alphabet's length picks between them
///
/// A power-of-two alphabet is masked (`alphabet[byte & (len - 1)]`); anything
/// else is rejection-sampled against `256 - 256 % len`, refilling the buffer
/// as it goes. Both are ported, and both are pinned deterministically by a
/// one-character alphabet: `'x'` takes the masked path (length 1 is a power
/// of two) and `'xxx'` takes the rejection path.
fn nano_id(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);

    let mut length = 21usize;
    if let Some(value) = args.get("length", 0) {
        if value.kind() == ValueKind::Number && value.is_integer() {
            let raw = i64::try_from(value.clone()).map_err(|_| pebble_error("long overflow"))?;
            if raw < 1 {
                return Err(pebble_error(
                    "The 'nanoId()' function field 'length' must be greater than: 0",
                ));
            }
            if raw > 1000 {
                return Err(pebble_error(
                    "The 'nanoId()' function field 'length' must be lower than: 1000",
                ));
            }
            length = raw as usize;
        }
    }

    let default: Vec<char> = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-_"
        .chars()
        .collect();
    let alphabet: Vec<char> = match args.get("alphabet", 1) {
        Some(value) if value.as_str().is_some() => {
            value.as_str().expect("checked").chars().collect()
        }
        _ => default,
    };
    if alphabet.is_empty() {
        return Err(pebble_error(
            "The 'nanoId()' function field 'alphabet' must not be empty",
        ));
    }
    // **`String.toCharArray().length` counts UTF-16 code units**, not
    // characters, so a 200-astral-character alphabet is 400 units and Kestra
    // refuses it. Measured; an earlier cut counted `char`s and accepted it.
    // The selection paths below index by `char`, which is the same set in a
    // different unit -- the count is the only place the two differ.
    let utf16_units: usize = alphabet.iter().map(|c| c.len_utf16()).sum();
    if utf16_units > 256 {
        return Err(pebble_error(
            "The 'nanoId()' function field 'alphabet' must not contain more than: 256 characters",
        ));
    }

    let mut rng = rand::thread_rng();
    let mut out = String::with_capacity(length);
    if alphabet.len() & (alphabet.len() - 1) == 0 {
        let mask = alphabet.len() - 1;
        let mut bytes = vec![0u8; length];
        rng.fill_bytes(&mut bytes);
        for byte in bytes {
            out.push(alphabet[usize::from(byte) & mask]);
        }
        return Ok(MjValue::from(out));
    }

    let limit = 256 - 256 % alphabet.len();
    let mut bytes = vec![0u8; length];
    let mut at = bytes.len();
    // Counted rather than re-measured: `out.chars().count()` in the condition
    // makes the loop quadratic in `length`, which at the 1000-character
    // maximum is half a million wasted steps for no reason.
    let mut written = 0usize;
    while written < length {
        if at == bytes.len() {
            rng.fill_bytes(&mut bytes);
            at = 0;
        }
        let byte = usize::from(bytes[at]);
        at += 1;
        if byte < limit {
            out.push(alphabet[byte % alphabet.len()]);
            written += 1;
        }
    }
    Ok(MjValue::from(out))
}

/// `RandomIntFunction` —
/// `(int) (Math.floor(Math.random() * (upper - lower)) + lower)`.
///
/// **`upper` is exclusive and `lower` is inclusive**, which is the thing the
/// oracle's shape check (`(1..10).contains(&n)`) states and cannot verify.
/// Three corpus entries verify it by exact value instead, because a one-wide
/// range has only one answer.
///
/// Both arguments are read with `instanceof Long`, so a numeric **string** is
/// refused rather than parsed — and all three of this function's messages
/// spell the function `randomIn()`, missing the `t`. That typo is upstream
/// and it is reproduced, because a flow author reading an egret error must
/// read the sentence Kestra would have shown them.
fn random_int(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let lower = random_int_argument(&args, "lower", 0)?;
    let upper = random_int_argument(&args, "upper", 1)?;
    if upper < lower {
        return Err(pebble_error("In 'randomIn()' upper is less than lower"));
    }
    // **`upper - lower` is Java `long` arithmetic and it WRAPS**, so
    // `randomInt(lower=Long.MIN_VALUE, upper=Long.MAX_VALUE)` has a span of
    // -1 there rather than 2^64. A plain `-` here is an arithmetic overflow:
    // a panic in a debug build and a silent wrap in a release one, from a
    // template. `wrapping_sub` is Kestra's arithmetic and `as i32` is Java's
    // `d2i`, which saturates at the `int` bounds exactly as Rust's does.
    //
    // Found by attacking the function rather than by reading it, and it
    // panicked the render thread -- which `tests/oracle.rs` treats as a
    // mismatch in every mode, but which no corpus entry reached.
    let span = upper.wrapping_sub(lower) as f64;
    let draw = rand::Rng::gen_range(&mut rand::thread_rng(), 0.0f64..1.0);
    let value = (draw * span).floor() + lower as f64;
    Ok(MjValue::from(value as i32))
}

fn random_int_argument(args: &Args, name: &str, index: usize) -> Result<i64, MjError> {
    let Some(value) = args.get(name, index) else {
        return Err(pebble_error(format!(
            "The 'randomIn()' function expects an argument {name}"
        )));
    };
    if value.kind() != ValueKind::Number || !value.is_integer() {
        return Err(pebble_error(format!(
            "The 'randomIn()' function expects an argument {name} of type Long."
        )));
    }
    i64::try_from(value.clone()).map_err(|_| pebble_error("long overflow"))
}

/// `RandomPortFunction` — `new ServerSocket(0).getLocalPort()`.
///
/// It is a real bind, and it has to be: the point of the function is a port
/// nothing else is listening on, which only the kernel can answer. The
/// listener is closed immediately (Java's try-with-resources; here, the drop
/// at the end of this function), so what comes back is a port that was free a
/// moment ago and not one that is reserved.
///
/// The range is therefore the host's ephemeral range, not `1..=65535`, and
/// neither engine avoids a well-known port because neither chooses the number
/// — `ServerSocket(0)` and `TcpListener::bind(":0")` both ask the kernel,
/// which never hands out a port in use.
fn random_port(_rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let listener =
        TcpListener::bind(("0.0.0.0", 0)).map_err(|_| pebble_error("Unable to get random port"))?;
    let port = listener
        .local_addr()
        .map_err(|_| pebble_error("Unable to get random port"))?
        .port();
    Ok(MjValue::from(i32::from(port)))
}

/// `Base62.encode(BigInteger)`: divide-and-remainder down to zero, most
/// significant digit first, left-padded to `pad` with `0`.
///
/// `friendly-id`'s encoder passes `pad = 0` (no padding at all) and `ksuid`'s
/// passes 27. That single argument is the whole difference between `id`'s
/// variable length and `ksuid`'s constant one.
pub(crate) fn base62(mut value: u128, pad: usize) -> String {
    let mut digits = Vec::new();
    while value > 0 {
        digits.push(BASE62[(value % 62) as usize]);
        value /= 62;
    }
    if digits.is_empty() {
        digits.push(BASE62[0]);
    }
    while digits.len() < pad {
        digits.push(BASE62[0]);
    }
    digits.reverse();
    String::from_utf8(digits).expect("base62 digits are ASCII")
}

/// The same encoding over a byte string too wide for a `u128` — `ksuid`'s 20
/// bytes. Long division over base-256 digits, which is what
/// `BigInteger.divideAndRemainder` does.
fn base62_bytes(bytes: &[u8], pad: usize) -> String {
    let mut value: Vec<u8> = bytes.to_vec();
    let mut digits = Vec::new();
    while value.iter().any(|b| *b != 0) {
        let mut remainder = 0u32;
        for byte in value.iter_mut() {
            let current = (remainder << 8) | u32::from(*byte);
            *byte = (current / 62) as u8;
            remainder = current % 62;
        }
        digits.push(BASE62[remainder as usize]);
    }
    if digits.is_empty() {
        digits.push(BASE62[0]);
    }
    while digits.len() < pad {
        digits.push(BASE62[0]);
    }
    digits.reverse();
    String::from_utf8(digits).expect("base62 digits are ASCII")
}

// ===========================================================================
// W2 Task 8 — the rest of the 34: seven ports, twelve refusals
// ===========================================================================
//
// See this module's doc for the split. What follows is measured against
// `io.kestra:core:2.0.0-rc13` with `javap -p -c -constants`, and every claim
// about an answer is a corpus entry rendered through the harness.

/// The message every Task 8 refusal carries.
///
/// **A refusal is a feature, and it has to say what it is waiting for.** The
/// oracle cannot check this: those entries are `compare: "error-class"`, which
/// any failure satisfies — including `unknown function: secret is unknown`,
/// which is what egret said before this task and which tells a flow author
/// nothing. So the shape is pinned by a unit test per function instead
/// (`runners::functions::the_eight_refusals_name_themselves_and_their_owner`),
/// and this one function is where the shape lives so the messages cannot
/// drift apart.
///
/// Three things are in it on purpose: the function's own wire name, what it
/// needs, and the **package that will implement it** — so the reader can find
/// the plan. Never empty, never null: an empty refusal is a wrong value that
/// renders successfully, which is the failure mode this whole programme keeps
/// meeting.
fn not_implemented(name: &str, needs: &str, package: &str) -> MjError {
    pebble_error(format!(
        "The '{name}' function is not implemented yet: it needs {needs}, \
         which egret's {package} package provides."
    ))
}

/// `EnvFunction` — `envs` out of the evaluation context, with a fallback.
///
/// Pure, despite the name: it never reads the process environment. Kestra's
/// runner puts a `Map<String, String> envs` into the render context (built
/// from `KESTRA_`-prefixed process variables plus configuration), and this
/// function only reads that variable. So it is portable today and W3 decides
/// what goes into `envs`.
///
/// The bytecode, in order, because three of the five branches are easy to get
/// backwards:
///
/// 1. `name` absent → `PebbleException`. **`containsKey`, not a null check**,
///    so `env(name=null)` is a *present* argument;
/// 2. `(String) args.get("name")` → a non-`String` is a `ClassCastException`.
///    A Java `null` passes a checkcast, which is why (1) and this differ;
/// 3. `context.getVariable("envs")` — absent, or present and not a `Map`, and
///    the **default is returned**, not an error;
/// 4. the value is `null` **or** `value.toString().isEmpty()` → the default.
///    An empty string is therefore indistinguishable from an absent key;
/// 5. otherwise the raw value, **not** its string form:
///    `env(name='A')` over `{"envs": {"A": {"k": 1}}}` renders `{"k":1}`.
///
/// The default itself is `args.get("default")` with no `containsKey` guard,
/// so an absent `default` is Java `null` and renders as nothing.
fn env_function(state: &State, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let Some(name) = args.get("name", 0) else {
        return Err(pebble_error(
            "The 'env' function expects an argument 'name'.",
        ));
    };
    let default = args.get("default", 1).unwrap_or_else(|| MjValue::from(()));

    // Branch 2. A Java `null` passes `checkcast`, and `Map.get(null)` then
    // finds nothing, so a null `name` falls all the way through to the
    // default rather than throwing.
    if is_java_null(&name) {
        return Ok(default);
    }
    let Some(key) = name.as_str() else {
        return Err(pebble_error(format!(
            "class {} cannot be cast to class java.lang.String",
            java_class_name(&name)
        )));
    };

    let Some(envs) = state.lookup("envs") else {
        return Ok(default);
    };
    if envs.kind() != ValueKind::Map {
        return Ok(default);
    }
    let Ok(value) = envs.get_item(&MjValue::from(key)) else {
        return Ok(default);
    };
    if is_java_null(&value) || super::env::kestra_to_string(&value).is_empty() {
        return Ok(default);
    }
    Ok(value)
}

/// `EncryptFunction` → `EncryptionService.encrypt(String key, String plaintext)`.
///
/// `AES/GCM/NoPadding` with a **12-byte random IV prefixed to the ciphertext**
/// and a 128-bit auth tag, the key base64-decoded into a `SecretKeySpec`, the
/// whole thing base64-encoded. Read out of `javap -p -c EncryptionService`.
///
/// **The order of the guards is load-bearing.** `EncryptionService.encrypt`
/// tests the *plaintext* first — `if (plaintext == null || plaintext.isEmpty())
/// return plaintext;` — and only then decodes the key, so
/// `encrypt(key='!!!', plaintext='')` is `''` and not a base64 error. A port
/// that validated the key first would refuse a call Kestra answers.
///
/// The IV is fresh on every call, so `function-encrypt`'s corpus entry is in
/// `tests/oracle.rs`'s `NON_DETERMINISTIC` set. That is a shape check, and a
/// shape check cannot see an IV length or a tag length — so the round trip
/// `decrypt(key=k, encrypted=encrypt(key=k, plaintext=…))` is a *deterministic*
/// corpus entry and it is what actually pins the format.
fn encrypt(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let (Some(key), Some(plaintext)) = (args.get("key", 0), args.get("plaintext", 1)) else {
        return Err(pebble_error(
            "The 'encrypt' function expects two arguments 'key' and 'plaintext'.",
        ));
    };
    let Some(text) = java_string_arg(&plaintext)? else {
        // `plaintext == null` returns `plaintext`, i.e. null.
        return Ok(MjValue::from(()));
    };
    if text.is_empty() {
        return Ok(MjValue::from(text));
    }
    // **A bound Kestra does not have, and the reason it is needed is that
    // `encrypt` GROWS.** Base64 of `12 + n + 16` bytes is `4/3 (n + 28)`, so
    // nesting the call multiplies: measured on the shipped path, 30 levels is
    // 672 KB in 6 ms, 48 levels is 1 686 ms, and the trend puts 60 levels at
    // ~16 GB. `super::rewrite`'s nesting cap does not see a function-call
    // chain, and the output cap sees only the LAST value, after every
    // intermediate has been allocated. Capping each level's INPUT at the
    // output ceiling caps the whole chain: nothing over 1 MiB can be rendered
    // anyway.
    if text.len() > super::env::MAX_OUTPUT_BYTES {
        return Err(pebble_error(format!(
            "the 'encrypt' function refuses a plaintext over {} bytes",
            super::env::MAX_OUTPUT_BYTES
        )));
    }
    let key = decode_aes_key(&key)?;
    let mut iv = [0u8; AES_GCM_IV_BYTES];
    rand::thread_rng().fill_bytes(&mut iv);
    let mut out = iv.to_vec();
    out.extend_from_slice(&aes_gcm_seal(&key, &iv, text.as_bytes())?);
    Ok(MjValue::from(BASE64.encode(out)))
}

/// `DecryptFunction` → `EncryptionService.decrypt(String key, String encrypted)`.
///
/// The inverse, with the same null/empty short-circuit ahead of the key.
///
/// **A ciphertext shorter than the IV is an `IllegalArgumentException`, not a
/// tag mismatch**, and the two are different sentences a flow author will see:
/// `Arrays.copyOf(data, 12)` zero-pads happily, and it is the *next* line,
/// `Arrays.copyOfRange(data, 12, data.length)`, that throws `12 > 3`. Measured
/// (`function-decrypt-shorter-than-the-iv`).
fn decrypt(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let (Some(key), Some(encrypted)) = (args.get("key", 0), args.get("encrypted", 1)) else {
        return Err(pebble_error(
            "The 'decrypt' function expects two arguments 'key' and 'encrypted'.",
        ));
    };
    let Some(text) = java_string_arg(&encrypted)? else {
        return Ok(MjValue::from(()));
    };
    if text.is_empty() {
        return Ok(MjValue::from(text));
    }
    // The same ceiling, for symmetry and for the same reason: a plaintext
    // that could not be rendered is not worth decrypting, and this keeps the
    // decode allocation bounded too.
    if text.len() > super::env::MAX_OUTPUT_BYTES {
        return Err(pebble_error(format!(
            "the 'decrypt' function refuses a ciphertext over {} bytes",
            super::env::MAX_OUTPUT_BYTES
        )));
    }
    let data = BASE64
        .decode(text.as_bytes())
        .map_err(|_| pebble_error("Illegal base64 character"))?;
    if data.len() < AES_GCM_IV_BYTES {
        return Err(pebble_error(format!("{AES_GCM_IV_BYTES} > {}", data.len())));
    }
    let key = decode_aes_key(&key)?;
    let iv: [u8; AES_GCM_IV_BYTES] = data[..AES_GCM_IV_BYTES]
        .try_into()
        .expect("the length was just checked");
    let plain = aes_gcm_open(&key, &iv, &data[AES_GCM_IV_BYTES..])?;
    // `new String(byte[])` on JDK 18+ is UTF-8 with replacement, never a
    // throw -- `String.CASE_INSENSITIVE_ORDER`'s charset default changed in
    // JEP 400. `from_utf8_lossy` is that.
    Ok(MjValue::from(String::from_utf8_lossy(&plain).into_owned()))
}

/// `FetchContextFunction`, registered **twice** — as `fetchContext` and as
/// `printContext`. One class, two `put` calls (`NAME` and `ALIAS`), which is
/// why 33 classes give 34 names.
///
/// It collects every key of every *global* scope and answers a map of
/// name → value. In egret that is the render context plus whatever `{% set %}`
/// has bound, which is what `minijinja`'s `State::known_variables` returns —
/// measured to agree with Pebble on both, including inside a `{% for %}`,
/// where Pebble's loop scope IS global and its loop variable does show up.
///
/// # `known_variables()` includes the GLOBALS, and subtracting them BY NAME
/// deletes the author's data
///
/// This is the defect the first cut shipped. `known_variables(true)` is the
/// union of the context's names and the environment's, and every
/// `add_function` name is in the second set — so filtering by name removed
/// **forty** names from the output *even when the context bound them*:
///
/// | context | Kestra | egret, before |
/// |---|---|---|
/// | `{"range":1,"a":2}` | `{"a":2,"range":1}` | `{"a":2}` |
/// | `{"secret":1,"a":2}` | `{"a":2,"secret":1}` | `{"a":2}` |
///
/// `{{ range }}` still rendered `1`, so the value was there and only this
/// function hid it — omission of data an author asked for by name. Thirteen
/// corpus entries now cover it, one per name a real flow is likely to hit.
///
/// **What replaces the name filter is an IDENTITY test.** `State::lookup`
/// returns the context's binding when there is one and the environment's
/// global otherwise, and `Value`'s `PartialEq` falls through to
/// `Object::is_same_object` — `Arc` identity — for two objects. So a name
/// belongs to the environment *only* when what `lookup` hands back **is the
/// very global object**; anything else, including a context value that merely
/// shares the name, is the author's. A JSON-derived context value can never be
/// the same `Arc` as a registered function, so this cannot confuse the two.
///
/// # The `_context` scope guard, ported
///
/// `lambda$execute$0` drops a whole **scope** whose key set contains
/// `_context`, and never filters by name — which is how Pebble's own internal
/// scope stays out of the answer. An author's `_context` key lands in the
/// render context's scope and takes that whole scope with it. Measured:
///
/// | template | context | Kestra |
/// |---|---|---|
/// | `{{ fetchContext() }}` | `{"_context":1,"a":2}` | `{}` |
/// | `{% set y = 9 %}{{ fetchContext() }}` | the same | `{"y":9}` |
///
/// so "drop everything" would be wrong in exactly the case the guard is for.
/// The render context's own names arrive through
/// [`super::env::CONTEXT_SCOPE_NAMES`], because `known_variables` carries no
/// provenance and nothing else in `minijinja`'s API does either.
///
/// # Two recorded divergences, neither of them fixable here
///
/// - **Key order.** Kestra collects into a `java.util.HashMap`, so the order
///   is `String.hashCode`'s bucket layout: `{"b":1,"a":2}` comes back
///   `{"a":2,"b":1}` and `{"zebra":…,"apple":…,"mango":…}` comes back in that
///   same insertion order — neither sorted nor insertion, just hashed. This
///   crate's standing ruling is that `HashMap` bucket order is *implementation
///   structure* and is recorded rather than ported (see `oracle/README.md`,
///   "What egret copies from upstream", and `filter-escapechar-map-two-keys`).
///   `known_variables` returns a `HashSet` anyway, so egret must impose *some*
///   order; it sorts by name, which is at least stable and explainable.
///   `function-fetchcontext-two-keys` and `-three-keys` are the entries.
/// - **The `loop` object.** Pebble's has `first/last/length/index/revindex`;
///   minijinja's has nine different fields.
///   `function-fetchcontext-in-a-loop` is baselined for that reason and it is
///   a fact about the loop object, not about this function.
///
/// A null value is **kept**, not dropped: the emptying happens later, in
/// `JsonWriter`, which omits null map entries — so `{"x": null}` renders `{}`
/// on both sides and for the same reason.
fn fetch_context(state: &State) -> Result<MjValue, MjError> {
    let globals: Vec<(String, MjValue)> = state
        .env()
        .globals()
        .map(|(name, value)| (name.to_string(), value))
        .collect();

    // The render context's own top-level names, as its scope.
    let scope: Vec<String> = state
        .lookup(super::env::CONTEXT_SCOPE_NAMES)
        .and_then(|names| {
            names
                .downcast_object_ref::<super::env::ContextScope>()
                .map(|held| held.0.clone())
        })
        .unwrap_or_default();
    // `lambda$execute$0`: a scope whose keys contain `_context` is skipped
    // **whole**, and there are two of them here. egret has the render
    // context's scope and everything a `{% set %}` or a `{% for %}` bound;
    // Pebble has more, but the two groups reproduce both measured cases:
    //
    // | template | context | Kestra |
    // |---|---|---|
    // | `{{ fetchContext() }}` | `{"_context":1,"a":2}` | `{}` |
    // | `{% set y = 9 %}{{ … }}` | the same | `{"y":9}` |
    // | `{% set _context = 1 %}{{ … }}` | `{"a":2}` | `{"a":2}` |
    // | `{% for _context in [1] %}{{ … }}` | `{"a":2}` | `{"a":2}` — the loop variable AND `loop` go |
    //
    // The first cut read only the context scope, so the last two kept a key
    // Kestra drops.
    let in_context = |name: &String| scope.contains(name);
    let drop_the_context_scope = scope.iter().any(|name| name == "_context");
    let drop_the_local_scope = state
        .known_variables()
        .iter()
        .any(|name| name.as_ref() == "_context" && !scope.iter().any(|s| s == "_context"));

    let mut names: Vec<String> = state
        .known_variables()
        .into_iter()
        .map(|name| name.into_owned())
        .collect();
    names.sort();

    let mut entries: Vec<(String, MjValue)> = Vec::with_capacity(names.len());
    for name in names {
        let local = !in_context(&name);
        if (drop_the_context_scope && !local) || (drop_the_local_scope && local) {
            continue;
        }
        let Some(value) = state.lookup(&name) else {
            continue;
        };
        // Identity, not name: see this function's doc. A global the context
        // has NOT shadowed hands back the very object the environment holds,
        // and that one is the engine's rather than the author's. This is also
        // what keeps `CONTEXT_SCOPE_NAMES` itself out of the answer.
        if globals
            .iter()
            .any(|(global, held)| *global == name && *held == value)
        {
            continue;
        }
        entries.push((name, value));
    }

    Ok(MjValue::from_object(OrderedMap::new(entries)))
}

/// `TasksWithStateFunction` — the tasks of the current execution whose state
/// matches, read out of the `tasks` context variable.
///
/// Pure: `tasks` is a context variable, so this is answerable today and W3
/// decides what goes into it.
///
/// The shape, from the bytecode, is a **two-level** walk that most readings of
/// the name get wrong. `tasks` is `Map<taskId, Map<key, value>>`, and for each
/// inner `(key, value)`:
///
/// | `key` | `value` | emitted when it matches |
/// |---|---|---|
/// | `"state"` | not a map | `{state}` — no `value` key |
/// | `"state"` | a map with a `state` entry | `{state, value: "state"}` |
/// | anything else | a map with a `state` entry | `{state, value: key}` |
/// | anything else | not a map | nothing |
///
/// so a task can contribute **more than one** row
/// (`function-taskswithstate-nested-and-flat`).
///
/// Three more measured details:
///
/// - the `state` argument is `((String) …).toUpperCase()`, and `toState` also
///   uppercases the value — so `state='failed'` matches `"FAILED"`, and
///   `state='FAILED'` matches `"failed"`, and the emitted `state` is always
///   the upper-cased one;
/// - `toState` returns `null` for anything that is neither a `String` nor an
///   `Enum`, so a numeric state never matches — not even `state='42'`;
/// - an absent or null `tasks` is `[]`; a `tasks` that is present and **not** a
///   map is a `ClassCastException`.
///
/// # The result map's key order is a constant, not a hash
///
/// The rows come out of a `HashMap` too, so upstream's order is
/// `state, value, taskId` for a three-key row and `state, taskId` for a
/// two-key one — measured, both. That is not `HashMap` order being *ported*:
/// the key set here is fixed at three names, so one emission order reproduces
/// both shapes exactly, and it is written down as that constant rather than as
/// a hash function. `function-fetchcontext-two-keys`, whose key set is the
/// author's, is where the same question has to be recorded instead.
fn tasks_with_state(state: &State, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let Some(wanted) = args.get("state", 0) else {
        return Err(pebble_error(
            "The 'tasksWithState' function expects an argument 'state'.",
        ));
    };
    // `((String) args.get("state")).toUpperCase()`: a null is an NPE on the
    // `toUpperCase` call, a non-String a ClassCastException, and the two say
    // different things.
    if is_java_null(&wanted) {
        return Err(pebble_error(
            "Cannot invoke \"String.toUpperCase()\" because the return value of \
             \"java.util.Map.get(Object)\" is null",
        ));
    }
    let Some(wanted) = wanted.as_str() else {
        return Err(pebble_error(format!(
            "class {} cannot be cast to class java.lang.String",
            java_class_name(&wanted)
        )));
    };
    let wanted = wanted.to_uppercase();

    let mut rows: Vec<MjValue> = Vec::new();
    let Some(tasks) = state.lookup("tasks") else {
        return Ok(MjValue::from(rows));
    };
    if is_java_null(&tasks) {
        return Ok(MjValue::from(rows));
    }
    if tasks.kind() != ValueKind::Map {
        return Err(pebble_error(format!(
            "class {} cannot be cast to class java.util.Map",
            java_class_name(&tasks)
        )));
    }

    for task_id in map_keys(&tasks) {
        let Ok(task) = tasks.get_item(&task_id) else {
            continue;
        };
        if task.kind() != ValueKind::Map {
            continue;
        }
        let task_id = super::env::kestra_to_string(&task_id);
        for key in map_keys(&task) {
            let Ok(value) = task.get_item(&key) else {
                continue;
            };
            let key = super::env::kestra_to_string(&key);
            let nested = (value.kind() == ValueKind::Map)
                .then(|| value.get_item(&MjValue::from("state")).ok())
                .flatten();
            let (found, value_key) = match (&nested, key.as_str()) {
                // `"state"` holding a map: the state comes from the map, and
                // the `value` key is the literal `"state"`.
                (Some(inner), "state") => (to_state(inner), Some("state".to_string())),
                // `"state"` holding a scalar: no `value` key at all.
                (None, "state") => (to_state(&value), None),
                // Any other key holding a map.
                (Some(inner), _) => (to_state(inner), Some(key.clone())),
                (None, _) => (None, None),
            };
            if found.as_deref() != Some(wanted.as_str()) {
                continue;
            }
            let mut row: Vec<(String, MjValue)> =
                vec![("state".to_string(), MjValue::from(found.expect("matched")))];
            if let Some(value_key) = value_key {
                row.push(("value".to_string(), MjValue::from(value_key)));
            }
            row.push(("taskId".to_string(), MjValue::from(task_id.clone())));
            rows.push(MjValue::from_object(OrderedMap::new(row)));
        }
    }
    Ok(MjValue::from(rows))
}

/// `TasksWithStateFunction.toState` — `String` or `Enum` uppercased, `null`
/// for everything else.
///
/// The `null` arm is what makes `tasksWithState(state='42')` answer `[]` for a
/// task whose state is the *number* 42: a `Long` is neither, so it never even
/// reaches the comparison.
fn to_state(value: &MjValue) -> Option<String> {
    (value.kind() == ValueKind::String).then(|| super::env::kestra_to_string(value).to_uppercase())
}

/// `LoopOutputsFunction` — pluck one output name out of every iteration of a
/// loop task.
///
/// Four guards, in this order, each with its own sentence (all four measured
/// through the harness, and all four `error-class` entries):
///
/// 1. `outputs` absent **or** Java null → "expects a non-null argument
///    'outputs'". Unlike `env`'s `containsKey`, this one is a null check, so
///    the two cases really do share a message;
/// 2. `outputs` not a `List` → "expects 'outputs' to be a list";
/// 3. `name` absent or null → "expects a non-null argument 'name'";
/// 4. `name` not a `String` → "expects 'name' to be a string".
///
/// Then, per element: an element that is not a map contributes `null`, a
/// missing or non-map `outputs` key contributes `null`, and an absent `name`
/// inside it contributes `null`. **Nothing is skipped** — the output list is
/// always exactly as long as the input.
fn loop_outputs(rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    let outputs = args.get("outputs", 0).unwrap_or_else(|| MjValue::from(()));
    if is_java_null(&outputs) {
        return Err(pebble_error(
            "The 'loopOutputs' function expects a non-null argument 'outputs'.",
        ));
    }
    let Some(items) = as_seq(&outputs) else {
        return Err(pebble_error(
            "The 'loopOutputs' function expects 'outputs' to be a list.",
        ));
    };
    let name = args.get("name", 1).unwrap_or_else(|| MjValue::from(()));
    if is_java_null(&name) {
        return Err(pebble_error(
            "The 'loopOutputs' function expects a non-null argument 'name'.",
        ));
    }
    let Some(name) = name.as_str() else {
        return Err(pebble_error(
            "The 'loopOutputs' function expects 'name' to be a string.",
        ));
    };

    let name = MjValue::from(name);
    let picked: Vec<MjValue> = items
        .into_iter()
        .map(|item| {
            if item.kind() != ValueKind::Map {
                return MjValue::from(());
            }
            let inner = item.get_item(&MjValue::from("outputs")).ok();
            match inner {
                Some(inner) if inner.kind() == ValueKind::Map => {
                    inner.get_item(&name).unwrap_or_else(|_| MjValue::from(()))
                }
                _ => MjValue::from(()),
            }
        })
        .collect();
    Ok(MjValue::from(picked))
}

// ---------------------------------------------------------------------------
// Shared helpers for the four ports above
// ---------------------------------------------------------------------------

/// The 12 bytes `EncryptionService.generateIv()` produces, and the 12 bytes
/// `decrypt` slices back off the front.
const AES_GCM_IV_BYTES: usize = 12;

/// `java.util.Base64`, which is RFC 4648 with padding and **no** line breaks.
const BASE64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// A Java `(String)` cast that tolerates `null`: `Ok(None)` for Kestra's null,
/// `Err` for anything else that is not a string.
///
/// Java's `checkcast` lets a `null` through, so `(String) null` is `null` and
/// not an exception — which is why `encrypt(plaintext=null)` returns null
/// rather than throwing. Every call site that reads a `String` argument and
/// has a null branch shares this.
fn java_string_arg(value: &MjValue) -> Result<Option<String>, MjError> {
    if is_java_null(value) {
        return Ok(None);
    }
    match value.as_str() {
        Some(text) => Ok(Some(text.to_string())),
        None => Err(pebble_error(format!(
            "class {} cannot be cast to class java.lang.String",
            java_class_name(value)
        ))),
    }
}

/// `new SecretKeySpec(Base64.getDecoder().decode(key), "AES")`.
///
/// Two separate failures, and Kestra says different things for each: a
/// non-base64 key is `IllegalArgumentException: Illegal base64 character …`
/// straight out of the decoder, and a decoded length that is not 16, 24 or 32
/// is `InvalidKeyException: Invalid AES key length: N bytes` out of the JCE.
/// Measured, both.
fn decode_aes_key(key: &MjValue) -> Result<Vec<u8>, MjError> {
    let Some(key) = java_string_arg(key)? else {
        // `Base64.getDecoder().decode((String) null)` is an NPE.
        return Err(pebble_error(
            "Cannot invoke \"String.length()\" because \"src\" is null",
        ));
    };
    let bytes = BASE64
        .decode(key.as_bytes())
        .map_err(|_| pebble_error("Illegal base64 character"))?;
    match bytes.len() {
        16 | 24 | 32 => Ok(bytes),
        other => Err(pebble_error(format!(
            "Invalid AES key length: {other} bytes"
        ))),
    }
}

/// `Cipher.getInstance("AES/GCM/NoPadding")` in ENCRYPT mode, 128-bit tag.
fn aes_gcm_seal(
    key: &[u8],
    iv: &[u8; AES_GCM_IV_BYTES],
    plaintext: &[u8],
) -> Result<Vec<u8>, MjError> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{AesGcm, Nonce};
    let nonce = Nonce::from(*iv);
    let sealed = match key.len() {
        16 => aes_gcm::Aes128Gcm::new_from_slice(key)
            .map_err(|_| pebble_error("Invalid AES key"))?
            .encrypt(&nonce, plaintext),
        24 => {
            AesGcm::<aes_gcm::aes::Aes192, aes_gcm::aes::cipher::consts::U12>::new_from_slice(key)
                .map_err(|_| pebble_error("Invalid AES key"))?
                .encrypt(&nonce, plaintext)
        }
        _ => aes_gcm::Aes256Gcm::new_from_slice(key)
            .map_err(|_| pebble_error("Invalid AES key"))?
            .encrypt(&nonce, plaintext),
    };
    sealed.map_err(|_| pebble_error("Encryption failed"))
}

/// The same in DECRYPT mode. A wrong key, a tampered ciphertext and a
/// truncated tag are all one failure in GCM, and Kestra's own message for it
/// is `Tag mismatch`.
fn aes_gcm_open(
    key: &[u8],
    iv: &[u8; AES_GCM_IV_BYTES],
    ciphertext: &[u8],
) -> Result<Vec<u8>, MjError> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{AesGcm, Nonce};
    let nonce = Nonce::from(*iv);
    let opened = match key.len() {
        16 => aes_gcm::Aes128Gcm::new_from_slice(key)
            .map_err(|_| pebble_error("Invalid AES key"))?
            .decrypt(&nonce, ciphertext),
        24 => {
            AesGcm::<aes_gcm::aes::Aes192, aes_gcm::aes::cipher::consts::U12>::new_from_slice(key)
                .map_err(|_| pebble_error("Invalid AES key"))?
                .decrypt(&nonce, ciphertext)
        }
        _ => aes_gcm::Aes256Gcm::new_from_slice(key)
            .map_err(|_| pebble_error("Invalid AES key"))?
            .decrypt(&nonce, ciphertext),
    };
    opened.map_err(|_| pebble_error("Tag mismatch"))
}

/// The keys of a minijinja map value, in the map's own order.
fn map_keys(value: &MjValue) -> Vec<MjValue> {
    value
        .try_iter()
        .map(|keys| keys.collect())
        .unwrap_or_default()
}

/// A sequence's elements, or `None` when the value is not a `List`.
///
/// **A map is not a list here**, which is `instanceof List` in Java and is not
/// what `try_iter` alone answers: a minijinja map iterates its keys, so
/// `loopOutputs(outputs={…})` would silently walk key names instead of
/// refusing.
fn as_seq(value: &MjValue) -> Option<Vec<MjValue>> {
    match value.kind() {
        ValueKind::Seq | ValueKind::Iterable => Some(value.try_iter().ok()?.collect()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::env::{render_str, RenderContext};

    fn render(template: &str, context: &str) -> Result<String, String> {
        let cx = RenderContext::from_json_str(context).expect("context must parse");
        render_str(template, &cx).map_err(|e| e.to_string())
    }

    fn ok(template: &str) -> String {
        render(template, "{}").expect("must render")
    }

    fn err(template: &str) -> String {
        render(template, "{}").expect_err("must fail")
    }

    // -----------------------------------------------------------------
    // The `compare: "error-class"` entries.
    //
    // The oracle compares these by error CLASS, which any failure
    // satisfies -- including `unknown function`, i.e. the function not
    // being registered at all. These assert egret's own message instead,
    // so the corpus entry is not what carries them. See
    // `oracle/error-class.txt` and `oracle/error-class-pairs.txt`.
    // -----------------------------------------------------------------

    #[test]
    fn range_refuses_an_increment_that_cannot_advance_it() {
        assert!(
            err("{{ range(start=1, end=3, increment=0) }}")
                .contains("The increment of the range function must be different than 0"),
            "{}",
            err("{{ range(start=1, end=3, increment=0) }}")
        );
        let message = err("{{ range(start=1, end=3, increment='x') }}");
        // **The space is Pebble's own constant**, proven by the golden
        // (`must be a number x`, not `numberx`). This assertion used to pin
        // egret's own missing space, which is exactly what an error-class
        // corpus entry cannot see.
        assert!(
            message.contains("The increment of the range function must be a number x"),
            "{message}"
        );
        // The 32-bit truncation Pebble's own character loop performs: an
        // increment whose low 32 bits are zero never moves the loop variable,
        // and Kestra hangs there rather than answering.
        let message = err("{{ range(start='a', end='z', increment=4294967296) }}");
        assert!(message.contains("never advances the range"), "{message}");
    }

    #[test]
    fn range_refuses_an_argument_that_is_neither_a_number_nor_one_character() {
        for template in [
            "{{ range(start='ab', end='e') }}",
            "{{ range(start=1, end='e') }}",
        ] {
            let message = err(template);
            assert!(
                message.contains(
                    "Arguments of range function must be of type Number or String with a \
                     length of 1"
                ),
                "{template}: {message}"
            );
        }
    }

    #[test]
    fn range_carries_kestras_own_million_element_cap() {
        let message = err("{{ range(start=0, end=1000000) }}");
        assert!(
            message.contains(
                "The range function cannot produce more than 1000000 elements, but this call \
                 would produce 1000001."
            ),
            "{message}"
        );
        // The count is `|end - start| / |increment| + 1`, so a two-million-wide
        // span with a two-million step is TWO elements and is allowed. A port
        // that bounded `|end - start|` would refuse it.
        assert_eq!(
            ok("{{ range(start=0, end=2000000, increment=2000000) }}"),
            "[0,2000000]"
        );
    }

    #[test]
    fn from_json_names_the_argument_it_wanted() {
        assert!(
            err("{{ fromJson() }}").contains("The 'fromJson' function expects an argument 'json'."),
            "{}",
            err("{{ fromJson() }}")
        );
        assert!(
            err("{{ fromJson(json=42) }}")
                .contains("The 'fromJson' function expects an argument 'json' with type string."),
            "{}",
            err("{{ fromJson(json=42) }}")
        );
        assert!(
            err("{{ fromJson(json='{a}') }}").contains("Invalid json"),
            "{}",
            err("{{ fromJson(json='{a}') }}")
        );
    }

    #[test]
    fn from_ion_names_the_argument_it_wanted() {
        assert!(
            err("{{ fromIon() }}").contains("The 'fromIon' function expects an argument 'ion'."),
            "{}",
            err("{{ fromIon() }}")
        );
        assert!(
            err("{{ fromIon(ion=42) }}").contains(
                "The 'fromIon' function expects an argument 'ion' with type string or byte[]."
            ),
            "{}",
            err("{{ fromIon(ion=42) }}")
        );
        assert!(
            err("{{ fromIon(ion='{a:') }}").contains("Invalid ion"),
            "{}",
            err("{{ fromIon(ion='{a:') }}")
        );
    }

    /// Upstream defect 6's last row, both halves.
    #[test]
    fn from_ion_refuses_the_text_its_own_writer_produces_for_nothing() {
        for template in [
            "{{ fromIon(ion='null') }}",
            "{{ fromIon(ion=(nothing | toIon)) }}",
        ] {
            let message = render(template, r#"{"nothing": null}"#).expect_err("must fail");
            assert!(
                message.contains("Invalid ion: t is null in sink.next(t)"),
                "{template}: {message}"
            );
        }
        // And the writer really does produce it, which is what makes the two
        // halves one defect rather than two facts.
        assert_eq!(
            render("{{ nothing | toIon }}", r#"{"nothing": null}"#).unwrap(),
            "null"
        );
    }

    /// `fromJson(json=null)` returns null; `yaml(yaml=null)` throws. Two
    /// functions in the same batch, two answers to the same input.
    #[test]
    fn yaml_has_no_null_branch_where_from_json_does() {
        assert_eq!(
            render("<{{ fromJson(json=nothing) }}>", r#"{"nothing": null}"#).unwrap(),
            "<>"
        );
        let message =
            render("{{ yaml(yaml=nothing) }}", r#"{"nothing": null}"#).expect_err("must fail");
        assert!(
            message.contains("The 'yaml' function expects an argument 'yaml' with type string."),
            "{message}"
        );
        assert!(
            err("{{ yaml() }}").contains("The 'yaml' function expects an argument 'yaml'."),
            "{}",
            err("{{ yaml() }}")
        );
        assert!(
            err("{{ yaml(yaml=42) }}")
                .contains("The 'yaml' function expects an argument 'yaml' with type string."),
            "{}",
            err("{{ yaml(yaml=42) }}")
        );
        assert!(
            err("{{ yaml(yaml='a: [') }}").contains("Invalid yaml"),
            "{}",
            err("{{ yaml(yaml='a: [') }}")
        );
    }

    #[test]
    fn yaml_refuses_the_infinity_its_own_filter_writes() {
        // The filter writes it unquoted -- that is upstream's
        // `MINIMIZE_QUOTES` with `ALWAYS_QUOTE_NUMBERS_AS_STRINGS` off.
        assert_eq!(ok("{{ '.inf' | yaml }}"), ".inf\n");
        let message = err("{{ yaml(yaml=('.inf' | yaml)) }}");
        assert!(
            message.contains("Malformed numeric value '.inf'"),
            "{message}"
        );
        assert!(
            err("{{ yaml(yaml='a: .nan') }}").contains("Malformed numeric value '.nan'"),
            "{}",
            err("{{ yaml(yaml='a: .nan') }}")
        );
    }

    #[test]
    fn a_calendar_function_names_itself_when_the_date_is_missing() {
        for (template, name) in [
            ("{{ dayOfWeek() }}", "dayOfWeek"),
            ("{{ dayOfMonth() }}", "dayOfMonth"),
            ("{{ monthOfYear() }}", "monthOfYear"),
            ("{{ isWeekend() }}", "isWeekend"),
            ("{{ hourOfDay() }}", "hourOfDay"),
            ("{{ isLastWorkingDay() }}", "isLastWorkingDay"),
            ("{{ isDayWeekInMonth() }}", "isDayWeekInMonth"),
            ("{{ isPublicHoliday() }}", "isPublicHoliday"),
        ] {
            let message = err(template);
            assert!(
                message.contains(&format!(
                    "The '{name}()' function expects a 'date' argument."
                )),
                "{template}: {message}"
            );
        }
        // A `date` that is present but null takes the same branch: Kestra's
        // guard is `date == null`, not `containsKey`.
        let message =
            render("{{ dayOfWeek(date=nothing) }}", r#"{"nothing": null}"#).expect_err("must fail");
        assert!(
            message.contains("The 'dayOfWeek()' function expects a 'date' argument."),
            "{message}"
        );
    }

    #[test]
    fn a_calendar_function_names_itself_when_the_date_does_not_parse() {
        let message = err("{{ dayOfWeek(date='not-a-date') }}");
        assert!(
            message.contains("The 'dayOfWeek()' function could not parse 'date':"),
            "{message}"
        );
    }

    /// `hourOfDay` is `ZonedDateTime.parse` / `LocalDateTime.parse`, never
    /// `LocalDate.parse`, so it is the only one of the seven that refuses a
    /// bare date.
    #[test]
    fn hour_of_day_refuses_a_date_with_no_time_where_its_siblings_take_one() {
        let message = err("{{ hourOfDay(date='2024-01-06') }}");
        assert!(
            message.contains("The 'hourOfDay()' function could not parse 'date':"),
            "{message}"
        );
        assert_eq!(ok("{{ dayOfWeek(date='2024-01-06') }}"), "SATURDAY");
        assert_eq!(ok("{{ isWeekend(date='2024-01-06') }}"), "true");
    }

    #[test]
    fn is_last_working_day_names_the_day_it_could_not_read() {
        let message = err("{{ isLastWorkingDay(date='2024-01-31', workingDays='FUNDAY') }}");
        assert!(
            message.contains(
                "The 'isLastWorkingDay()' function received an invalid 'workingDays' value: \
                 'FUNDAY'. Expected day names like MONDAY, TUESDAY, etc."
            ),
            "{message}"
        );
    }

    #[test]
    fn is_day_week_in_month_lists_what_it_would_have_accepted() {
        let message =
            err("{{ isDayWeekInMonth(date='2024-01-01', dayOfWeek='MONDAY', position='FIFTH') }}");
        assert!(
            message.contains(
                "received an invalid 'position' value: 'FIFTH'. Expected one of: FIRST, SECOND, \
                 THIRD, FOURTH, LAST."
            ),
            "{message}"
        );
        let message =
            err("{{ isDayWeekInMonth(date='2024-01-01', dayOfWeek='FUNDAY', position='FIRST') }}");
        assert!(
            message.contains(
                "received an invalid 'dayOfWeek' value: 'FUNDAY'. Expected one of: \
                 MONDAY, TUESDAY, WEDNESDAY, THURSDAY, FRIDAY, SATURDAY, SUNDAY."
            ),
            "{message}"
        );
        let message = err("{{ isDayWeekInMonth(date='2024-01-01', dayOfWeek='MONDAY') }}");
        assert!(
            message.contains("The 'isDayWeekInMonth()' function expects a 'position' argument."),
            "{message}"
        );
    }

    /// All three of `randomInt`'s messages spell the function `randomIn()`.
    /// The typo is upstream and it is reproduced, because a flow author
    /// reading an egret error must read the sentence Kestra would have shown.
    #[test]
    fn random_int_reproduces_upstreams_missing_letter() {
        let message = err("{{ randomInt(lower=5, upper=4) }}");
        assert!(
            message.contains("In 'randomIn()' upper is less than lower"),
            "{message}"
        );
        let message = err("{{ randomInt() }}");
        assert!(
            message.contains("The 'randomIn()' function expects an argument lower"),
            "{message}"
        );
        let message = err("{{ randomInt(lower='1', upper=10) }}");
        assert!(
            message.contains("The 'randomIn()' function expects an argument lower of type Long."),
            "{message}"
        );
        // And the typo is the only spelling: nothing here says `randomInt()`.
        assert!(!err("{{ randomInt() }}").contains("randomInt()"));
    }

    #[test]
    fn nano_id_refuses_a_length_or_an_alphabet_it_cannot_use() {
        assert!(
            err("{{ nanoId(alphabet='') }}")
                .contains("The 'nanoId()' function field 'alphabet' must not be empty"),
            "{}",
            err("{{ nanoId(alphabet='') }}")
        );
        assert!(
            err("{{ nanoId(length=0) }}")
                .contains("The 'nanoId()' function field 'length' must be greater than: 0"),
            "{}",
            err("{{ nanoId(length=0) }}")
        );
        assert!(
            err("{{ nanoId(length=1001) }}")
                .contains("The 'nanoId()' function field 'length' must be lower than: 1000"),
            "{}",
            err("{{ nanoId(length=1001) }}")
        );

        // **Both bounds are `>`, not `>=`.** A mutation run found `>= 1000`
        // and `>= 256` surviving, because every case sat one side of the
        // boundary. These sit ON it, and `function-nanoid-length-at-the-max`
        // / `-alphabet-at-the-max` do the same through the oracle.
        assert_eq!(
            ok("{{ nanoId(length=1000, alphabet='x') }}"),
            "x".repeat(1000)
        );
        let widest: String = (0..256u32)
            .map(|c| char::from_u32(c + 0x100).expect("a valid char"))
            .collect();
        let at_max = render(
            "{{ nanoId(length=4, alphabet=a) }}",
            &format!(r#"{{"a": {}}}"#, serde_json::to_string(&widest).unwrap()),
        )
        .expect("a 256-character alphabet is allowed");
        assert_eq!(at_max.chars().count(), 4, "{at_max}");
        let message = render(
            "{{ nanoId(length=4, alphabet=a) }}",
            &format!(
                r#"{{"a": {}}}"#,
                serde_json::to_string(&format!("{widest}x")).unwrap()
            ),
        )
        .expect_err("257 is past the bound");
        // And the count is UTF-16 code units, not characters: 200 astral
        // characters is 400 units and Kestra refuses it. Measured; an earlier
        // cut counted `char`s and accepted it. 128 astral characters is
        // exactly 256 units and is accepted.
        let astral: String = (0..200u32)
            .map(|i| char::from_u32(0x10000 + i).expect("a valid char"))
            .collect();
        let astral_message = render(
            "{{ nanoId(length=4, alphabet=a) }}",
            &format!(r#"{{"a": {}}}"#, serde_json::to_string(&astral).unwrap()),
        )
        .expect_err("400 UTF-16 units is past the bound");
        assert!(
            astral_message.contains("must not contain more than: 256 characters"),
            "{astral_message}"
        );
        let at_max: String = astral.chars().take(128).collect();
        assert_eq!(
            render(
                "{{ nanoId(length=4, alphabet=a) }}",
                &format!(r#"{{"a": {}}}"#, serde_json::to_string(&at_max).unwrap()),
            )
            .expect("256 units is allowed")
            .chars()
            .count(),
            4
        );
        assert!(
            message.contains(
                "The 'nanoId()' function field 'alphabet' must not contain more than: 256 \
                 characters"
            ),
            "{message}"
        );
    }

    /// Ion's own grammar refuses these; the first cut of this reader did not,
    /// and answered `7`, `5`, `$0` and `[1,2]`. Over-acceptance is the one
    /// kind of divergence this crate's rules do not allow — a refusal that
    /// says so is fine, a plausible wrong answer is not.
    ///
    /// **Every source here is the bad token ON ITS OWN, and that is the whole
    /// point of the test above it.** The first cut of these entries and this
    /// test used only whole-document sources, which passed the correct
    /// implementation *and* the one that applied every refusal to the whole
    /// document — see
    /// [`from_ion_reads_one_value_and_never_looks_at_the_tail`], which is the
    /// variant they were all one step away from.
    #[test]
    fn the_ion_reader_refuses_what_ion_itself_forbids() {
        for (source, why) in [
            ("007", "leading zero"),
            ("00", "leading zero"),
            ("-007", "leading zero"),
            ("+5", "leading '+'"),
            ("+1.5", "leading '+'"),
            ("$0", "symbol ID"),
            ("$12", "symbol ID"),
            ("[1,,2]", "unexpected character"),
            ("{,}", "an ion field name"),
            ("$ion_1_01", "unsupported ion version marker"),
            ("$ion_1_1 1", "unsupported ion version marker"),
        ] {
            let message = render(
                "{{ fromIon(ion=i) }}",
                &format!(r#"{{"i": {}}}"#, serde_json::to_string(source).unwrap()),
            )
            .expect_err("must be refused");
            assert!(message.contains(why), "{source}: {message}");
        }
        // And the shapes Ion DOES allow, which the refusals must not catch.
        assert_eq!(
            ok("{{ fromIon(ion='[0x10, 1_0, 0b11, -0]') }}"),
            "[16,10,3,0]"
        );
        assert_eq!(ok("{{ fromIon(ion='0.5') }}"), "0.5");
        assert_eq!(ok("{{ fromIon(ion='+inf') }}"), "Infinity");
        assert_eq!(ok("{{ fromIon(ion='$ion_1_0 1') }}"), "1");
        // Both i64 ends, which an `i64`-only magnitude parse refused.
        assert_eq!(
            ok("{{ fromIon(ion='-9223372036854775808') }}"),
            "-9223372036854775808"
        );
        assert_eq!(
            ok("{{ fromIon(ion='12345678901234567890') }}"),
            "12345678901234567890"
        );
    }

    /// **`FileSerde.readAll(…).findFirst()` is `take(1)`: Kestra never parses
    /// past the first value, and `allRows=true` (`toList()`) parses all of
    /// them.** Measured both ways over fourteen documents.
    ///
    /// This reader parsed the whole document eagerly and then took `rows[0]`,
    /// which was harmless while it accepted nearly everything and became
    /// thirteen regressions the moment fix round 1 gave it six refusals: a
    /// valid value followed by a bad token answered the bad token's error.
    ///
    /// Every entry written for those refusals was a document consisting only
    /// of the bad token, so each passed the correct implementation and the
    /// broken one alike. **The variant they were all one step away from is
    /// simply a valid value in front**, and that is what this asserts.
    #[test]
    fn from_ion_reads_one_value_and_never_looks_at_the_tail() {
        for (source, expected) in [
            ("1 /*c", "1"),
            ("1 +5", "1"),
            ("1 a::1", "1"),
            ("1 null", "1"),
            ("1 007", "1"),
            ("1 $0", "1"),
            ("1 [1,,2]", "1"),
            ("1 2 007", "1"),
            ("1 (", "1"),
            ("1 abc::", "1"),
            ("[1,2] /*c", "[1,2]"),
            ("{a:1} /*c", r#"{"a":1}"#),
        ] {
            assert_eq!(
                render(
                    "{{ fromIon(ion=i) }}",
                    &format!(r#"{{"i": {}}}"#, serde_json::to_string(source).unwrap()),
                )
                .unwrap_or_else(|e| panic!("{source}: {e}")),
                expected,
                "{source}"
            );
        }

        // A bad FIRST value is still an error -- laziness stops at one value,
        // it does not skip one.
        for source in ["null 1", "007 1", "+5 1"] {
            render(
                "{{ fromIon(ion=i) }}",
                &format!(r#"{{"i": {}}}"#, serde_json::to_string(source).unwrap()),
            )
            .expect_err("a bad FIRST value is still an error");
        }

        // `allRows=true` is `toList()`, which DOES read the tail.
        for source in ["1 /*c", "1 +5", "1 007", "1 [1,,2]"] {
            render(
                "{{ fromIon(ion=i, allRows=true) }}",
                &format!(r#"{{"i": {}}}"#, serde_json::to_string(source).unwrap()),
            )
            .expect_err("allRows reads the tail");
        }
        assert_eq!(
            render("{{ fromIon(ion=i, allRows=true) }}", r#"{"i": "1 2 3"}"#).unwrap(),
            "[1,2,3]"
        );
    }

    /// A `/*` with no `*/` mapped to end-of-input, so a **truncated** document
    /// rendered as nothing. Kestra says `Corrupt content to decode`.
    ///
    /// **The distinction the first version of this test named and did not
    /// draw**: it probed only `/* nope`, so it passed the implementation that
    /// applied the refusal to a document whose first value was already
    /// complete. `1 /*c` is `1`, because nothing ever reads that far.
    #[test]
    fn an_unterminated_ion_comment_is_corrupt_not_an_empty_document() {
        let message = render("{{ fromIon(ion=i) }}", r#"{"i": "/* nope"}"#)
            .expect_err("a truncated document must not render as nothing");
        assert!(
            message.contains("unterminated ion block comment"),
            "{message}"
        );
        // The same document with a complete value in front of it is that
        // value, on both engines.
        assert_eq!(
            render("{{ fromIon(ion=i) }}", r#"{"i": "1 /*c"}"#).unwrap(),
            "1"
        );
        assert_eq!(
            render("{{ fromIon(ion=i) }}", r#"{"i": "/* c */ 1 /*c"}"#).unwrap(),
            "1"
        );

        assert_eq!(ok("{{ fromIon(ion='1//c') }}"), "1");
        assert_eq!(ok("{{ fromIon(ion='1 // c') }}"), "1");
        assert_eq!(ok("{{ fromIon(ion='/* c */ 1') }}"), "1");
        assert_eq!(
            render("<{{ fromIon(ion=i) }}>", r#"{"i": "// c"}"#).unwrap(),
            "<>"
        );
        assert_eq!(ok("<{{ fromIon(ion='') }}>"), "<>");
    }

    /// **The second unmeasured claim about annotations in two rounds.** Round
    /// 1 corrected "Jackson drops them" to "Kestra throws for one inside a
    /// container" — and that is false too: `[a::1, 2] | length` is `2`,
    /// `[a::1, 2][1]` is `2`, and `{f:a::1} | className` is
    /// `java.util.LinkedHashMap`. Only *stringifying* a container that holds
    /// one fails, as a Jackson serialiser artifact whose message is not even
    /// stable between JVM runs.
    ///
    /// The lesson is not about annotations: **the correction inherited the
    /// method that produced the error.** Rewriting a claim that was wrong for
    /// lack of measurement is the moment to measure. Fifteen forms were probed
    /// for this one.
    #[test]
    fn an_annotated_value_is_its_own_text_and_its_container_is_a_container() {
        // The value carries its own source text, which is what Kestra renders.
        assert_eq!(ok("{{ fromIon(ion='a::1') }}"), "a::1");
        assert_eq!(ok("{{ fromIon(ion='a::b::1') }}"), "a::b::1");
        assert_eq!(ok("{{ fromIon(ion='a::{b:1}') }}"), "a::{b:1}");
        assert_eq!(ok("{{ fromIon(ion='a::1') | string }}"), "a::1");

        // And the CONTAINER is an ordinary container. These three were the
        // wrong answers the refusal produced.
        assert_eq!(ok("{{ fromIon(ion='[a::1, 2]') | length }}"), "2");
        assert_eq!(ok("{{ fromIon(ion='[a::1, 2]')[1] }}"), "2");
        assert_eq!(
            ok("{{ fromIon(ion='{f:a::1}') | className }}"),
            "java.util.LinkedHashMap"
        );
        assert_eq!(ok("{{ fromIon(ion='{f:a::1}').f }}"), "a::1");
        assert_eq!(ok("{{ fromIon(ion='[[a::1]]')[0][0] }}"), "a::1");
        assert_eq!(
            ok("{{ fromIon(ion='{f:a::1,g:2}') | keys }}"),
            r#"["f","g"]"#
        );

        // **The one thing that cannot be a corpus entry.** Stringifying a
        // container that HOLDS an annotated value throws in Kestra, and the
        // message is not stable between JVM runs -- on the same jar and the
        // same corpus, three fresh runs gave `Document nesting depth (1001)
        // exceeds the maximum` once and `Direct self-reference leading to
        // cycle` twice. A golden that changes on regeneration is worse than
        // none, so this is the pin instead, and `oracle/README.md` says why.
        assert_eq!(ok("{{ fromIon(ion='[a::1, 2]') }}"), r#"["a::1",2]"#);
        assert_eq!(ok("{{ fromIon(ion='{f:a::1}') }}"), r#"{"f":"a::1"}"#);

        // **An annotated LIST is not an annotated STRUCT**, and the first
        // version of this test asserted `a::[1]` renders `a::[1]` — which
        // Kestra does not do. Measured: `a::[1]`, `a::[1,2]` and `a::(1 2)`
        // all throw, `a::[]` and `a::()` are `[]` with the annotation DROPPED,
        // and `a::{b:1}` and `a::{}` keep it. The fifteen-form table had
        // `a::{b:1}` standing in for the whole container case, so it recorded
        // the one member of it that renders.
        assert_eq!(ok("{{ fromIon(ion='a::[1]') }}"), "a::[1]");
        assert_eq!(ok("{{ fromIon(ion='a::[]') }}"), "a::[]");
        assert_eq!(ok("{{ fromIon(ion='a::{}') }}"), "a::{}");

        // A malformed annotation is still an error: the decorated value is
        // parsed, which is what says where the text ends.
        assert!(render("{{ fromIon(ion=i) }}", r#"{"i": "abc::"}"#).is_err());
        // A bare symbol is not an annotation.
        assert_eq!(ok("{{ fromIon(ion='abc') }}"), "abc");
        assert_eq!(ok("{{ fromIon(ion='{a:b}') }}"), r#"{"a":"b"}"#);
    }

    /// **Every non-integer Ion number is an `f64` here, and Kestra splits them
    /// in two**: `e`/`E` is a `java.lang.Double`, and `d`/`D` or a bare `.` is
    /// a `java.math.BigDecimal`.
    ///
    /// Fix round 2 carried a decimal as `filters::BigNumber` to keep the
    /// scale, and that is an `Object`: `value::ops::coerce` has no object arm,
    /// and `Ord for Value` compares `kind()` first with `ValueKind::Plain`
    /// above `ValueKind::Number`. So every `+ - * / %` became an error and
    /// **every `>`/`>=` became unconditionally true**, silently. Reverted
    /// here, with the scale recorded in `oracle/baseline.txt` instead.
    ///
    /// **This is the axis the test could not see before.** It checked render,
    /// `className`, `toIon` and `toJson` — exactly where `BigNumber` is right
    /// — and nothing put an operator on a decimal. Changing what a value *is*
    /// has a blast radius of every operation the type takes part in, not the
    /// operations the entry exercises.
    #[test]
    fn every_non_integer_ion_number_is_an_f64_and_takes_operators() {
        // The operator axis first, because it is the one that was wrong.
        assert_eq!(ok("{{ fromIon(ion='1.5') + 1 }}"), "2.5");
        assert_eq!(ok("{{ fromIon(ion='1.5') - 1 }}"), "0.5");
        assert_eq!(ok("{{ fromIon(ion='1.5') / 2 }}"), "0.75");
        assert_eq!(ok("{{ fromIon(ion='1.5') | abs }}"), "1.5");
        assert_eq!(ok("{{ fromIon(ion='1.5') > 1 }}"), "true");
        assert_eq!(ok("{{ fromIon(ion='1.5') < 1 }}"), "false");
        assert_eq!(ok("{{ fromIon(ion='1.5') == 1.5 }}"), "true");
        // The boundary the object ordering made unconditionally true.
        assert_eq!(ok("{{ fromIon(ion='1.') > 1 }}"), "false");
        assert_eq!(ok("{{ fromIon(ion='1d2') > 1000 }}"), "false");

        for (source, plain, class, ion) in [
            // `d`/`D` and a bare `.`: a `BigDecimal` there, an `f64` here.
            ("1d2", "100.0", "java.lang.Double", "100e0"),
            ("1.50", "1.5", "java.lang.Double", "1.5e0"),
            ("1.", "1.0", "java.lang.Double", "1e0"),
            ("-0.0", "-0.0", "java.lang.Double", "-0e0"),
            // `e`/`E` is a float on BOTH engines, **with or without a dot**.
            // Fix round 2 classified `1.5E2` by the dot rather than by the
            // exponent letter and made it a decimal; its three float entries
            // were all `1E2` and its unit test added `1e0`, so every one of
            // them sat on the same, dotless side of the boundary that was
            // wrong.
            ("1E2", "100.0", "java.lang.Double", "100e0"),
            ("1e0", "1.0", "java.lang.Double", "1e0"),
            ("1.5E2", "150.0", "java.lang.Double", "150e0"),
            ("-2.5e10", "-2.5E10", "java.lang.Double", "-2.5e10"),
            ("1.0E308", "1.0E308", "java.lang.Double", "1.0e308"),
            ("1.5e0", "1.5", "java.lang.Double", "1.5e0"),
        ] {
            let cx = format!(r#"{{"i": {}}}"#, serde_json::to_string(source).unwrap());
            assert_eq!(
                render("{{ fromIon(ion=i) }}", &cx).unwrap(),
                plain,
                "{source}"
            );
            assert_eq!(
                render("{{ fromIon(ion=i) | className }}", &cx).unwrap(),
                class,
                "{source}"
            );
            assert_eq!(
                render("{{ fromIon(ion=i) | toIon }}", &cx).unwrap(),
                ion,
                "{source} through toIon"
            );
        }
        // The `number(type='BIGDECIMAL')` path still returns the object, so
        // arithmetic on it is still refused (recorded in
        // `oracle/baseline.txt`). Its COMPARISONS are exact since W9c Task 4
        // fix round 1 (2026-10-08): every comparison is compiled to a
        // `super::compare` call, so `> 1` is no longer unconditionally true.
        assert!(render("{{ '1.5' | number(type='BIGDECIMAL') + 1 }}", "{}").is_err());
        assert_eq!(ok("{{ '1' | number(type='BIGDECIMAL') > 1 }}"), "false");

        // Three writer facts that path DOES get right, all fixed here: a
        // scale-0 `BigDecimal` needs Ion's `.` or it reads back as an integer;
        // Ion writes the UNSCALED digits with the scale where
        // `BigDecimal.toString()` normalises the mantissa; and
        // `String.valueOf` is `toString()`, not `toPlainString()`.
        assert_eq!(ok("{{ '1' | number(type='BIGDECIMAL') | toIon }}"), "1.");
        assert_eq!(
            ok("{{ '1.50' | number(type='BIGDECIMAL') | toIon }}"),
            "1.50"
        );
        assert_eq!(ok("{{ '1E2' | number(type='BIGDECIMAL') | toIon }}"), "1d2");
        assert_eq!(
            ok("{{ '1.5E2' | number(type='BIGDECIMAL') | toIon }}"),
            "15d1"
        );
        assert_eq!(
            ok("{{ '1E2' | number(type='BIGDECIMAL') | string }}"),
            "1E+2"
        );
    }

    /// `serde_norway` deserialises a source with no content to a **unit**,
    /// which renders as nothing; Jackson throws. A wrong value that renders
    /// successfully is the failure mode this whole package exists to prevent,
    /// so the question is asked of the source, before the parser.
    ///
    /// The shapes that are NOT empty are asserted beside it, because a check
    /// that answered "empty" for `~` would be a new bug of the same kind.
    #[test]
    fn an_empty_yaml_document_is_an_error_not_a_null() {
        for source in ["", "   ", "# c", "\n\n", "# a\n# b\n"] {
            let message = render(
                "{{ yaml(yaml=y) }}",
                &format!(r#"{{"y": {}}}"#, serde_json::to_string(source).unwrap()),
            )
            .expect_err("must be refused");
            assert!(
                message.contains("No content to map due to end-of-input"),
                "{source:?}: {message}"
            );
        }
        assert_eq!(ok("<{{ yaml(yaml='~') }}>"), "<>");
        assert_eq!(ok("{{ yaml(yaml='a: null') }}"), "{}");
        assert_eq!(ok("{{ yaml(yaml='0') }}"), "0");
        // The FIRST of several documents, rather than a refusal.
        assert_eq!(
            render("{{ yaml(yaml=y) }}", r#"{"y": "a: 1\n---\nb: 2"}"#).unwrap(),
            r#"{"a":1}"#
        );
    }

    /// `AbstractDate.formatter` falls through both name maps to
    /// `DateTimeFormatter.ofPattern(name)`, so an unknown format NAME is a
    /// pattern-compile error rather than an unknown-format one.
    #[test]
    fn now_compiles_an_unknown_format_name_as_a_pattern() {
        let message = err("{{ now(format='nope_format') }}");
        assert!(message.contains("Unknown pattern letter: o"), "{message}");
    }

    // -----------------------------------------------------------------
    // What the oracle's SHAPE check cannot see.
    // -----------------------------------------------------------------

    /// `Base62.encode` pads nothing, so `id()` is 21 or 22 characters and
    /// occasionally 20 -- which is why `tests/oracle.rs`'s shape check is not
    /// `== 22`. Ten thousand draws is enough to see the 12.8% case
    /// (P(all 22) is about 10^-596).
    #[test]
    fn an_id_is_base62_and_is_not_always_22_characters() {
        let mut lengths = std::collections::BTreeSet::new();
        for _ in 0..10_000 {
            let value = ok("{{ id() }}");
            assert!(
                value.bytes().all(|b| b.is_ascii_alphanumeric()),
                "{value} is not base62"
            );
            assert!(value.len() <= 22, "{value} is longer than 22");
            lengths.insert(value.len());
        }
        assert!(
            lengths.contains(&21),
            "10 000 draws produced no 21-character id, so the encoder is padding: {lengths:?}"
        );
        assert!(lengths.contains(&22), "{lengths:?}");
    }

    /// `Ksuid.PAD_TO_LENGTH` is 27, and the first four bytes are
    /// `epochSecond - 1_400_000_000`, big-endian.
    ///
    /// **The prefix was pinned by nothing.** The first version of this test
    /// checked the length, the alphabet, and then `base62_bytes` on twenty
    /// ZERO bytes — which exercises the padding helper and never looks at a
    /// real ksuid at all. Replacing the four timestamp bytes with random ones
    /// left every test in the crate and the whole oracle green; verified by
    /// running that mutation. So this decodes the value back and reads the
    /// field: a random `u32` lands inside the window below with probability
    /// about 2 in 10^9.
    #[test]
    fn a_ksuid_is_27_padded_base62_characters_with_a_time_prefix() {
        for _ in 0..200 {
            assert_eq!(ok("{{ ksuid() }}").len(), 27);
        }
        let value = ok("{{ ksuid() }}");
        assert!(value.bytes().all(|b| b.is_ascii_alphanumeric()), "{value}");

        let bytes = base62_decode(&value);
        let stamp = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let expected = (Utc::now().timestamp() - 1_400_000_000) as u32;
        assert!(
            stamp.abs_diff(expected) < 5,
            "the four-byte prefix must be the ksuid epoch, not random: {stamp} vs {expected}"
        );

        // And it is monotonic across a second, which is what "sorts in mint
        // order" means. Compared as strings: base 62 over a big-endian value
        // at a fixed width is order-preserving.
        let earlier = ok("{{ ksuid() }}");
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        let later = ok("{{ ksuid() }}");
        assert!(earlier < later, "{earlier} then {later}");

        // The encoder's own round trip, which is what makes the decode above
        // evidence rather than a second guess.
        assert_eq!(base62_decode(&base62_bytes(&[0u8; 20], 27)), [0u8; 20]);
        let mut probe = [0u8; 20];
        probe[0] = 0x12;
        probe[3] = 0x34;
        probe[19] = 0xff;
        assert_eq!(base62_decode(&base62_bytes(&probe, 27)), probe);
    }

    /// `Base62.encode`'s inverse, for the test above: 27 digits back into the
    /// twenty big-endian bytes they came from, by `buf = buf * 62 + digit`.
    fn base62_decode(text: &str) -> [u8; 20] {
        let mut bytes = [0u8; 20];
        for c in text.chars() {
            let digit = BASE62
                .iter()
                .position(|d| *d as char == c)
                .expect("a base62 digit") as u32;
            let mut carry = digit;
            for byte in bytes.iter_mut().rev() {
                let value = u32::from(*byte) * 62 + carry;
                *byte = (value & 0xff) as u8;
                carry = value >> 8;
            }
            assert_eq!(carry, 0, "a 27-digit ksuid must fit in twenty bytes");
        }
        bytes
    }

    /// A `TimeBasedEpochRandomGenerator` UUID: version nibble `7`, variant
    /// bits `10`, lower-case hex, and 48 bits of Unix milliseconds in front.
    #[test]
    fn a_uuid_is_a_version_7_uuid_with_a_millisecond_prefix() {
        let value = ok("{{ uuid() }}");
        assert_eq!(value.len(), 36, "{value}");
        let parts: Vec<&str> = value.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(
            value.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
            "{value}"
        );
        assert!(
            value.bytes().all(|b| !b.is_ascii_uppercase()),
            "java.util.UUID.toString is lower case: {value}"
        );
        assert!(parts[2].starts_with('7'), "version nibble: {value}");
        assert!(
            matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'),
            "variant bits: {value}"
        );
        // The 48-bit prefix is Unix milliseconds, so it tracks the clock.
        let millis = u64::from_str_radix(&format!("{}{}", parts[0], parts[1]), 16).expect("hex");
        let now = Utc::now().timestamp_millis() as u64;
        assert!(millis.abs_diff(now) < 60_000, "{millis} vs {now}");
    }

    /// The default alphabet is 64 characters read out of `NanoIDFunction`'s
    /// `static {}` block, and it includes `-` and `_` -- which a
    /// `[0-9A-Za-z]` shape check would pass on 21 characters and still be
    /// wrong about.
    #[test]
    fn nano_id_uses_kestras_own_64_character_alphabet_and_both_selection_paths() {
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..2_000 {
            let value = ok("{{ nanoId() }}");
            assert_eq!(value.chars().count(), 21, "{value}");
            seen.extend(value.chars());
        }
        let alphabet: std::collections::BTreeSet<char> =
            "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-_"
                .chars()
                .collect();
        assert_eq!(seen, alphabet, "the alphabet is not Kestra's");

        // A one-character alphabet is deterministic and picks each path:
        // length 1 is a power of two (masked), length 3 is not (rejection
        // sampled). Both must terminate and both must answer the same.
        assert_eq!(ok("{{ nanoId(length=6, alphabet='x') }}"), "xxxxxx");
        assert_eq!(ok("{{ nanoId(length=6, alphabet='xxx') }}"), "xxxxxx");
        // A non-Long length and a non-String alphabet are IGNORED, not
        // refused -- Kestra reads both with `instanceof`.
        assert_eq!(ok("{{ nanoId(length='6', alphabet='x') }}"), "x".repeat(21));

        // **The rejection path has to be the rejection path.** A one-character
        // alphabet cannot tell the two apart -- every answer is `x` either way
        // -- and a mutation run found "always mask" surviving because of it.
        // With a three-character alphabet the mask is `byte & 2`, which yields
        // 0 and 2 and NEVER 1, so the middle letter disappears.
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..200 {
            seen.extend(ok("{{ nanoId(length=21, alphabet='abc') }}").chars());
        }
        assert_eq!(
            seen,
            "abc".chars().collect::<std::collections::BTreeSet<char>>(),
            "a non-power-of-two alphabet must be rejection-sampled, not masked"
        );
    }

    /// The two arguments whose subtraction overflows an `i64`. Kestra's
    /// `upper - lower` is Java `long` arithmetic, which wraps; a plain Rust
    /// `-` panics in debug and wraps silently in release. A panic in the
    /// renderer is a mismatch in every oracle mode, and no corpus entry
    /// reaches this -- it was found by attacking the function.
    #[test]
    fn random_int_does_not_panic_on_a_span_wider_than_an_i64() {
        for template in [
            "{{ randomInt(lower=-9223372036854775808, upper=9223372036854775807) }}",
            "{{ randomInt(lower=-9223372036854775808, upper=0) }}",
            "{{ randomInt(lower=0, upper=9223372036854775807) }}",
        ] {
            let value = ok(template);
            assert!(
                value.parse::<i32>().is_ok(),
                "{template} must answer an int, not panic: {value}"
            );
        }
    }

    /// `floor(random() * (upper - lower)) + lower`: lower-inclusive,
    /// upper-EXCLUSIVE. `tests/oracle.rs`'s shape check states the range and
    /// cannot verify either end; this does.
    #[test]
    fn random_int_is_lower_inclusive_and_upper_exclusive() {
        for _ in 0..1_000 {
            assert_eq!(ok("{{ randomInt(lower=5, upper=6) }}"), "5");
            assert_eq!(ok("{{ randomInt(lower=5, upper=5) }}"), "5");
            assert_eq!(ok("{{ randomInt(lower=-3, upper=-2) }}"), "-3");
        }
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..2_000 {
            seen.insert(ok("{{ randomInt(lower=1, upper=10) }}"));
        }
        assert!(seen.contains("1"), "lower is inclusive: {seen:?}");
        assert!(!seen.contains("10"), "upper is exclusive: {seen:?}");
    }

    /// `new ServerSocket(0)` asks the KERNEL, and the kernel never hands out
    /// a port that is currently bound. That is the whole property, and it is
    /// the one a fake cannot have.
    ///
    /// **The first version of this test could not see it.** It asserted
    /// `1023 < port < 65536` and that two calls differed, which any RNG
    /// satisfies: replacing the whole `TcpListener` block with
    /// `gen_range(49152..=65535)` left every test in the crate green — the
    /// build only warned about an unused import. Verified by running that
    /// mutation.
    ///
    /// So this holds a set of ports open and asserts no draw ever names one.
    /// The arithmetic, rather than a hope: the ephemeral range is 16 384 ports
    /// wide, so a uniform fake names a held port on a `held / 16384` fraction
    /// of draws and the expected number of collisions over `DRAWS` is
    /// `DRAWS * held / 16384`. With 120 held and 4 000 draws that is 29, so a
    /// fake survives with probability about `e^-29` — 2 in 10^13. The real
    /// implementation collides with probability **zero**, not merely small.
    #[test]
    fn random_port_is_a_free_port_the_kernel_chose() {
        // As many as this process's descriptor limit allows, up to 120 —
        // enough for the arithmetic above and well inside a 256-fd default.
        let held: Vec<TcpListener> = (0..120)
            .map_while(|_| TcpListener::bind(("127.0.0.1", 0)).ok())
            .collect();
        let taken: std::collections::BTreeSet<u16> = held
            .iter()
            .filter_map(|l| l.local_addr().ok().map(|a| a.port()))
            .collect();
        assert!(
            taken.len() >= 60,
            "not enough listeners to make this test decisive: {}",
            taken.len()
        );

        // Called directly rather than through `render_str`, which builds a
        // whole environment and a thread per call; the wiring is checked once
        // at the end.
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..4_000 {
            let value = random_port(Rest(Vec::new())).expect("a port");
            let port = u16::try_from(i64::try_from(value).expect("an integer")).expect("a port");
            assert!(port > 1023, "{port}");
            assert!(
                !taken.contains(&port),
                "randomPort() returned {port}, which this test is holding open — the kernel \
                 would never hand that out, so the number did not come from the kernel"
            );
            seen.insert(port);
        }
        assert!(
            seen.len() > 1,
            "every call returned the same port: {seen:?}"
        );
        drop(held);

        let port: u32 = ok("{{ randomPort() }}").parse().expect("a port");
        assert!(port > 1023 && port < 65536, "{port}");
    }

    // -----------------------------------------------------------------
    // W2 Task 8. The oracle CANNOT check any of this.
    //
    // The refusal entries are `compare: "error-class"`, which any failure
    // satisfies -- including `unknown function: secret is unknown`, which is
    // what egret said before this task and which names neither the function
    // nor anything a reader could act on. A green oracle is therefore not
    // evidence a refusal works, and it never will be. These are.
    // -----------------------------------------------------------------

    /// The eight names egret registers only to refuse, each asserted for its
    /// **exact message**: the function's own name, what it needs, and the
    /// package that will implement it.
    ///
    /// Pairs `function-secret`, `-kv`, `-fileuri`, `-http` (which were
    /// `unported:task-8` until W2 Task 8), and `function-render`,
    /// `-renderonce`, `-errorlogs`, `-subflow`. **W9a Task 4** took `read`,
    /// `fileSize`, `fileExists` and `isFileEmpty` out (they answer from a
    /// render's file scope, `file_functions` below) and gave `kv` and `fileURI`
    /// their own packages, W9c and W9b.
    ///
    /// # These refuse unconditionally, and that is the honest answer
    ///
    /// The standing rule is that a paired test asserts a refused input **and**
    /// a neighbouring accepted one, because an entry pins *that* egret refuses
    /// and a test pins *what it says* — neither pins *which inputs*. There is
    /// no boundary here: every one of the eight refuses for every argument
    /// list, including none. So what this asserts instead is the two things
    /// that could actually go wrong:
    ///
    /// - the refusal does not depend on the arguments (three shapes each);
    /// - it is **not** the "unknown function" that a deletion of the
    ///   registration would produce, which is the failure the corpus entry
    ///   cannot tell apart from a real refusal.
    ///
    /// The second of those is belt and braces rather than the discriminating
    /// assertion, and saying so is the honest version: unregistering the
    /// eight dies on the `contains(&want)` assert first, because
    /// `unknown function: secret is unknown` does not contain the sentence.
    /// It earns its place only for a *different* mutation — a refusal that
    /// forwards to minijinja's unknown-name error while still being
    /// registered — and it costs one line.
    ///
    /// **W9c Task 4** took `kv` out: it reads the KV store over the render's
    /// scope now. The test that held seven names is replaced by this one,
    /// which pins six and proves a read through a fake scope.
    #[test]
    fn the_refused_list_is_six_and_kv_is_not_in_it() {
        // W9b Task 4: `fileURI` left the list; W9c Task 4: `kv`.
        for gone in ["fileURI", "kv"] {
            assert!(
                !REFUSED.iter().any(|(name, _)| *name == gone),
                "{gone} is implemented, not refused"
            );
        }
        assert_eq!(REFUSED.len(), 6);
        let expected = [
            ("render", "an execution context to render against", "W3"),
            ("renderOnce", "an execution context to render against", "W3"),
            ("errorLogs", "the running execution's logs", "W11"),
            ("subflow", "subflow execution", "W6"),
        ];
        // `secret` (Confined) and `http` (Unsupported) are checked by their sentences below.
        assert_eq!(
            expected.len() + 2,
            REFUSED.len(),
            "a name was added or dropped"
        );

        let not_yet = expected.iter().map(|(name, needs, package)| {
            (
                *name,
                format!(
                    "The '{name}' function is not implemented yet: it needs {needs}, \
                     which egret's {package} package provides."
                ),
            )
        });
        for (name, want) in not_yet.chain([
            ("secret", SECRET_REFUSAL.to_string()),
            ("http", HTTP_REFUSAL.to_string()),
        ]) {
            // Three argument shapes: none, one named, one positional. A
            // refusal that only fired for one of them would still satisfy the
            // corpus entry.
            for call in [
                format!("{{{{ {name}() }}}}"),
                format!("{{{{ {name}(key='x', path='y', uri='z') }}}}"),
                format!("{{{{ {name}('x') }}}}"),
            ] {
                let message = err(&call);
                assert!(message.contains(&want), "{call}\n  got: {message}");
                // The one thing the corpus entry cannot see.
                assert!(
                    !message.contains("is unknown"),
                    "{call}: this is minijinja saying the name is not registered, \
                     not egret refusing: {message}"
                );
            }
        }

        let message = err("{{ http('https://example.com') }}");
        assert!(
            !message.contains("not implemented yet") && !message.contains("W9"),
            "http names no package that would provide it: {message}"
        );

        // `kv` is a function now: with a scope it answers the stored value,
        // and without one it is refused by its own sentence (the corpus's
        // `function-kv` render has none), never "unknown".
        let cx = kv_fn::FakeKv::default()
            .with("k", crate::storage::kv::value::KvData::String("v".into()))
            .cx();
        assert_eq!(kv_fn::render("{{ kv('k') }}", &cx).unwrap(), "v");
        let message = err("{{ kv(key='my-key') }}");
        assert!(
            message.contains("The 'kv' function needs a running execution"),
            "{message}"
        );
        assert!(!message.contains("is unknown"), "{message}");
    }

    /// `env` — the two refusals, and the accepted neighbour of each.
    ///
    /// Pairs `function-env-missing-name` and `function-env-name-not-a-string`.
    ///
    /// The boundary that matters is **`containsKey` versus a null check**:
    /// an absent `name` throws and a `name` that is explicitly null does not,
    /// which is the same distinction `loopOutputs` gets the other way round.
    #[test]
    fn env_refuses_a_missing_name_and_a_non_string_one() {
        assert!(
            err("{{ env(default='D') }}")
                .contains("The 'env' function expects an argument 'name'."),
            "{}",
            err("{{ env(default='D') }}")
        );
        assert!(
            err("{{ env(name=42, default='D') }}")
                .contains("class java.lang.Integer cannot be cast to class java.lang.String"),
            "{}",
            err("{{ env(name=42, default='D') }}")
        );

        // The accepted neighbours. A null `name` is PRESENT, so it falls all
        // the way through to the default rather than throwing.
        assert_eq!(
            render(
                "{{ env(name=nothing, default='D') }}",
                r#"{"nothing": null}"#
            )
            .unwrap(),
            "D"
        );
        assert_eq!(
            render("{{ env(name='A') }}", r#"{"envs": {"A": "v"}}"#).unwrap(),
            "v"
        );
    }

    /// `encrypt`/`decrypt` — eight refusals, and the key lengths either side
    /// of each boundary.
    ///
    /// Pairs `function-encrypt-missing-argument`, `-key-not-base64`,
    /// `-key-wrong-length`, `-plaintext-not-a-string`,
    /// `function-decrypt-bad-base64`, `-wrong-key`, `-shorter-than-the-iv`
    /// and `-missing-argument`.
    ///
    /// Two boundaries are real and both are asserted from both sides: the AES
    /// key length (16/24/32 accepted, 3 refused) and the IV length (a
    /// ciphertext of 12 bytes or more reaches the cipher, one of 3 does not).
    #[test]
    fn encrypt_and_decrypt_refuse_what_the_jce_refuses() {
        const K: &str = r#"{"k": "vWgkeuVCYm5x3qwIU19oyzGfKFRlzLPlisR3dsVwhvY="}"#;

        for (template, needle) in [
            (
                "{{ encrypt(key=k) }}",
                "The 'encrypt' function expects two arguments 'key' and 'plaintext'.",
            ),
            ("{{ encrypt(key='!!!', plaintext='x') }}", "Illegal base64"),
            (
                "{{ encrypt(key='YWJj', plaintext='x') }}",
                "Invalid AES key length: 3 bytes",
            ),
            (
                "{{ encrypt(key=k, plaintext=42) }}",
                "cannot be cast to class java.lang.String",
            ),
            ("{{ decrypt(key=k, encrypted='!!!') }}", "Illegal base64"),
            ("{{ decrypt(key=k, encrypted='AAAA') }}", "12 > 3"),
            (
                "{{ decrypt(key=k) }}",
                "The 'decrypt' function expects two arguments 'key' and 'encrypted'.",
            ),
        ] {
            let message = render(template, K).expect_err("must be refused");
            assert!(message.contains(needle), "{template}\n  got: {message}");
        }

        // A well-formed 32-byte key that is the WRONG key: GCM's tag check,
        // not an argument error.
        let message = render(
            "{{ decrypt(key='AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=', encrypted=e) }}",
            r#"{"e": "jw802WCJ41neKSYhoaevC8MOepbp12q0JVqoduhDIfo0Gv7nMnr2uA=="}"#,
        )
        .expect_err("must be refused");
        assert!(message.contains("Tag mismatch"), "{message}");

        // The accepted neighbours, on both boundaries. All three AES key
        // sizes round-trip; 3 bytes does not, above.
        for key in [
            "AAAAAAAAAAAAAAAAAAAAAA==",                     // 16 bytes
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",             // 24 bytes
            "vWgkeuVCYm5x3qwIU19oyzGfKFRlzLPlisR3dsVwhvY=", // 32 bytes
        ] {
            let cx = format!(r#"{{"k": "{key}"}}"#);
            assert_eq!(
                render(
                    "{{ decrypt(key=k, encrypted=encrypt(key=k, plaintext='hello')) }}",
                    &cx
                )
                .unwrap_or_else(|e| panic!("{key}: {e}")),
                "hello"
            );
        }

        // The bound egret has and Kestra does not, and the shape that needs
        // it: `encrypt` grows by 4/3 (n + 28) bytes, so NESTING multiplies.
        // Measured on the shipped path before the bound: 30 levels was 672 KB
        // in 6 ms and 48 levels 1 686 ms, with 60 levels extrapolating to
        // ~16 GB. `rewrite`'s nesting cap does not see a call chain and the
        // output cap sees only the last value.
        let mut expr = "'seed'".to_string();
        for _ in 0..60 {
            expr = format!("encrypt(key=k, plaintext={expr})");
        }
        let message =
            render(&format!("{{{{ {expr} }}}}"), K).expect_err("the chain must be refused");
        assert!(message.contains("refuses a plaintext over"), "{message}");
        // The accepted neighbour: a chain that stays inside the ceiling still
        // answers, so this is a ceiling and not a refusal of nesting.
        let mut expr = "'seed'".to_string();
        for _ in 0..8 {
            expr = format!("encrypt(key=k, plaintext={expr})");
        }
        assert!(render(&format!("{{{{ {expr} }}}}"), K).is_ok());

        // And the empty/null short-circuit, which happens BEFORE the key is
        // looked at -- so a key egret would refuse never gets decoded.
        assert_eq!(
            render("{{ encrypt(key='!!!', plaintext='') }}", "{}").unwrap(),
            ""
        );
        assert_eq!(
            render("{{ encrypt(key='!!!', plaintext=n) }}", r#"{"n": null}"#).unwrap(),
            ""
        );
    }

    /// `tasksWithState` — four refusals that say four different things, and
    /// the accepted neighbour of each.
    ///
    /// Pairs `function-taskswithstate-tasks-not-a-map`, `-missing-argument`,
    /// `-argument-not-a-string` and `-argument-null`.
    ///
    /// The `null` argument and the absent one are **different** errors here
    /// (an NPE on `toUpperCase` against a `containsKey` check), which is a
    /// distinction the error-class entries cannot see at all, and it is the
    /// opposite of what `loopOutputs` does with the same two cases.
    #[test]
    fn tasks_with_state_refuses_four_shapes_and_each_says_something_different() {
        let tasks = r#"{"tasks": {"t1": {"state": "FAILED"}}, "nothing": null}"#;
        for (template, needle) in [
            (
                "{{ tasksWithState() }}",
                "The 'tasksWithState' function expects an argument 'state'.",
            ),
            (
                "{{ tasksWithState(state=42) }}",
                "cannot be cast to class java.lang.String",
            ),
            (
                "{{ tasksWithState(state=nothing) }}",
                "String.toUpperCase()",
            ),
        ] {
            let message = render(template, tasks).expect_err("must be refused");
            assert!(message.contains(needle), "{template}\n  got: {message}");
        }
        let message = render("{{ tasksWithState(state='FAILED') }}", r#"{"tasks": "no"}"#)
            .expect_err("must be refused");
        assert!(
            message.contains("cannot be cast to class java.util.Map"),
            "{message}"
        );

        // The accepted neighbours. `tasks` ABSENT and `tasks` NULL are `[]`,
        // not errors -- only a present non-map throws, which is the boundary
        // the entry above cannot pin on its own.
        assert_eq!(ok("{{ tasksWithState(state='FAILED') }}"), "[]");
        assert_eq!(
            render("{{ tasksWithState(state='FAILED') }}", r#"{"tasks": null}"#).unwrap(),
            "[]"
        );
        assert_eq!(
            render("{{ tasksWithState(state='FAILED') }}", tasks).unwrap(),
            r#"[{"state":"FAILED","taskId":"t1"}]"#
        );
    }

    /// `loopOutputs` — six refusals in a fixed order, and the accepted
    /// neighbour of each.
    ///
    /// Pairs `function-loopoutputs-missing-outputs`, `-outputs-null`,
    /// `-outputs-not-a-list`, `-missing-name`, `-name-null` and
    /// `-name-not-a-string`.
    ///
    /// **The order is the assertion.** `outputs` is validated completely
    /// before `name` is looked at, so `loopOutputs()` with neither says
    /// `'outputs'` and never `'name'`; a port that checked both up front would
    /// pass every error-class entry and give a flow author the wrong sentence.
    #[test]
    fn loop_outputs_has_four_guards_in_this_order() {
        let cx = r#"{"o": [{"outputs": {"x": 1}}], "nothing": null}"#;
        for (template, needle) in [
            (
                "{{ loopOutputs(name='x') }}",
                "The 'loopOutputs' function expects a non-null argument 'outputs'.",
            ),
            (
                "{{ loopOutputs(outputs=nothing, name='x') }}",
                "The 'loopOutputs' function expects a non-null argument 'outputs'.",
            ),
            (
                "{{ loopOutputs(outputs=42, name='x') }}",
                "The 'loopOutputs' function expects 'outputs' to be a list.",
            ),
            (
                "{{ loopOutputs(outputs=o) }}",
                "The 'loopOutputs' function expects a non-null argument 'name'.",
            ),
            (
                "{{ loopOutputs(outputs=o, name=nothing) }}",
                "The 'loopOutputs' function expects a non-null argument 'name'.",
            ),
            (
                "{{ loopOutputs(outputs=o, name=42) }}",
                "The 'loopOutputs' function expects 'name' to be a string.",
            ),
        ] {
            let message = render(template, cx).expect_err("must be refused");
            assert!(message.contains(needle), "{template}\n  got: {message}");
        }

        // `outputs` first: with BOTH arguments missing the message is
        // `outputs`, never `name`.
        let message = render("{{ loopOutputs() }}", cx).expect_err("must be refused");
        assert!(message.contains("'outputs'"), "{message}");
        assert!(!message.contains("'name'"), "{message}");

        // A MAP is not a list, which `try_iter` alone would have accepted --
        // it would have walked the key names.
        let message = render(
            "{{ loopOutputs(outputs=m, name='x') }}",
            r#"{"m": {"a": 1}}"#,
        )
        .expect_err("a map is not a list");
        assert!(message.contains("to be a list"), "{message}");

        // The accepted neighbours: an empty list, and one whose elements are
        // every shape the refusals above are not.
        assert_eq!(
            render("{{ loopOutputs(outputs=[], name='x') }}", cx).unwrap(),
            "[]"
        );
        assert_eq!(
            render("{{ loopOutputs(outputs=o, name='x') }}", cx).unwrap(),
            "[1]"
        );
        assert_eq!(
            render(
                "{{ loopOutputs(outputs=o, name='x') }}",
                r#"{"o": [1, {"a": 1}, {"outputs": 5}, {"outputs": {"y": 1}}]}"#
            )
            .unwrap(),
            "[null,null,null,null]"
        );
    }

    /// The seven ports answer, and they answer the way the bytecode says.
    ///
    /// Not a refusal test: these are the values `oracle/corpus.json` pins
    /// through the differential, restated here for the three properties the
    /// differential is weakest on — that `printContext` really is the SAME
    /// class as `fetchContext`, that the result map of `tasksWithState` has
    /// its three keys in the engine's own order, and that a task can
    /// contribute two rows.
    #[test]
    fn the_seven_ports_answer_the_way_the_bytecode_does() {
        assert_eq!(
            render("{{ printContext() }}", r#"{"x": 1}"#).unwrap(),
            render("{{ fetchContext() }}", r#"{"x": 1}"#).unwrap()
        );
        // `state, value, taskId` -- Kestra's HashMap order for this fixed key
        // set, reproduced as a constant rather than as a hash function.
        assert_eq!(
            render(
                "{{ tasksWithState(state='FAILED') }}",
                r#"{"tasks": {"t1": {"child": {"state": "FAILED"}}}}"#
            )
            .unwrap(),
            r#"[{"state":"FAILED","value":"child","taskId":"t1"}]"#
        );
        assert_eq!(
            render(
                "{{ tasksWithState(state='FAILED') }}",
                r#"{"tasks": {"t1": {"state": "FAILED", "child": {"state": "FAILED"}}}}"#
            )
            .unwrap(),
            r#"[{"state":"FAILED","taskId":"t1"},{"state":"FAILED","value":"child","taskId":"t1"}]"#
        );
        // `toState` is String-or-Enum, so a numeric state never matches --
        // not even against its own digits.
        assert_eq!(
            render(
                "{{ tasksWithState(state='42') }}",
                r#"{"tasks": {"t1": {"state": 42}}}"#
            )
            .unwrap(),
            "[]"
        );
    }

    // -----------------------------------------------------------------
    // range, the third minijinja name this crate overrides.
    // -----------------------------------------------------------------

    /// Deleting the `add_function("range", ...)` line does not produce
    /// `unknown function`: minijinja's own `range` global answers instead,
    /// exclusively and positionally. These are the values that separate them.
    #[test]
    fn range_is_kestras_and_not_minijinjas() {
        assert_eq!(ok("{{ range(1, 3) }}"), "[1,2,3]");
        assert_eq!(ok("{{ range(start=1, end=3) }}"), "[1,2,3]");
        assert_eq!(ok("{{ range(start=0, end=10, increment=3) }}"), "[0,3,6,9]");
        assert_eq!(ok("{{ range(start=3, end=1, increment=-1) }}"), "[3,2,1]");
        assert_eq!(ok("{{ range(start=3, end=1) }}"), "[]");
        assert_eq!(
            ok("{{ range(start='a', end='e') }}"),
            r#"["a","b","c","d","e"]"#
        );
        assert_eq!(
            ok("{% for i in range(start=1, end=3) %}{{ i }}{% endfor %}"),
            "123"
        );
    }

    /// Java's `long` arithmetic wraps and its loop then never ends;
    /// `checked_add` and an `i128` count turn both into a refusal or a finite
    /// range. Neither can have a golden: at these sizes Kestra hangs.
    #[test]
    fn range_refuses_the_two_shapes_kestras_own_loop_never_leaves() {
        let message = err("{{ range(start=-9223372036854775808, end=9223372036854775807) }}");
        assert!(
            message.contains("The range function cannot produce more than 1000000 elements"),
            "{message}"
        );
        let message = err("{{ range(start='a', end='z', increment=4294967296) }}");
        assert!(message.contains("never advances the range"), "{message}");
    }

    // -----------------------------------------------------------------
    // The refusals that are egret's own, not Kestra's.
    // -----------------------------------------------------------------

    /// Kestra answers `isPublicHoliday` out of `de.focus_shift.jollyday`
    /// 2.19.0. egret carries no such database, so it refuses **by name**
    /// rather than answering a date a flow branches on -- the same call
    /// `javafmt` makes for `z`, `v` and `B`. `function-ispublicholiday` is
    /// baselined with this reason.
    #[test]
    fn is_public_holiday_refuses_rather_than_guessing() {
        let message = err("{{ isPublicHoliday(date='2024-01-01', countryCode='FR') }}");
        assert!(
            message.contains("egret cannot answer isPublicHoliday()"),
            "{message}"
        );
        assert!(message.contains("jollyday"), "{message}");
        // The argument checks come first, so a broken call reads Kestra's own
        // message rather than the refusal.
        assert!(
            err("{{ isPublicHoliday(date='2024-01-01') }}")
                .contains("The 'isPublicHoliday()' function expects a 'countryCode' argument."),
            "{}",
            err("{{ isPublicHoliday(date='2024-01-01') }}")
        );
    }

    /// Both readers descend recursively over author-controlled text on the
    /// render thread, so both are bounded by the crate's one value-depth
    /// invariant. Verified by REMOVING the bound's effect -- a value one level
    /// inside it must still read.
    #[test]
    fn both_readers_are_bounded_by_the_value_depth_limit() {
        let inside = MAX_VALUE_DEPTH - 1;
        let json = format!("{}{}", "[".repeat(inside), "]".repeat(inside));
        assert!(render(
            "{{ fromJson(json=j) }}",
            &format!(r#"{{"j": {}}}"#, serde_json::to_string(&json).unwrap())
        )
        .is_ok());

        let past = MAX_VALUE_DEPTH + 50;
        let deep = format!("{}{}", "[".repeat(past), "]".repeat(past));
        let message = render(
            "{{ fromJson(json=j) }}",
            &format!(r#"{{"j": {}}}"#, serde_json::to_string(&deep).unwrap()),
        )
        .expect_err("must be refused");
        assert!(message.contains("Invalid json"), "{message}");

        // YAML has no parser-side limit at all -- `serde_norway`'s parser is
        // an event stream -- so `Depth` is the ONLY thing standing here.
        let yaml_deep = format!("{}{}", "[".repeat(past), "]".repeat(past));
        let message = render(
            "{{ yaml(yaml=y) }}",
            &format!(r#"{{"y": {}}}"#, serde_json::to_string(&yaml_deep).unwrap()),
        )
        .expect_err("must be refused");
        assert!(
            message.contains("nests more than 128 levels deep"),
            "{message}"
        );

        // And Ion, whose reader is this crate's own.
        let ion_deep = format!("{}{}", "[".repeat(past), "]".repeat(past));
        let message = render(
            "{{ fromIon(ion=i) }}",
            &format!(r#"{{"i": {}}}"#, serde_json::to_string(&ion_deep).unwrap()),
        )
        .expect_err("must be refused");
        assert!(
            message.contains("nests more than 128 levels deep"),
            "{message}"
        );
    }

    /// `fromJson` decodes `serde_json`'s `arbitrary_precision` sentinel.
    ///
    /// The feature is declared by this crate's `Cargo.toml` because the
    /// shipping binary resolves it on regardless (see
    /// `env::OrderedJsonVisitor::visit_map`), and with it on **every** float
    /// and every past-64-bit integer reaches `BoundedVisitor` as a one-entry
    /// map rather than through `visit_i64`/`visit_u64`/`visit_f64`. Without
    /// the decode, `fromJson('2.5')` answers
    /// `{"$serde_json::private::Number":"2.5"}`.
    ///
    /// Deliberately asserted through the real `fromJson` rather than by
    /// calling `visit_map` directly: the point is what the parser hands us,
    /// and a hand-built `MapAccess` would prove nothing about that.
    #[test]
    fn fromjson_decodes_the_arbitrary_precision_sentinel() {
        // Floats, which are the whole of the sentinel's traffic in practice.
        assert_eq!(ok("{{ fromJson(json='2.5') }}"), "2.5");
        assert_eq!(
            ok("{{ fromJson(json='[1, 2.5, 1.0e100]') }}"),
            "[1,2.5,1.0E100]"
        );
        // An integer past `u64::MAX`: exact, which is the point of the
        // widest-first ladder in `env::integer_from_literal`.
        assert_eq!(
            ok("{{ fromJson(json='18446744073709551616') }}"),
            "18446744073709551616"
        );
        // A map is still a map, and a key that merely LOOKS numeric is not a
        // sentinel: the sentinel is one entry under that exact key.
        assert_eq!(
            ok("{{ fromJson(json='{\"b\": 2.5, \"a\": 1}') }}"),
            "{\"b\":2.5,\"a\":1}"
        );
    }

    /// **`arbitrary_precision` moved this refusal from `serde_json` to us.**
    ///
    /// `oracle/baseline.txt`'s `function-fromjson-huge-exponent` used to be
    /// justified as "`serde_json` rejects an out-of-range exponent in its own
    /// parser". With the feature on it does not — it keeps the literal's text
    /// and hands it over — so the refusal is now
    /// `finite_or_malformed`, reached from the sentinel branch of
    /// `BoundedVisitor::visit_map`. The entry stays baselined
    /// (Kestra answers `Infinity`), but for a different reason, and this is
    /// what fails if that reason changes again.
    #[test]
    fn a_huge_exponent_is_refused_by_this_crate_now_and_not_by_serde_json() {
        let message = err("{{ fromJson(json='1e400') }}");
        assert!(
            message.contains("Malformed numeric value '.inf'"),
            "{message}"
        );
        assert!(
            err("{{ fromJson(json='-1e400') }}").contains("Malformed numeric value '-.inf'"),
            "{}",
            err("{{ fromJson(json='-1e400') }}")
        );
        // And the exponent that IS representable is still a value, so the
        // assertion above is about non-finiteness and not about exponents.
        assert_eq!(ok("{{ fromJson(json='1e300') }}"), "1.0E300");
    }

    /// The round trips F3 asked for, end to end: serialise with the filter
    /// Task 6 ported and read back with the function this task ports.
    #[test]
    fn the_three_round_trips_come_back_unchanged_except_where_upstream_loses_them() {
        let cx = r#"{"v": {"b": 1, "a": [1, 2.5, "x"], "c": {"d": true}}}"#;
        let direct = render("{{ v }}", cx).unwrap();
        assert_eq!(
            render("{{ fromJson(json=(v | toJson)) }}", cx).unwrap(),
            direct
        );
        assert_eq!(
            render("{{ fromIon(ion=(v | toIon)) }}", cx).unwrap(),
            direct
        );
        assert_eq!(render("{{ yaml(yaml=(v | yaml)) }}", cx).unwrap(), direct);

        // The three that change TYPE, all `yaml`, all upstream.
        assert_eq!(
            ok("{{ yaml(yaml=('42' | yaml)) | className }}"),
            "java.lang.Integer"
        );
        assert_eq!(
            ok("{{ yaml(yaml=('1.5' | yaml)) | className }}"),
            "java.lang.Double"
        );
        assert_eq!(ok("{{ yaml(yaml=('1e5' | yaml)) }}"), "100000.0");
    }

    // -----------------------------------------------------------------
    // W9a Task 4: the four file functions, over a fake scope.
    // -----------------------------------------------------------------

    mod file_functions {
        use std::collections::BTreeMap;
        use std::sync::Arc;

        use crate::runners::env::{render_bound, render_str, RenderContext};
        use std::sync::Mutex;

        use crate::runners::files::{
            FileMeta, FileScope, FileUnavailable, NsFileArgs, NsFileMeta, RenderFiles, FILE_SCOPE,
        };

        /// A `RenderFiles` over a map: what each URI resolves to, and its text.
        /// W9b: namespace files by their `path` argument as given, and a spy
        /// of which route each call took.
        #[derive(Default)]
        struct Fake {
            meta: BTreeMap<String, Result<FileMeta, FileUnavailable>>,
            text: BTreeMap<String, String>,
            ns: BTreeMap<String, Result<NsFileMeta, FileUnavailable>>,
            ns_text: BTreeMap<String, String>,
            calls: Arc<Mutex<Vec<String>>>,
        }

        impl Fake {
            fn file(mut self, uri: &str, text: &str) -> Self {
                self.meta.insert(
                    uri.into(),
                    Ok(FileMeta {
                        size: text.len() as u64,
                    }),
                );
                self.text.insert(uri.into(), text.into());
                self
            }
            fn gone(mut self, uri: &str, why: FileUnavailable) -> Self {
                self.meta.insert(uri.into(), Err(why));
                self
            }
            fn cx(self) -> RenderContext {
                RenderContext::new().with_files(FileScope(Arc::new(self)))
            }
            /// A namespace file at `path` (as the template passes it).
            fn ns_file(mut self, path: &str, uri: &str, text: &str) -> Self {
                self.ns.insert(
                    path.into(),
                    Ok(NsFileMeta {
                        path: path.into(),
                        size: text.len() as u64,
                        is_directory: false,
                        uri: uri.into(),
                    }),
                );
                self.ns_text.insert(path.into(), text.into());
                self
            }
            fn ns_dir(mut self, path: &str) -> Self {
                self.ns.insert(
                    path.into(),
                    Ok(NsFileMeta {
                        // The stored form storage answers: no trailing '/'.
                        path: format!("/{}", path.trim_matches('/')),
                        size: 0,
                        is_directory: true,
                        uri: String::new(),
                    }),
                );
                self
            }
            fn ns_gone(mut self, path: &str, why: FileUnavailable) -> Self {
                self.ns.insert(path.into(), Err(why));
                self
            }
            fn spy(&self) -> Arc<Mutex<Vec<String>>> {
                self.calls.clone()
            }
            fn record(&self, call: String) {
                self.calls.lock().unwrap().push(call);
            }
        }

        impl RenderFiles for Fake {
            fn lookup(&self, uri: &str) -> Result<FileMeta, FileUnavailable> {
                self.meta.get(uri).cloned().unwrap_or_else(|| {
                    Err(FileUnavailable::Refused(format!("{uri} not in the fake")))
                })
            }
            fn read_utf8(&self, uri: &str) -> Result<String, String> {
                self.lookup(uri).map_err(|e| e.message().to_string())?;
                Ok(self.text[uri].clone())
            }
            fn read_bytes(&self, uri: &str, _whole: &str) -> Result<Vec<u8>, String> {
                self.read_utf8(uri).map(String::into_bytes)
            }
            fn task_prefix(&self) -> Option<&str> {
                None
            }
            fn namespace_file(&self, args: &NsFileArgs<'_>) -> Result<NsFileMeta, FileUnavailable> {
                self.record(format!(
                    "namespace_file {} ns={:?} rev={:?}",
                    args.path, args.namespace, args.revision
                ));
                self.ns.get(args.path).cloned().unwrap_or_else(|| {
                    Err(FileUnavailable::Refused(format!(
                        "{} not in the fake",
                        args.path
                    )))
                })
            }
            fn read_namespace_file(&self, args: &NsFileArgs<'_>) -> Result<String, String> {
                self.record(format!(
                    "read_namespace_file {} ns={:?} rev={:?}",
                    args.path, args.namespace, args.revision
                ));
                match self.ns.get(args.path) {
                    Some(Ok(_)) => Ok(self.ns_text[args.path].clone()),
                    Some(Err(why)) => Err(why.message().to_string()),
                    None => Err(format!("{} not in the fake", args.path)),
                }
            }
        }

        /// A `RenderFiles` that records which route a call took and
        /// answers every one: the spy of the routing test.
        fn spied() -> (RenderContext, Arc<Mutex<Vec<String>>>) {
            let fake = Fake::default()
                .file(OWN, "own content")
                .ns_file("data/x.csv", "kestra:///w9b/fn/_files/data/x.csv", "x")
                .ns_file(
                    "nsfile:///data/x.csv",
                    "kestra:///w9b/fn/_files/data/x.csv",
                    "x",
                )
                .ns_file(
                    "kestra:///w9b/fn/_files/data/x.csv",
                    "kestra:///w9b/fn/_files/data/x.csv",
                    "x",
                );
            let spy = fake.spy();
            (fake.cx(), spy)
        }

        const OWN: &str = "kestra:///w9a/storage/ff-main/executions/2G1i2hGvCOdQ5cwWChXwwu/tasks/w/6fG4Jp3jvbNakpAsBdWyGO/own.txt";
        const EMPTY: &str = "kestra:///w9a/storage/ff-main/executions/2G1i2hGvCOdQ5cwWChXwwu/tasks/w/6fG4Jp3jvbNakpAsBdWyGO/empty.txt";
        /// `file_functions.missing_in_own`'s URI, written LITERALLY (Kestra's
        /// probe assembled it with `~ execution.id ~`, which egret's
        /// snapshot ruling answers differently -- see `tests/storage_render.rs`).
        const NOPE: &str = "kestra:///w9a/storage/ff-main/executions/2G1i2hGvCOdQ5cwWChXwwu/tasks/w/6fG4Jp3jvbNakpAsBdWyGO/nope.txt";

        const FOUR: [&str; 4] = ["read", "fileSize", "fileExists", "isFileEmpty"];

        fn call(name: &str, uri: &str, cx: &RenderContext) -> Result<String, String> {
            render_str(&format!("{{{{ {name}('{uri}') }}}}"), cx).map_err(|e| e.to_string())
        }

        /// `w9a-storage.json` `file_functions.own` and `file_functions_extra.own_empty`,
        /// each value the artifact's, and the argument passed both ways.
        #[test]
        fn the_four_functions_answer_from_the_scope() {
            let cx = Fake::default()
                .file(OWN, "own content")
                .file(EMPTY, "")
                .cx();
            for (name, own, empty) in [
                ("read", "own content", ""),
                ("fileSize", "11", "0"),
                ("fileExists", "true", "true"),
                ("isFileEmpty", "false", "true"),
            ] {
                assert_eq!(call(name, OWN, &cx).as_deref(), Ok(own), "{name}");
                assert_eq!(call(name, EMPTY, &cx).as_deref(), Ok(empty), "{name} empty");
                assert_eq!(
                    render_str(&format!("{{{{ {name}(path='{OWN}') }}}}"), &cx)
                        .map_err(|e| e.to_string())
                        .as_deref(),
                    Ok(own),
                    "{name}(path=…)"
                );
            }
            // fileSize is a number, not text: it compares.
            assert_eq!(
                render_str(&format!("{{{{ fileSize('{OWN}') > 10 }}}}"), &cx).unwrap(),
                "true"
            );
        }

        /// `file_functions.missing_in_own`: `fileExists` is `false`, the other
        /// three fail naming the URI.
        #[test]
        fn file_exists_on_a_missing_file_is_the_artifacts_answer() {
            let cx = Fake::default()
                .gone(
                    NOPE,
                    FileUnavailable::Missing(format!("{NOPE} does not exist")),
                )
                .cx();
            assert_eq!(call("fileExists", NOPE, &cx).as_deref(), Ok("false"));
            for name in ["read", "fileSize", "isFileEmpty"] {
                let err = call(name, NOPE, &cx).unwrap_err();
                assert!(
                    err.contains(&format!("{NOPE} does not exist")),
                    "{name}: {err}"
                );
                assert!(err.contains(&format!("'{name}'")), "{name}: {err}");
            }
        }

        /// `file_functions.unrelated_same_namespace` (chosen: NotReadable) and
        /// `file_functions.malformed`, `file_functions_extra.malformed_dotdot`
        /// and `.malformed_scheme`: all four FAIL, `fileExists` included.
        #[test]
        fn every_other_unreadable_uri_is_an_error_naming_it() {
            let unrelated = "kestra:///w9a/storage/ff-other/executions/3Hq/tasks/w/x/u.txt";
            let malformed = "kestra:///not-an-execution-file.txt";
            let dotdot = "kestra:///w9a/storage/ff-main/executions/2G1i2hGvCOdQ5cwWChXwwu/tasks/w/../../../../other/x.txt";
            let cx = Fake::default()
                .gone(
                    unrelated,
                    FileUnavailable::Refused(format!(
                        "{unrelated} belongs to an execution this flow may not read"
                    )),
                )
                .gone(
                    malformed,
                    FileUnavailable::Refused(format!(
                        "{malformed} is not a storage URI egret can read: x"
                    )),
                )
                .gone(
                    dotdot,
                    FileUnavailable::Refused(format!(
                        "{dotdot} is not a storage URI egret can read: .."
                    )),
                )
                .cx();
            for uri in [unrelated, malformed, dotdot, "s3://bucket/key.txt"] {
                for name in FOUR {
                    let err = call(name, uri, &cx).unwrap_err();
                    assert!(err.contains(uri), "{name}({uri}): {err}");
                }
            }
        }

        #[test]
        fn without_a_scope_the_four_are_refused_by_name_saying_they_need_an_execution() {
            for name in FOUR.into_iter().chain(["fileURI"]) {
                let err = call(name, OWN, &RenderContext::new()).unwrap_err();
                assert!(
                    err.contains(&format!("The '{name}' function needs a running execution")),
                    "{name}: {err}"
                );
                assert!(!err.contains("is unknown"), "{name}: {err}");
            }
        }

        /// W9b `file_uri.present_rev3`: the URI of the revision storage resolved.
        #[test]
        fn file_uri_returns_the_resolved_revisions_uri() {
            let cx = Fake::default()
                .ns_file(
                    "/fu/three.txt",
                    "kestra:///w9b/fn/_files/fu/three.txt.v3",
                    "r3",
                )
                .cx();
            assert_eq!(
                render_str("{{ fileURI('/fu/three.txt') }}", &cx).unwrap(),
                "kestra:///w9b/fn/_files/fu/three.txt.v3"
            );
        }

        /// §6: a `kestra://` argument comes back as given once it parses --
        /// storage answers its metadata, and the function returns the
        /// ARGUMENT, not the metadata's URI (the fake makes them differ).
        /// An execution URI is returned too, once readable; one storage
        /// refuses is an error naming it.
        #[test]
        fn file_uri_of_a_kestra_uri_returns_it_unchanged_after_it_parses() {
            let ns_uri = "kestra:///w9b/fn/_files/fu/one.txt";
            let bad = "kestra:///w9b/fn/_files/a/../b";
            let cx = Fake::default()
                .ns_file(ns_uri, "kestra:///somewhere/else", "one")
                .ns_gone(
                    bad,
                    FileUnavailable::Refused(format!(
                        "{bad} is not a storage URI egret can read: '..'"
                    )),
                )
                .file(OWN, "own content")
                .cx();
            assert_eq!(
                render_str(&format!("{{{{ fileURI('{ns_uri}') }}}}"), &cx).unwrap(),
                ns_uri
            );
            assert_eq!(
                render_str(&format!("{{{{ fileURI('{OWN}') }}}}"), &cx).unwrap(),
                OWN
            );
            let err = render_str(&format!("{{{{ fileURI('{bad}') }}}}"), &cx)
                .unwrap_err()
                .to_string();
            assert!(err.contains(bad) && err.contains("'fileURI'"), "{err}");
        }

        /// `file_uri._extra.authority_and_namespace_arg`,
        /// `read.nsfile_other_with_namespace_arg` (measured): Kestra's
        /// sentence, for all five, and storage is never asked. The control:
        /// the same namespace argument beside `nsfile:///` (no authority) is
        /// passed on (`read._extra.nsfile_own_with_namespace_arg`).
        #[test]
        fn a_namespace_argument_together_with_an_authority_is_an_error() {
            for name in ["read", "fileSize", "fileExists", "isFileEmpty", "fileURI"] {
                let fake = Fake::default()
                    .ns_file(
                        "nsfile://w9b.other/o.txt",
                        "kestra:///w9b/other/_files/o.txt",
                        "o",
                    )
                    .ns_file("nsfile:///o.txt", "kestra:///w9b/other/_files/o.txt", "o");
                let spy = fake.spy();
                let cx = fake.cx();
                let err = render_str(
                    &format!("{{{{ {name}('nsfile://w9b.other/o.txt', namespace='w9b.other') }}}}"),
                    &cx,
                )
                .unwrap_err()
                .to_string();
                assert!(
                    err.contains(
                        "You cannot set a namespace both as the function argument and inside the URI"
                    ) && err.contains("nsfile://w9b.other/o.txt"),
                    "{name}: {err}"
                );
                assert!(spy.lock().unwrap().is_empty(), "{name}: storage was asked");
                render_str(
                    &format!("{{{{ {name}('nsfile:///o.txt', namespace='w9b.other') }}}}"),
                    &cx,
                )
                .unwrap_or_else(|e| panic!("{name}: {e}"));
                let calls = spy.lock().unwrap();
                assert_eq!(calls.len(), 1, "{name}: {calls:?}");
                assert!(
                    calls[0].contains("ns=Some(\"w9b.other\")"),
                    "{name}: {calls:?}"
                );
            }
        }

        /// The route: a bare path, `nsfile://` and a `kestra:///…/_files/…`
        /// URI reach the namespace methods, with `namespace` and (for read
        /// and fileURI) `revision` handed on; an execution URI reaches W9a's.
        #[test]
        fn a_bare_path_and_nsfile_reach_the_namespace_route_and_an_execution_uri_does_not() {
            for (name, method) in [
                ("read", "read_namespace_file"),
                ("fileSize", "namespace_file"),
                ("fileExists", "namespace_file"),
                ("isFileEmpty", "namespace_file"),
                ("fileURI", "namespace_file"),
            ] {
                for path in [
                    "data/x.csv",
                    "nsfile:///data/x.csv",
                    "kestra:///w9b/fn/_files/data/x.csv",
                ] {
                    let (cx, spy) = spied();
                    render_str(&format!("{{{{ {name}('{path}') }}}}"), &cx)
                        .unwrap_or_else(|e| panic!("{name}({path}): {e}"));
                    assert_eq!(
                        *spy.lock().unwrap(),
                        vec![format!("{method} {path} ns=None rev=None")],
                        "{name}({path})"
                    );
                }
                let (cx, spy) = spied();
                render_str(&format!("{{{{ {name}('{OWN}') }}}}"), &cx)
                    .unwrap_or_else(|e| panic!("{name}(OWN): {e}"));
                assert!(spy.lock().unwrap().is_empty(), "{name}: an execution URI");
            }
            // `namespace` for all five, positional or named; `revision` on
            // read and fileURI only (Kestra's declared names).
            let (cx, spy) = spied();
            render_str("{{ read('data/x.csv', 'w9b.fn', 2) }}", &cx).unwrap();
            render_str("{{ fileURI('data/x.csv', revision='x') }}", &cx).unwrap();
            render_str("{{ fileSize('data/x.csv', namespace='w9b.fn') }}", &cx).unwrap();
            assert_eq!(
                *spy.lock().unwrap(),
                vec![
                    "read_namespace_file data/x.csv ns=Some(\"w9b.fn\") rev=Some(\"2\")"
                        .to_string(),
                    "namespace_file data/x.csv ns=None rev=Some(\"x\")".to_string(),
                    "namespace_file data/x.csv ns=Some(\"w9b.fn\") rev=None".to_string(),
                ]
            );
        }

        /// M1 (follow Kestra): `fileSize`, `fileExists` and `isFileEmpty`
        /// declare `path, namespace` only, and Pebble refuses an undeclared
        /// named argument (`The following named argument does not exist`),
        /// on either route, before storage is asked. `read` and `fileURI`
        /// take it (the control).
        #[test]
        fn the_three_metadata_functions_refuse_a_revision_argument() {
            for name in ["fileSize", "fileExists", "isFileEmpty"] {
                for path in ["data/x.csv", OWN] {
                    let (cx, spy) = spied();
                    let err = render_str(&format!("{{{{ {name}('{path}', revision=2) }}}}"), &cx)
                        .unwrap_err()
                        .to_string();
                    assert!(
                        err.contains("The following named argument does not exist: revision")
                            && err.contains(&format!("'{name}'")),
                        "{name}({path}): {err}"
                    );
                    assert!(spy.lock().unwrap().is_empty(), "{name}: storage was asked");
                }
            }
            let (cx, _) = spied();
            for name in ["read", "fileURI"] {
                render_str(&format!("{{{{ {name}('data/x.csv', revision=2) }}}}"), &cx)
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
            }
        }

        /// `four_functions_ns.*.missing`: `fileExists` is `false`; the other
        /// four fail naming the path.
        #[test]
        fn file_exists_of_a_missing_namespace_file_is_false() {
            let cx = Fake::default()
                .ns_gone(
                    "/fu/nope.txt",
                    FileUnavailable::Missing(
                        "namespace file '/fu/nope.txt' does not exist in namespace 'w9b.fn'".into(),
                    ),
                )
                .cx();
            assert_eq!(
                call("fileExists", "/fu/nope.txt", &cx).as_deref(),
                Ok("false")
            );
            for name in ["read", "fileSize", "isFileEmpty", "fileURI"] {
                let err = call(name, "/fu/nope.txt", &cx).unwrap_err();
                assert!(
                    err.contains("'/fu/nope.txt' does not exist")
                        && err.contains(&format!("'{name}'")),
                    "{name}: {err}"
                );
            }
            // A refusal is not a missing file: fileExists fails too.
            let cx = Fake::default()
                .ns_gone(
                    "nsfile://w9b._files/x",
                    FileUnavailable::Refused(
                        "namespace 'w9b._files' has a segment '_files'".into(),
                    ),
                )
                .cx();
            let err = call("fileExists", "nsfile://w9b._files/x", &cx).unwrap_err();
            assert!(err.contains("w9b._files"), "{err}");
        }

        /// R3: `fileURI` of a directory is an error naming the path; the
        /// other three answer from the directory's row
        /// (`four_functions_ns.*.directory`, `file_uri._extra.directory`,
        /// `uri.directory`).
        #[test]
        fn file_uri_of_a_directory_is_an_error_naming_the_path() {
            let cx = Fake::default().ns_dir("fu").ns_dir("/d/").cx();
            // M7: the message names the stored form storage answered, not the
            // argument as written (`'/d'` for `'/d/'`, `'/fu'` for `'fu'`).
            for (path, named) in [("fu", "/fu"), ("/d/", "/d")] {
                let err = call("fileURI", path, &cx).unwrap_err();
                assert!(
                    err.contains(&format!("'{named}' is a directory"))
                        && err.contains("no file URI"),
                    "{path}: {err}"
                );
            }
            for (name, want) in [
                ("fileSize", "0"),
                ("fileExists", "true"),
                ("isFileEmpty", "true"),
            ] {
                assert_eq!(call(name, "fu", &cx).as_deref(), Ok(want), "{name}");
            }
        }

        /// No argument, and a non-string one.
        #[test]
        fn the_path_argument_is_required_and_a_string() {
            let cx = Fake::default().cx();
            for name in FOUR {
                let err = render_str(&format!("{{{{ {name}() }}}}"), &cx)
                    .unwrap_err()
                    .to_string();
                assert!(
                    err.contains(&format!("The '{name}' function expects an argument 'path'")),
                    "{err}"
                );
                let err = render_str(&format!("{{{{ {name}(42) }}}}"), &cx)
                    .unwrap_err()
                    .to_string();
                assert!(err.contains("with type string"), "{err}");
            }
        }

        /// The probes `the_context_scope_global_is_unreachable_from_a_template`
        /// uses, aimed at [`FILE_SCOPE`] -- and `fetchContext()` does not
        /// list it either, with a scope attached.
        #[test]
        fn the_file_scope_global_is_unreachable_from_a_template() {
            assert!(FILE_SCOPE.contains(' '));
            let cx = Fake::default().file(OWN, "own content").cx();
            let message =
                render_str(&format!("{{{{ {FILE_SCOPE} }}}}"), &cx).expect_err("must not parse");
            assert!(message.to_string().contains("syntax error"), "{message}");
            assert!(render_str("{{ egret }}", &cx).is_err());
            let all = render_str("{{ fetchContext() }}", &cx).unwrap();
            assert!(!all.contains("egret"), "{all}");
            // The control: the scope IS bound on this render.
            assert_eq!(call("fileSize", OWN, &cx).as_deref(), Ok("11"));
        }

        /// `egret.sql` renders `sql:` on the OTHER render thread
        /// (`render_bound`); the scope must reach it too.
        #[test]
        fn the_file_functions_render_inside_an_egret_sql_statement() {
            let cx = Fake::default().file(OWN, "own content").cx();
            let (sql, binds) =
                render_bound(&format!("SELECT {{{{ fileSize('{OWN}') }}}}"), &cx).unwrap();
            assert_eq!(sql, "SELECT ?");
            assert_eq!(binds, vec![serde_json::json!(11)]);
        }
    }

    // -----------------------------------------------------------------
    // W9c Task 4: `kv()`, over a fake scope.
    // -----------------------------------------------------------------

    pub(super) mod kv_fn {
        use std::collections::BTreeMap;
        use std::sync::{Arc, Mutex};

        use crate::runners::env::{render_str, RenderContext};
        use crate::runners::files::{
            FileMeta, FileScope, FileUnavailable, KvAnswer, KvArgs, RenderFiles,
        };
        use crate::storage::kv::value::{decode, KvData, KvNumber, KvType};

        /// A `RenderFiles` answering `kv()` from a map by key, and a spy of
        /// what each call asked. A key not in the map is missing, with the
        /// sentence a real scope writes for namespace `w9c.fn`.
        #[derive(Default)]
        pub(in super::super) struct FakeKv {
            values: BTreeMap<String, Result<KvAnswer, String>>,
            calls: Arc<Mutex<Vec<String>>>,
        }

        impl FakeKv {
            pub(in super::super) fn with(mut self, key: &str, data: KvData) -> Self {
                self.values.insert(key.into(), Ok(KvAnswer::Value(data)));
                self
            }
            fn refusing(mut self, key: &str, why: &str) -> Self {
                self.values.insert(key.into(), Err(why.into()));
                self
            }
            fn spy(&self) -> Arc<Mutex<Vec<String>>> {
                self.calls.clone()
            }
            pub(in super::super) fn cx(self) -> RenderContext {
                RenderContext::new().with_files(FileScope(Arc::new(self)))
            }
        }

        impl RenderFiles for FakeKv {
            fn lookup(&self, uri: &str) -> Result<FileMeta, FileUnavailable> {
                Err(FileUnavailable::Refused(format!("{uri} not in the fake")))
            }
            fn read_utf8(&self, uri: &str) -> Result<String, String> {
                Err(format!("{uri} not in the fake"))
            }
            fn read_bytes(&self, uri: &str, _whole: &str) -> Result<Vec<u8>, String> {
                Err(format!("{uri} not in the fake"))
            }
            fn task_prefix(&self) -> Option<&str> {
                None
            }
            fn kv(&self, args: &KvArgs<'_>) -> Result<KvAnswer, String> {
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("{} ns={:?}", args.key, args.namespace));
                self.values.get(args.key).cloned().unwrap_or_else(|| {
                    Ok(KvAnswer::Missing(format!(
                        "The key '{}' does not exist in the namespace 'w9c.fn'.",
                        args.key
                    )))
                })
            }
        }

        pub(in super::super) fn render(
            template: &str,
            cx: &RenderContext,
        ) -> Result<String, String> {
            render_str(template, cx).map_err(|e| e.to_string())
        }

        fn fixture() -> serde_json::Value {
            serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/oracle-exec/w9c-kv.json"
            )))
            .unwrap()
        }

        fn int(s: &str) -> KvData {
            KvData::Number(KvNumber::Int(s.into()))
        }

        fn dec(s: &str) -> KvData {
            KvData::Number(KvNumber::Decimal(s.into()))
        }

        /// `{a: 1}`, w9c `kv_function._setup`'s `k`.
        fn a_one() -> KvData {
            KvData::Struct(vec![("a".into(), int("1"))])
        }

        /// `KvFunction.java:34-36`: `key`, `namespace`, `errorOnMissing`, in
        /// that order, positionally or by name, or mixed; `namespace`
        /// absent or null asks for the walk (`None`).
        #[test]
        fn kv_takes_key_namespace_and_error_on_missing_positionally_or_by_name() {
            let fake = FakeKv::default().with("k", KvData::String("v".into()));
            let spy = fake.spy();
            let cx = fake.cx();
            for (call, asked) in [
                ("{{ kv('k') }}", "k ns=None"),
                ("{{ kv(key='k') }}", "k ns=None"),
                ("{{ kv('k', null) }}", "k ns=None"),
                ("{{ kv('k', 'w9c.other') }}", "k ns=Some(\"w9c.other\")"),
                (
                    "{{ kv(namespace='w9c.other', key='k') }}",
                    "k ns=Some(\"w9c.other\")",
                ),
                (
                    "{{ kv('k', namespace='w9c.other') }}",
                    "k ns=Some(\"w9c.other\")",
                ),
                (
                    "{{ kv('k', 'w9c.other', true) }}",
                    "k ns=Some(\"w9c.other\")",
                ),
                ("{{ kv(key='k', errorOnMissing=false) }}", "k ns=None"),
            ] {
                spy.lock().unwrap().clear();
                assert_eq!(render(call, &cx).unwrap(), "v", "{call}");
                assert_eq!(*spy.lock().unwrap(), vec![asked.to_string()], "{call}");
            }
            // The third argument, either way, on a missing key.
            for call in [
                "{{ kv('missing', 'w9c.fn', false) }}",
                "{{ kv('missing', errorOnMissing=false) }}",
                "{{ kv(errorOnMissing=false, key='missing') }}",
            ] {
                assert_eq!(render(call, &cx).unwrap(), "", "{call}");
            }
            for call in [
                "{{ kv('missing', 'w9c.fn', true) }}",
                "{{ kv('missing', errorOnMissing=true) }}",
            ] {
                assert!(
                    render(call, &cx)
                        .unwrap_err()
                        .contains("The key 'missing' does not exist in the namespace 'w9c.fn'."),
                    "{call}"
                );
            }
        }

        /// w9c `kv_function.no_key`, `no_key_ns_only` (measured), and Pebble's
        /// own argument checks (`too_many_args`, `unknown_named`, whose
        /// sentences are the artifact's raw messages); `null_key` hands the
        /// scope the empty key, which the key rule refuses with Kestra's
        /// sentence.
        #[test]
        fn kv_without_a_key_is_kestras_sentence() {
            let fake = FakeKv::default().with("k", KvData::String("v".into()));
            let spy = fake.spy();
            let cx = fake.cx();
            for call in ["{{ kv() }}", "{{ kv(namespace='w9c.fn') }}"] {
                let e = render(call, &cx).unwrap_err();
                assert!(
                    e.contains("The 'kv' function expects an argument 'key'."),
                    "{call}: {e}"
                );
            }
            let e = render("{{ kv('k', 'w9c.fn', false, 1) }}", &cx).unwrap_err();
            assert!(
                e.contains(
                    "The argument at position 4 is not allowed. Only 3 argument(s) are allowed."
                ),
                "{e}"
            );
            let e = render("{{ kv(key='k', foo=1) }}", &cx).unwrap_err();
            assert!(
                e.contains("The following named argument does not exist: foo"),
                "{e}"
            );
            assert!(spy.lock().unwrap().is_empty(), "nothing was looked up");
            let _ = render("{{ kv(null) }}", &cx);
            assert_eq!(*spy.lock().unwrap(), vec![" ns=None".to_string()]);
            let e = render("{{ kv(1) }}", &cx).unwrap_err();
            assert!(e.contains("'key'") && e.contains("string"), "{e}");
            let e = render("{{ kv('k', 1) }}", &cx).unwrap_err();
            assert!(e.contains("'namespace'") && e.contains("string"), "{e}");
        }

        /// `KvFunction.java:53`: `errorOnMissing` defaults to TRUE. Missing
        /// is an error carrying the scope's sentence; `false` is `null`
        /// (w9c `kv_function.missing_*`, `field_missing_safe`). The cell
        /// `missing_eom_false_is_null` writes Pebble's `is null`, a test
        /// egret's renderer does not bind (`env.rs`'s fence keeps it
        /// unknown, as it does `is none`); `== null` asks the same of the
        /// value.
        #[test]
        fn kv_error_on_missing_defaults_to_true() {
            let cx = FakeKv::default().cx();
            let e = render("{{ kv('missing') }}", &cx).unwrap_err();
            assert!(
                e.contains("The key 'missing' does not exist in the namespace 'w9c.fn'."),
                "{e}"
            );
            let e = render("{{ kv('missing', errorOnMissing=null) }}", &cx).unwrap_err();
            assert!(e.contains("does not exist"), "null is the default: {e}");
            for (call, want) in [
                ("{{ kv('missing', errorOnMissing=false) }}", ""),
                ("{{ kv('missing', errorOnMissing=false) == null }}", "true"),
                (
                    "{{ kv('missing', errorOnMissing=false) ?? 'dflt' }}",
                    "dflt",
                ),
                ("{{ kv('missing', errorOnMissing=false).a ?? 'n' }}", "n"),
            ] {
                assert_eq!(render(call, &cx).unwrap(), want, "{call}");
            }
            // A refusal is not missing: errorOnMissing=false does not hide it.
            let cx = FakeKv::default().refusing("r", "refused, naming r").cx();
            let e = render("{{ kv('r', errorOnMissing=false) }}", &cx).unwrap_err();
            assert!(e.contains("refused, naming r"), "{e}");
        }

        /// w9c `types.<T>.kv_prints` (and `kv_tojson` where measured): the
        /// artifact's own bytes, decoded as the store decodes them, then
        /// rendered through `kv()`. 39 rows, every type.
        #[test]
        fn kv_returns_each_type_as_kestras_kv_printed_it() {
            let f = fixture();
            let types = f["egret_expected"]["types"].as_object().unwrap();
            let (mut examined, mut tojson) = (0, 0);
            for (name, cell) in types {
                if name.starts_with('_') {
                    continue;
                }
                let kind = KvType::parse(cell["api"]["type"].as_str().unwrap()).unwrap();
                let bytes = cell["ion_bytes"].as_str().unwrap();
                let data = decode(kind, bytes.as_bytes(), 1 << 20).unwrap();
                let cx = FakeKv::default().with("x", data).cx();
                assert_eq!(
                    render("{{ kv('x') }}", &cx).unwrap(),
                    cell["kv_prints"].as_str().unwrap(),
                    "{name}"
                );
                if let Some(want) = cell.get("kv_tojson").and_then(|v| v.as_str()) {
                    assert_eq!(
                        render("{{ kv('x') | toJson }}", &cx).unwrap(),
                        want,
                        "{name} | toJson"
                    );
                    tojson += 1;
                }
                examined += 1;
            }
            assert_eq!(examined, 39, "every types row");
            assert!(tojson > 20, "{tojson} toJson rows");
        }

        /// The user's ruling (2026-10-07): decimals reach the template
        /// exactly. A decimal is a `java.math.BigDecimal` that prints every
        /// digit; an int past `u128` a `java.math.BigInteger`; a float an
        /// `f64`.
        #[test]
        fn kv_renders_a_decimal_and_a_big_int_exactly() {
            let cx = FakeKv::default()
                .with("d", dec("123456789012345678901234567890.5"))
                .with("big", int("123456789012345678901234567890123456789012"))
                .with("f", KvData::Number(KvNumber::Float(1.5)))
                .with("tiny", dec("0.0000001"))
                .with("scaled", dec("1.50"))
                .cx();
            for (call, want) in [
                ("{{ kv('d') }}", "123456789012345678901234567890.5"),
                ("{{ kv('d') | className }}", "java.math.BigDecimal"),
                ("{{ [kv('d')] }}", "[123456789012345678901234567890.5]"),
                ("{{ kv('d') | toJson }}", "123456789012345678901234567890.5"),
                (
                    "{{ kv('big') }}",
                    "123456789012345678901234567890123456789012",
                ),
                ("{{ kv('big') | className }}", "java.math.BigInteger"),
                ("{{ kv('f') }}", "1.5"),
                ("{{ kv('tiny') }}", "0.0000001"),
                // BigDecimal.toString() past the -6 adjusted exponent.
                ("{{ kv('tiny') | toJson }}", "1E-7"),
                ("{{ kv('scaled') }}", "1.50"),
            ] {
                assert_eq!(render(call, &cx).unwrap(), want, "{call}");
            }
        }

        /// Fix round 3, re-review P4 (W9c spec §15): arithmetic on an exact
        /// decimal, or on an int past `i128`, fails the render naming the
        /// operator (minijinja's `InvalidOperation`), and nothing is
        /// rounded; an int within `i128` and a float compute.
        #[test]
        fn arithmetic_on_an_exact_decimal_or_big_int_is_a_named_refusal() {
            let cx = FakeKv::default()
                .with("d", dec("1.5"))
                .with("big", int(&"9".repeat(40)))
                .with("i", int("41"))
                .with("f", KvData::Number(KvNumber::Float(1.5)))
                .cx();
            for (call, op) in [
                ("{{ kv('d') + 1 }}", "+"),
                ("{{ kv('big') * 2 }}", "*"),
                ("{{ 1 - kv('d') }}", "-"),
            ] {
                let e = render(call, &cx).expect_err(call);
                assert!(
                    e.contains(&format!("tried to use {op} operator"))
                        && e.contains("unsupported types"),
                    "{call}: {e}"
                );
            }
            assert_eq!(render("{{ kv('i') + 1 }}", &cx).unwrap(), "42");
            assert_eq!(render("{{ kv('f') + 1 }}", &cx).unwrap(), "2.5");
        }

        /// W9c Task 4 fix round 1, review I1 (the user's ruling, 2026-10-07:
        /// comparisons on exact numbers compare exactly). Every row of the
        /// review's table with its correct answer, then `>=`/`<=`/`!=`
        /// between two exact numbers, an int on the other side, an int past
        /// `u128`, and the float rule (a float is its exact binary value).
        #[test]
        fn comparisons_on_an_exact_decimal_or_big_int_are_exact() {
            let u128_max = u128::MAX.to_string();
            let past_u128 = format!("{}0", u128_max);
            let cx = FakeKv::default()
                .with("d", dec("1.5"))
                .with("d150", dec("1.50"))
                .with("d10", dec("10.5"))
                .with("d9", dec("9.5"))
                .with("m1", dec("-1.5"))
                .with("m2", dec("-2.5"))
                .with("t", dec("0.75"))
                .with("d01", dec("0.1"))
                .with("big", int(&past_u128))
                .with("big2", int(&past_u128))
                .with("bigneg", int(&format!("-{past_u128}")))
                .cx();
            let rows: Vec<(String, &str)> = vec![
                // The review's table, row for row.
                ("{{ kv('d') > 100 }}".into(), "false"),
                ("{{ kv('d') < 100 }}".into(), "true"),
                ("{{ kv('d') >= 2 }}".into(), "false"),
                ("{{ kv('d') <= 2 }}".into(), "true"),
                (
                    "{% if kv('d') > 100 %}BIG{% else %}small{% endif %}".into(),
                    "small",
                ),
                ("{{ kv('d') == 1.5 }}".into(), "true"),
                ("{{ kv('d') != 1.5 }}".into(), "false"),
                ("{{ kv('d') in [1.5] }}".into(), "true"),
                ("{{ kv('d10') > kv('d9') }}".into(), "true"),
                ("{{ kv('m1') > kv('m2') }}".into(), "true"),
                ("{{ kv('d') == kv('d150') }}".into(), "true"),
                // The review's failure scenario: a threshold.
                ("{{ kv('t') > 0.9 }}".into(), "false"),
                ("{{ kv('t') == 0.75 }}".into(), "true"),
                // Between two exact numbers, every operator.
                ("{{ kv('d') >= kv('d150') }}".into(), "true"),
                ("{{ kv('d') <= kv('d150') }}".into(), "true"),
                ("{{ kv('d') != kv('d150') }}".into(), "false"),
                ("{{ kv('d9') >= kv('d10') }}".into(), "false"),
                ("{{ kv('m2') <= kv('m1') }}".into(), "true"),
                // (Two chains were here. Since package #5a Task A2 a chain is
                // Pebble's left-associative one, which fails; it is asserted
                // below, and the pairwise comparisons it meant are here.)
                (
                    "{{ kv('m2') < kv('m1') and kv('m1') < kv('d') and kv('d') < kv('d10') }}"
                        .into(),
                    "true",
                ),
                (
                    "{{ kv('m1') > kv('m2') and kv('m2') > kv('d') }}".into(),
                    "false",
                ),
                // An int on the other side, either side.
                ("{{ kv('d') > 1 }}".into(), "true"),
                ("{{ 2 > kv('d') }}".into(), "true"),
                ("{{ kv('m1') < -1 }}".into(), "true"),
                ("{{ kv('m1') > -2 }}".into(), "true"),
                ("{{ 1 in [kv('d'), 1] }}".into(), "true"),
                ("{{ kv('d150') in [1, 1.5] }}".into(), "true"),
                ("{{ kv('d') not in [1, 2] }}".into(), "true"),
                // An int past `u128`, which no Rust integer holds.
                (format!("{{{{ kv('big') > {u128_max} }}}}"), "true"),
                ("{{ kv('big') > 1e30 }}".into(), "true"),
                ("{{ kv('big') < 1e300 }}".into(), "true"),
                ("{{ kv('big') == kv('big2') }}".into(), "true"),
                ("{{ kv('bigneg') < -1 }}".into(), "true"),
                ("{{ kv('bigneg') < kv('big') }}".into(), "true"),
                ("{{ kv('big') in [kv('big2')] }}".into(), "true"),
                // A float is its exact binary value (spec §15): the decimal
                // 0.1 is below the double nearest 0.1.
                ("{{ kv('d01') == 0.1 }}".into(), "false"),
                ("{{ kv('d01') < 0.1 }}".into(), "true"),
                ("{{ kv('d') == 1.5 }}".into(), "true"),
                // `sort` orders them by value.
                (
                    "{{ [kv('d10'), kv('m1'), kv('d150'), kv('m2'), kv('d9')] | sort }}".into(),
                    "[-2.5,-1.5,1.50,9.5,10.5]",
                ),
                // `isIn` is Kestra's `InExpression`, which compares
                // `toString()`s: `1.5` matches, `1.50` does not.
                ("{{ kv('d') isIn [1.5] }}".into(), "true"),
                ("{{ kv('d150') isIn [1.5] }}".into(), "false"),
            ];
            for (template, want) in &rows {
                assert_eq!(render(template, &cx).unwrap(), *want, "{template}");
            }
            // A chain compares a boolean on its outer step, which Kestra
            // refuses (spec D1).
            let e = render("{{ kv('m2') < kv('m1') < kv('d') }}", &cx).unwrap_err();
            assert!(e.contains("Could not perform less than comparison"), "{e}");
            // Arithmetic stays a named refusal.
            let e = render("{{ kv('d') + 1 }}", &cx).unwrap_err();
            assert!(e.contains("tried to use + operator"), "{e}");
        }

        /// w9c `kv_function.eom_*` (chosen: Kestra throws a raw
        /// `ClassCastException`): a non-boolean `errorOnMissing` is refused
        /// naming the argument and its type, present key or missing, before
        /// anything is looked up.
        #[test]
        fn a_non_boolean_error_on_missing_is_refused_as_measured() {
            let f = fixture();
            let cells = f["egret_expected"]["kv_function"].as_object().unwrap();
            let fake = FakeKv::default().with("k", a_one());
            let spy = fake.spy();
            let cx = fake.cx();
            let mut examined = 0;
            for (name, cell) in cells {
                if !name.starts_with("eom_") {
                    continue;
                }
                let e = render(cell["expr"].as_str().unwrap(), &cx).unwrap_err();
                for fragment in cell["message_contains"].as_array().unwrap() {
                    assert!(e.contains(fragment.as_str().unwrap()), "{name}: {e}");
                }
                examined += 1;
            }
            assert_eq!(examined, 4);
            assert!(spy.lock().unwrap().is_empty(), "refused before the lookup");
            // The control: a boolean passes.
            assert_eq!(render("{{ kv('k', 'w9c.fn', true).a }}", &cx).unwrap(), "1");
        }

        /// w9c `kv_function.no_execution` (spec §6): a render with no scope
        /// (a Flow trigger's conditions, the corpus) refuses `kv()` by name
        /// -- the pairing of the expression oracle's `function-kv`.
        #[test]
        fn kv_without_a_scope_refuses_by_name() {
            for call in ["{{ kv('k') }}", "{{ kv(key='my-key') }}"] {
                let e = render(call, &RenderContext::new()).unwrap_err();
                assert!(
                    e.contains(
                        "The 'kv' function needs a running execution: it reads the KV store as \
                         the running flow"
                    ),
                    "{call}: {e}"
                );
                assert!(!e.contains("is unknown"), "{e}");
            }
        }

        /// Task 3 re-review 2, H1, at the render: a value nested 10,000
        /// deep (built by hand, with a loop, never `serde_json::json!`) is
        /// refused naming the limit, with no stack overflow; one at
        /// `MAX_DEPTH` renders.
        #[test]
        fn a_value_nested_10000_deep_is_refused_at_render_with_no_abort() {
            let nest = |depth: usize| {
                let mut v = KvData::Null;
                for _ in 0..depth {
                    v = KvData::List(vec![v]);
                }
                v
            };
            let limit = crate::storage::ion::MAX_DEPTH;
            let cx = FakeKv::default()
                .with("deep", nest(10_000))
                .with("edge", nest(limit))
                .with("over", nest(limit + 1))
                .cx();
            for key in ["deep", "over"] {
                let e = render(&format!("{{{{ kv('{key}') }}}}"), &cx).unwrap_err();
                assert!(
                    e.contains(&format!("deeper than {limit} levels")),
                    "{key}: {e}"
                );
            }
            let edge = render("{{ kv('edge') }}", &cx).unwrap();
            assert_eq!(
                edge,
                format!("{}null{}", "[".repeat(limit), "]".repeat(limit))
            );
        }
    }
}
