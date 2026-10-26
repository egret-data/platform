//! W5a Task 5: the handler that lived here is now `handlers::script::Script`,
//! serving `io.kestra.plugin.scripts.python.Script` and this module's
//! `egret.script.python` as its alias. This module keeps what is Python's own:
//! [`DEFAULT_IMAGE`], [`SCRIPT_FILE`], [`ENV_DEFAULTS`] and the
//! [`NameHintSink`] authoring hint. The history below is W4's and still holds.
//!
//! `egret.script.python` — run an inline Python script in a container.
//!
//! # This handler was called `egret.python` for one commit, and that name was
//! # already taken
//!
//! Fix round 1's Critical 2. `crate::spec::registry::TASK_TYPES` has held an
//! `egret.python` since W1 (commit `228173f9`) declaring a **required**
//! `RefField { field: "notebook", kind: RefKind::Notebook }`, and the programme
//! spec's §6.4 table says what it is for: *"`egret.python` | `KernelLauncher`;
//! cần port `egret-kernel-api` mới"* — **run a notebook through the kernel
//! launcher**, needing a port that does not exist yet. `src/models/flow.rs:417`
//! and `spec/validate.rs`'s fixtures both write `type: egret.python` with a
//! `notebook:` field, and `tests/common/mod.rs:199` has a `FakeRefs` arm for it.
//!
//! A container that runs an inline `script:` is a different task. The collision
//! was live in both directions and neither was a compile error:
//!
//! * a `script:` flow **fails publish**, because publish resolves the declared
//!   `notebook` reference and there is none;
//! * a `notebook:` flow that somehow published would **fail at run time**, on
//!   this module's `deny_unknown_fields`.
//!
//! **The existing entry is not bent to fit.** It is a shipped, spec-declared
//! meaning with fixtures behind it; this handler is the new thing, so this
//! handler is what renamed. `egret.script.python` follows the vocabulary that is
//! already there (`egret.dataapi.call`, `egret.dataset.refresh`,
//! `egret.report.send` all namespace before the verb) and reads as what it is —
//! a script task, in Python. Nothing outside `crates/egret-workflow` referenced
//! either name; measured with a repo-wide grep over `*.rs`, `*.ts`, `*.tsx`,
//! `*.md`, `*.yaml`, `*.json`.
//!
//! `TASK_TYPES` gains a `egret.script.python` row with `references: &[]` in the
//! same change, because publish refuses a type absent from that table and a
//! handler nothing can publish to is dead surface.
//! `handlers::container::tests::the_publish_table_and_these_handlers_agree`
//! pins all of it — including that this handler's id is **not** `egret.python`,
//! so the collision cannot come back quietly.
//!
//! # No oracle coverage either, and for the same measured reason
//!
//! Zero of the 351 corpus flows declare a script task type
//! ([`handlers::container`](super::container) carries the measurement). This
//! file's instruments are its own unit tests over a fake
//! [`TaskRunner`](crate::worker::commands::TaskRunner) and
//! `tests/container_docker_live.rs` against a real daemon.
//!
//! # What it corresponds to, measured on the pinned image
//!
//! `io.kestra.plugin.scripts.python.Script`, from
//! `/app/plugins/io_kestra_plugin__plugin-script-python__1_9_8.jar`. What that
//! class does, read with `javap -p -c -constants`:
//!
//! * writes the author's `script:` to a file whose suffix is the literal
//!   `".py"`, then runs it;
//! * the interpreter binary is the literal `"python"`
//!   (`PythonEnvironmentManager`);
//! * `AbstractPythonExecScript`'s container image default is the literal
//!   `"python:3.13-slim"`;
//! * it sets `PYTHONUNBUFFERED=true`, `PIP_ROOT_USER_ACTION=ignore` and
//!   `PIP_DISABLE_PIP_VERSION_CHECK=1` (and a `PYTHONPATH`, which depends on a
//!   virtualenv this handler does not build).
//!
//! [`ENV_DEFAULTS`] is the third of those, verbatim, and [`DEFAULT_IMAGE`] and
//! [`SCRIPT_FILE`] the others.
//!
//! # Two measured behaviours this handler deliberately does NOT copy
//!
//! * **The `true = True` preamble.** `Script.class` prepends the literal
//!   `"true = True\nfalse = False\nnull = None\n"` to the script, so that a
//!   `{{ … }}` rendering a JSON boolean into Python source still parses. egret
//!   does not, because it shifts every line number in every traceback by three
//!   and the author never wrote those lines — a `NameError` on line 1 reported
//!   as line 4 is a debugging cost paid on every run to rescue a template that
//!   should have used `{{ … | lower }}`. The divergence is stated here rather
//!   than discovered from a stack trace.
//! * **The dependency resolver.** Plugin 1.9.8 carries a whole
//!   `PythonEnvironmentManager` that installs `uv` (it holds a
//!   `uvInstallerVersion` and a `uvInstallerSha256`) and builds a virtualenv
//!   per run. egret runs the image's own `python`; an author who needs packages
//!   names an image that has them, or installs them in `beforeCommands`. Porting
//!   the resolver means downloading and checksumming a binary inside a task,
//!   which is a supply-chain decision and not a handler's.

use std::sync::Mutex;

use super::super::commands::LineObserver;
use crate::logs::LogLevel;

/// The `type:` a flow writes.
///
/// **Not `egret.python`** — that name is the notebook-kernel task, declared in
/// `spec::registry::TASK_TYPES` since W1. See this module's doc.
pub const TYPE_ID: &str = crate::scripts::ALIAS_SCRIPT_PYTHON;

/// `AbstractPythonExecScript`'s own container image default, verbatim --
/// the same value as `scripts::default_image` for the `python.*` types.
pub const DEFAULT_IMAGE: &str = "python:3.13-slim";

/// Where the author's `script:` is written, relative to
/// [`WORKING_DIR`](crate::worker::commands::WORKING_DIR).
///
/// Kestra generates a random name with the `".py"` suffix; egret fixes the
/// name, because a random one would appear in every traceback the author reads
/// and in none of the two runs the same. The suffix is what makes `python` treat
/// it as a module for the purposes of an import beside it.
pub const SCRIPT_FILE: &str = "main.py";

/// The environment `Script.class` sets, verbatim and in its own spellings.
///
/// `PYTHONUNBUFFERED` is the load-bearing one: without it CPython buffers
/// stdout when it is a pipe, which is exactly what
/// [`DockerRunner`](crate::worker::commands::DockerRunner) gives it, and every
/// `print()` — including a `::{"outputs": …}::` marker — would arrive in one
/// burst at exit instead of streaming.
pub const ENV_DEFAULTS: [(&str, &str); 3] = [
    ("PYTHONUNBUFFERED", "true"),
    ("PIP_ROOT_USER_ACTION", "ignore"),
    ("PIP_DISABLE_PIP_VERSION_CHECK", "1"),
];

/// The Python names Kestra's `Script` predefines and egret deliberately does
/// not.
///
/// `Script.class` prepends the literal `"true = True\nfalse = False\nnull = None\n"`
/// so a `{{ … }}` rendering a JSON boolean into Python source still parses.
/// [This module's doc](self) says why egret does not copy it: three invisible
/// lines shift every traceback the author reads.
///
/// **Review Minor 7 is that the divergence lived only in rustdoc**, so an author
/// porting a Kestra script got a `NameError` with nothing to connect it to. The
/// repo has no author-facing workflow task reference to put it in — measured, no
/// document in `docs/` mentions `outputFiles` or `beforeCommands` at all — so it
/// is connected at the place the author actually meets it instead:
/// [`NameHintSink`] watches the script's own stderr for exactly these names and
/// the script handler (`handlers::script`) appends the explanation to the failure message.
const KESTRA_PREDEFINED: [&str; 3] = ["true", "false", "null"];

/// A [`LineObserver`] that remembers whether Python complained about one of
/// [`KESTRA_PREDEFINED`].
///
/// W11 made it an observer rather than a sink decorator: the runner shows it
/// each line and then writes the line to `task.log` itself, so the hint can
/// neither lose nor reorder a line. It is still not a field on
/// `CommandsResult`, because this is one handler's authoring hint and not part
/// of what a command produces — the runner has no business knowing that `null`
/// means something to Python.
#[derive(Debug, Default)]
pub(super) struct NameHintSink {
    seen: Mutex<Option<String>>,
}

impl NameHintSink {
    /// The name CPython said was undefined, if it was one of Kestra's three.
    ///
    /// Matched on CPython's own wording — `NameError: name 'true' is not
    /// defined` — rather than on the bare word, so an ordinary log line
    /// mentioning `null` cannot produce the hint.
    fn undefined_name(line: &str) -> Option<&'static str> {
        let rest = line.split("NameError: name '").nth(1)?;
        let name = rest.split('\'').next()?;
        KESTRA_PREDEFINED.iter().copied().find(|k| *k == name)
    }

    pub(super) fn hint(&self) -> Option<String> {
        self.seen.lock().unwrap().clone()
    }
}

impl LineObserver for NameHintSink {
    fn observe(&self, _level: LogLevel, message: &str) {
        if let Some(name) = Self::undefined_name(message) {
            let mut seen = self.seen.lock().unwrap();
            if seen.is_none() {
                *seen = Some(format!(
                    "`{name}` is not a Python name. Kestra's `python.Script` \
                     silently prepends `true = True`, `false = False` and \
                     `null = None` to every script; egret deliberately does \
                     not, because those three invisible lines shift every line \
                     number in every traceback. If a `{{{{ … }}}}` rendered a JSON \
                     boolean or null into your source, render it as Python \
                     (`{{{{ … | lower }}}}` is not enough for `null`) or define \
                     the names yourself"
                ));
            }
        }
    }
}
