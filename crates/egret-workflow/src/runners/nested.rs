// kestra: core/src/main/java/io/kestra/core/runners/pebble/functions/RenderFunction.java
// kestra: core/src/main/java/io/kestra/core/runners/pebble/functions/RenderOnceFunction.java
// kestra: core/src/main/java/io/kestra/core/runners/VariableRenderer.java

//! Package #5b, Task B6 (spec D9, user decision Q4): Kestra's `render()` and
//! `renderOnce()`, a render **nested** inside the one that calls it.
//!
//! # What Kestra does (measured, 2026-10-29)
//!
//! `RenderFunction.execute` (read in the checkout and in the jar with
//! `javap -c -p`; the harness now wires both functions, see
//! `oracle/README.md` "What package #5b's Task B6 measured"):
//!
//! 1. reads its depth from the variable `__kestra_render_depth` (absent = 0)
//!    and fails at `depth >= maxRenderDepth` (16) with `Maximum render()
//!    nesting depth (16) exceeded at line <n> — check for circular render()
//!    calls in your template.` So the top-level `render()` runs at depth 0 and
//!    16 nested calls succeed; the 17th fails (`function-render-depth-16`,
//!    `-17`);
//! 2. refuses a missing `toRender` and a `recursive` that is not a boolean (a
//!    null `recursive` is `true`);
//! 3. collects every variable of the caller's scopes -- the context, and also
//!    what a `{% set %}` or a `{% for %}` bound (`function-render-sees-a-set-
//!    variable`, `-loop-variable`) -- puts the depth plus one under
//!    `__kestra_render_depth`, and calls `VariableRenderer.renderObject`:
//!    - a **string** renders as a whole template (the no-`{` short circuit,
//!      the raw-region protection, `renderRecursively` to a fixpoint of at
//!      most 101 passes, or one pass when `recursive` is false, then the raw
//!      markers stripped), and the result is always a string;
//!    - a **map** renders each key in ONE pass (the key goes through
//!      `render(String, Map)`, which reads `recursiveRendering`, `false` by
//!      default) and each value as an object, keeping the first value of two
//!      keys that render alike; a key that is not a string fails with a
//!      `ClassCastException`;
//!    - a **list** renders each element as an object;
//!    - anything else (a number, a boolean, a null) is returned as it is.
//!
//! `renderOnce(x)` is `render(x, recursive=false)` with `toRender` as its
//! only declared argument.
//!
//! # How egret does it
//!
//! [`render`] and [`render_once`] are registered on the shared
//! [`super::env::environment`]. A call builds a **fresh** `environment()`,
//! binds the caller's variables into it and runs [`super::env::render_pass`]
//! -- the same front end, raw-region protection and output cap as every other
//! pass -- on the calling thread.
//!
//! # The confinement (spec D9 rules 1-5, user decision Q4)
//!
//! Q4 allows `render()` on run-time data (inputs, webhook payloads), which an
//! author does not write. What keeps that from reaching more than the
//! author's own text reaches:
//!
//! 1. **`secret()` stays refused.** The nested render runs on a fresh
//!    `environment()`, never on the caller's, so it does not inherit the
//!    resolving `secret` that `runners::secrets` registers on a plugin task's
//!    copy: text `render()` evaluates meets `functions::SECRET_REFUSAL`. And
//!    because a template can hand a function to the nested render as a value
//!    (`{% set s = secret %}{{ render(inputs.x) }}` with `inputs.x =
//!    "{{ s('db') }}"`; Pebble has no function values, minijinja does), the
//!    resolving `secret` itself refuses while [`in_nested_render`] -- a
//!    second wall, measured necessary: without it that alias resolves `db`.
//! 2. No secret name is collected from run-time text: the collecting render
//!    meets the same two walls, so a key reachable only through `render()`
//!    is never fetched.
//! 3. (`secrets::text_uses_secret`'s doc comment states 1 and 2.)
//! 4. **The forbidden roots carry in.** A webhook's `inputs:` render with
//!    `trigger` forbidden ([`super::env::render_str_guarded`]); every pass of a
//!    nested render asks the same question of the text it is about to
//!    evaluate, and a hit is reported to the outer render as
//!    `GuardedRender::Forbidden`, so `render(vars.tpl)` with `vars.tpl =
//!    "{{ trigger }}"` is refused as the outer text would be.
//! 5. `http()` stays refused (the shared environment's refusal).
//!
//! # Bounds (egret's, Kestra has none of the last three)
//!
//! - **Depth:** Kestra's 16, counted as Kestra counts it, but kept in a
//!   thread-local the template cannot reach. Kestra reads it back from a
//!   variable, so `{% set __kestra_render_depth = -100 %}` there lets 116
//!   nested calls through (measured); here the variable is published for
//!   reading only (recorded divergence).
//! - **Each pass** has at most [`super::env::environment`]'s fuel and the
//!   1 MiB output cap ([`super::env::MAX_OUTPUT_BYTES`]).
//! - **Calls and passes, shared:** at most [`MAX_NESTED_PASSES`] units in one
//!   top-level render, all depths and all calls together: every `render()`
//!   call costs one, whatever it renders, and every nested pass one more.
//!   Depth alone is no bound on work: a call is one VM instruction, so a
//!   template can make tens of thousands of them at every level.
//! - **Fuel, shared** (fix wave #5b, M2): the nested passes of one top-level
//!   render consume at most [`MAX_NESTED_FUEL`] instructions together.
//! - **Output, shared** (fix wave #5b, M1): the nested passes of one
//!   top-level render produce at most [`MAX_NESTED_OUTPUT_BYTES`] together,
//!   so the values a template keeps from `render()` are bounded in total and
//!   not only per pass.
//! - **The per-render budgets are shared, not renewed, over the WHOLE
//!   top-level render** -- its fixpoint passes and every nested pass: the
//!   `replace` scan budget is the render thread's (the nested passes run on
//!   it and install none), and the list operators' work tally
//!   (`arith::MAX_LIST_OPERATOR_WORK`) is the [`Scope`]'s
//!   ([`render_list_work`], fix wave #5b, M3; before it, each fixpoint pass
//!   started a fresh tally). A `render()` of a map or a list charges every
//!   element it visits to that tally (fix wave #5b, I2), since the walk
//!   copies them and costs its caller one instruction.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use minijinja::value::{Rest, Value as MjValue, ValueKind};
use minijinja::{Environment, Error as MjError, State};

use super::env::{
    environment, referenced_roots, render_pass_metered, strip_raw_markers, ContextScope,
    OrderedMap, PassError, CONTEXT_SCOPE_NAMES, FUEL, MAX_RENDERING_AMOUNT,
};
use super::filters::{java_class_name, pebble_error, Args};

/// `VariableConfiguration.DEFAULT_MAX_RENDER_DEPTH`.
pub(super) const MAX_RENDER_DEPTH: usize = 16;

/// `VariableRenderer.RENDER_DEPTH_VAR`: the variable a nested render
/// publishes its depth under. Read-only here; see the module doc.
pub(super) const RENDER_DEPTH_VAR: &str = "__kestra_render_depth";

/// The most nested passes one top-level render runs, over every `render()`
/// call at every depth, each call itself counting as one (fix wave #5b, I2:
/// a call whose value holds no `{` runs no pass, and used to cost nothing).
/// egret's (Kestra has no such bound).
///
/// Each pass is bounded by fuel and the 1 MiB output cap; what this bounds is
/// their NUMBER, which nothing else does: the top-level render alone runs at
/// most `MAX_RENDERING_AMOUNT + 1` = 101 passes, and a `render()` call costs
/// its caller a few units of fuel, so without this a template could start
/// thousands of nested renders at each of 16 levels. A thousand is ten times
/// the top level's own pass bound, and far more than a template that renders
/// an input or a list of them needs.
pub(super) const MAX_NESTED_PASSES: usize = 1_000;

/// The most fuel (minijinja VM instructions) the nested passes of one
/// top-level render consume together (fix wave #5b, M2). egret's.
///
/// Each nested pass used to get a fresh [`FUEL`] (50 000), so one render
/// could run `(101 + 1 000) x 50 000` instructions, about 11 times the
/// top level's own `101 x 50 000`. Twenty passes' worth keeps the nested
/// share under a fifth of the top level's: the worst case is now
/// `101 x 50 000 + 1 000 000` = 6.05 M instructions. A `render()` of an
/// input costs a handful of instructions per pass (`{{ x }}` is 3), so a
/// template that renders hundreds of inputs stays far below it. A pass that
/// runs it out fails with [`NESTED_FUEL_REFUSAL`].
pub(super) const MAX_NESTED_FUEL: u64 = 1_000_000;

/// The most bytes the nested passes of one top-level render produce
/// together (fix wave #5b, M1). egret's.
///
/// Each pass is capped at `MAX_OUTPUT_BYTES` (1 MiB), but a template keeps
/// what `render()` answers (`renderOnce([t, t, …])`), so 1 000 passes could
/// hold about 1 GiB at once. Sixteen passes at the cap: every output counts,
/// the intermediate passes of a fixpoint included, which is also what the
/// passes spent building them.
pub(super) const MAX_NESTED_OUTPUT_BYTES: usize = 16 << 20;

/// [`MAX_NESTED_FUEL`]'s refusal.
const NESTED_FUEL_REFUSAL: &str = "egret refuses render() passes that run more than 1000000 \
     instructions in one render";

/// The line placeholder of the depth refusal, filled by the render that ran
/// the refused call (only it knows the line). Not text a template can make
/// minijinja put in an error.
const LINE_PENDING: &str = "\u{0}egret render line\u{0}";

/// What every `render()` in one top-level render shares.
struct Scope {
    /// The depth the code now running renders at: 0 for the top-level text.
    depth: usize,
    /// The roots the top-level render refuses (`["trigger"]` for a
    /// webhook's `inputs:`), checked on every nested pass.
    forbidden: Vec<String>,
    /// What is left of [`MAX_NESTED_PASSES`].
    passes_left: usize,
    /// What is left of [`MAX_NESTED_FUEL`].
    fuel_left: u64,
    /// What is left of [`MAX_NESTED_OUTPUT_BYTES`].
    output_left: usize,
    /// The list operators' work tally of the whole top-level render: its
    /// fixpoint passes and every nested pass (fix wave #5b, M3).
    list_work: Arc<AtomicU64>,
    /// The forbidden root a nested pass met.
    forbidden_hit: Option<String>,
}

thread_local! {
    /// The [`Scope`] of the render running on this thread. A thread-local
    /// because a render owns its thread (`env::RENDER_STACK_BYTES`) and a
    /// nested render runs on it, inside the caller's function call, where
    /// minijinja gives a function no other place to keep state that the
    /// nested render's own `State` can see.
    static SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

/// Installs a fresh [`Scope`] for one top-level render, and puts back the
/// one it replaced when dropped.
pub(super) struct RenderScope {
    previous: Option<Scope>,
}

impl RenderScope {
    /// Called once per top-level render, on its thread, before any pass.
    pub(super) fn install(forbidden: &[&str]) -> Self {
        let fresh = Scope {
            depth: 0,
            forbidden: forbidden.iter().map(|f| (*f).to_string()).collect(),
            passes_left: MAX_NESTED_PASSES,
            fuel_left: MAX_NESTED_FUEL,
            output_left: MAX_NESTED_OUTPUT_BYTES,
            list_work: Arc::default(),
            forbidden_hit: None,
        };
        let previous = SCOPE.with(|scope| scope.borrow_mut().replace(fresh));
        Self { previous }
    }

    /// The forbidden root a nested render met, if any.
    pub(super) fn forbidden_hit(&self) -> Option<String> {
        SCOPE.with(|scope| {
            scope
                .borrow()
                .as_ref()
                .and_then(|scope| scope.forbidden_hit.clone())
        })
    }
}

impl Drop for RenderScope {
    fn drop(&mut self) {
        let previous = self.previous.take();
        SCOPE.with(|scope| *scope.borrow_mut() = previous);
    }
}

/// Whether the code now running is text a `render()` evaluates (depth 1 or
/// more). The resolving `secret` refuses there (D9 rule 1, second wall).
pub(crate) fn in_nested_render() -> bool {
    SCOPE.with(|scope| scope.borrow().as_ref().is_some_and(|scope| scope.depth > 0))
}

/// The list operators' work tally of the top-level render running on this
/// thread, shared by its every pass, nested or not (fix wave #5b, M3).
/// `None` outside one (an environment composed by hand), where each `State`
/// keeps its own.
pub(super) fn render_list_work() -> Option<Arc<AtomicU64>> {
    SCOPE.with(|scope| scope.borrow().as_ref().map(|scope| scope.list_work.clone()))
}

/// Sets the depth for the length of one `render()` call, and puts the
/// caller's back when dropped (a failure or a panic included).
struct Descent {
    depth: usize,
}

impl Descent {
    fn enter(depth: usize) -> Self {
        SCOPE.with(|scope| {
            let mut scope = scope.borrow_mut();
            let scope = scope.as_mut().expect("checked by the caller");
            let saved = Self { depth: scope.depth };
            scope.depth = depth;
            saved
        })
    }
}

impl Drop for Descent {
    fn drop(&mut self) {
        let depth = self.depth;
        SCOPE.with(|scope| {
            if let Some(scope) = scope.borrow_mut().as_mut() {
                scope.depth = depth;
            }
        });
    }
}

/// `render(toRender, recursive=true)`.
pub(super) fn render(state: &State, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    bind(&args, &["toRender", "recursive"])?;
    let recursive = match args.get("recursive", 1) {
        None => Some(true),
        Some(value) if value.is_none() || value.is_undefined() => Some(true),
        Some(value) if value.kind() == ValueKind::Bool => Some(value.is_true()),
        Some(_) => None,
    };
    execute(state, args.get("toRender", 0), recursive)
}

/// `renderOnce(toRender)`: `render` with `recursive` false, and `toRender`
/// its only argument.
pub(super) fn render_once(state: &State, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    bind(&args, &["toRender"])?;
    execute(state, args.get("toRender", 0), Some(false))
}

/// Pebble's own argument checks (`ArgumentsNode`), made before the function
/// runs, with Pebble's sentences.
fn bind(args: &Args, declared: &[&str]) -> Result<(), MjError> {
    let positional = args.positional_only().len();
    if positional > declared.len() {
        return Err(pebble_error(format!(
            "The argument at position {} is not allowed. Only {} argument(s) are allowed.",
            declared.len() + 1,
            declared.len()
        )));
    }
    if let Some(unknown) = args.named_keys().find(|k| !declared.contains(k)) {
        return Err(pebble_error(format!(
            "The following named argument does not exist: {unknown}"
        )));
    }
    Ok(())
}

/// The depth the next nested render would run at, or Kestra's refusal.
fn check_depth() -> Result<usize, MjError> {
    let depth = SCOPE.with(|scope| scope.borrow().as_ref().map(|scope| scope.depth));
    let Some(depth) = depth else {
        // Every render path installs a scope; a render composed by hand from
        // `environment()` does not, and gets no nesting at all.
        return Err(pebble_error(
            "internal: render() was called outside a render of egret's renderer",
        ));
    };
    if depth >= MAX_RENDER_DEPTH {
        return Err(pebble_error(LINE_PENDING));
    }
    Ok(depth)
}

/// `RenderFunction.execute`, in its order: the depth, `toRender`, then
/// `recursive` (`None`: not a boolean).
fn execute(
    state: &State,
    to_render: Option<MjValue>,
    recursive: Option<bool>,
) -> Result<MjValue, MjError> {
    let depth = check_depth()?;
    let Some(value) = to_render else {
        return Err(pebble_error(
            "The 'render' function expects an argument 'toRender'.",
        ));
    };
    let Some(recursive) = recursive else {
        return Err(pebble_error(
            "The 'render' function expects an optional argument 'recursive' with type boolean.",
        ));
    };
    // A strict undefined fails as the VM's own undefined, as Pebble's strict
    // variables fail before the function runs; a silent one (a missing
    // `else`) is egret's spelling of null.
    super::compare::assert_defined(state, &value)?;
    // Fix wave #5b, I2: the call itself is charged, whatever it renders.
    charge_pass()?;

    let tally = super::arith::list_work_tally(state);
    let mut nested = Nested::new(state, depth + 1, tally);
    let _descent = Descent::enter(depth + 1);
    nested.object(&value, recursive)
}

/// One `render()` call's nested render: the variables it renders against
/// and the environment, built on the first string that needs it.
struct Nested<'s> {
    caller: &'s State<'s, 's>,
    depth: usize,
    env: Option<(Environment<'static>, MjValue)>,
    /// The top-level render's list-work tally, charged per element visited.
    list_work: Arc<AtomicU64>,
}

impl<'s> Nested<'s> {
    fn new(caller: &'s State<'s, 's>, depth: usize, list_work: Arc<AtomicU64>) -> Self {
        Self {
            caller,
            depth,
            env: None,
            list_work,
        }
    }

    /// `VariableRenderer.renderObject`.
    fn object(&mut self, value: &MjValue, recursive: bool) -> Result<MjValue, MjError> {
        match value.kind() {
            ValueKind::String => {
                let text = value.as_str().unwrap_or("");
                // No `{`: the value itself, not a copy (fix wave #5b, M1).
                if !text.contains('{') && !value.is_safe() {
                    return Ok(value.clone());
                }
                Ok(MjValue::from(self.string(text, recursive)?))
            }
            ValueKind::Map => {
                let mut entries: Vec<(String, MjValue)> = Vec::new();
                // `putIfAbsent` is a hash lookup; a scan of `entries` made
                // the walk quadratic in the key count (fix wave #5b, I2).
                let mut seen: HashSet<String> = HashSet::new();
                for key in value.try_iter()? {
                    super::arith::charge_tally(&self.list_work, 1)?;
                    let Some(text) = key.as_str() else {
                        let class = java_class_name(&key);
                        return Err(pebble_error(format!(
                            "class {class} cannot be cast to class java.lang.String ({class} and \
                             java.lang.String are in module java.base of loader 'bootstrap')"
                        )));
                    };
                    // `render(String, Map)`: the configured recursion,
                    // `false` by default, so one pass.
                    let key_text = self.string(text, false)?;
                    let item = value.get_item(&key)?;
                    let rendered = self.object(&item, recursive)?;
                    // `putIfAbsent`: the first of two keys that render alike,
                    // in insertion order.
                    if seen.insert(key_text.clone()) {
                        entries.push((key_text, rendered));
                    }
                }
                Ok(MjValue::from_object(OrderedMap::new(entries)))
            }
            ValueKind::Seq | ValueKind::Iterable => {
                let mut items = Vec::new();
                for item in value.try_iter()? {
                    super::arith::charge_tally(&self.list_work, 1)?;
                    items.push(self.object(&item, recursive)?);
                }
                Ok(MjValue::from(items))
            }
            ValueKind::Undefined => Ok(MjValue::from(())),
            _ => Ok(value.clone()),
        }
    }

    /// `VariableRenderer.render(String, Map, recursive, stringify = true)`.
    fn string(&mut self, text: &str, recursive: bool) -> Result<String, MjError> {
        // "it's not a Pebble template so we short-circuit rendering"; it
        // cannot name a forbidden root either.
        if !text.contains('{') {
            return Ok(text.to_string());
        }
        if self.env.is_none() {
            self.env = Some(self.environment());
        }
        let (env, variables) = self.env.as_mut().expect("built above");
        let mut current = text.to_string();
        let passes = if recursive {
            MAX_RENDERING_AMOUNT + 1
        } else {
            1
        };
        for _ in 0..passes {
            charge_pass()?;
            check_forbidden(&current)?;
            let allotment = take_fuel()?;
            env.set_fuel(Some(allotment));
            let (next, consumed) = render_pass_metered(env, &current, variables);
            spend_fuel(consumed.unwrap_or(allotment));
            let next = match next {
                Err(PassError::Engine(e))
                    if e.kind() == minijinja::ErrorKind::OutOfFuel && allotment < FUEL =>
                {
                    return Err(pebble_error(NESTED_FUEL_REFUSAL));
                }
                next => next.map_err(nested_error)?,
            };
            charge_output(next.len())?;
            if !recursive || next == current {
                return Ok(strip_raw_markers(&next));
            }
            current = next;
        }
        Err(pebble_error("Too many rendering attempts"))
    }

    /// A fresh [`environment`] (D9 rule 1) with the caller's variables, the
    /// render context's scope sentinel over them, and the caller's file
    /// scope -- what the author's own text reaches (rule 5).
    fn environment(&self) -> (Environment<'static>, MjValue) {
        let caller = self.caller;
        let globals: Vec<(String, MjValue)> = caller
            .env()
            .globals()
            .map(|(name, value)| (name.to_string(), value))
            .collect();
        let mut names: Vec<String> = caller
            .known_variables()
            .into_iter()
            .map(|name| name.into_owned())
            .filter(|name| name != RENDER_DEPTH_VAR)
            .collect();
        names.sort();
        let mut entries: Vec<(String, MjValue)> = Vec::with_capacity(names.len() + 1);
        for name in names {
            let Some(value) = caller.lookup(&name) else {
                continue;
            };
            if value.is_undefined() {
                continue;
            }
            // The engine's own globals (functions, filters' helpers, the
            // scope sentinels) are the environment's, not the caller's
            // variables: the fresh environment has its own. Identity, as
            // `functions::fetch_context` decides it.
            if globals
                .iter()
                .any(|(global, held)| *global == name && *held == value)
            {
                continue;
            }
            entries.push((name, value));
        }
        entries.push((RENDER_DEPTH_VAR.to_string(), MjValue::from(self.depth)));

        let mut env = environment();
        env.add_global(
            CONTEXT_SCOPE_NAMES,
            MjValue::from_object(ContextScope(
                entries.iter().map(|(name, _)| name.clone()).collect(),
            )),
        );
        if let Some((_, files)) = globals
            .iter()
            .find(|(name, _)| name == super::files::FILE_SCOPE)
        {
            env.add_global(super::files::FILE_SCOPE, files.clone());
        }
        // Package #5b Task B7: `errorLogs()` answers inside `render()` on the
        // worker, as Kestra's nested render on the worker does.
        if let Some((_, logs)) = globals
            .iter()
            .find(|(name, _)| name == super::error_logs::ERROR_LOGS_SCOPE)
        {
            env.add_global(super::error_logs::ERROR_LOGS_SCOPE, logs.clone());
        }
        (env, MjValue::from_object(OrderedMap::new(entries)))
    }
}

/// Charges one nested pass to [`MAX_NESTED_PASSES`].
fn charge_pass() -> Result<(), MjError> {
    SCOPE.with(|scope| {
        let mut scope = scope.borrow_mut();
        let scope = scope.as_mut().expect("checked by check_depth");
        if scope.passes_left == 0 {
            return Err(pebble_error(format!(
                "egret refuses more than {MAX_NESTED_PASSES} render() passes in one render"
            )));
        }
        scope.passes_left -= 1;
        Ok(())
    })
}

/// The fuel the next nested pass may consume: [`FUEL`], or what is left of
/// [`MAX_NESTED_FUEL`] when that is less.
fn take_fuel() -> Result<u64, MjError> {
    SCOPE.with(|scope| {
        let scope = scope.borrow();
        let scope = scope.as_ref().expect("checked by check_depth");
        if scope.fuel_left == 0 {
            return Err(pebble_error(NESTED_FUEL_REFUSAL));
        }
        Ok(scope.fuel_left.min(FUEL))
    })
}

/// Takes what a nested pass consumed off [`MAX_NESTED_FUEL`].
fn spend_fuel(consumed: u64) {
    SCOPE.with(|scope| {
        if let Some(scope) = scope.borrow_mut().as_mut() {
            scope.fuel_left = scope.fuel_left.saturating_sub(consumed);
        }
    });
}

/// Charges a nested pass's output to [`MAX_NESTED_OUTPUT_BYTES`].
fn charge_output(bytes: usize) -> Result<(), MjError> {
    SCOPE.with(|scope| {
        let mut scope = scope.borrow_mut();
        let scope = scope.as_mut().expect("checked by check_depth");
        if bytes > scope.output_left {
            return Err(pebble_error(format!(
                "egret refuses render() passes that produce more than \
                 {MAX_NESTED_OUTPUT_BYTES} bytes in one render"
            )));
        }
        scope.output_left -= bytes;
        Ok(())
    })
}

/// D9 rule 4: refuse a nested pass whose text names a forbidden root, and
/// tell the top-level render.
fn check_forbidden(text: &str) -> Result<(), MjError> {
    let forbidden = SCOPE.with(|scope| {
        scope
            .borrow()
            .as_ref()
            .map(|scope| scope.forbidden.clone())
            .unwrap_or_default()
    });
    if forbidden.is_empty() {
        return Ok(());
    }
    // A text that does not parse names nothing; its pass then fails with the
    // parser's own message, as at the top level.
    let Ok(roots) = referenced_roots(text) else {
        return Ok(());
    };
    let Some(hit) = forbidden.into_iter().find(|f| roots.contains(f)) else {
        return Ok(());
    };
    let message = format!("render() was refused: the text it renders names `{hit}`");
    SCOPE.with(|scope| {
        if let Some(scope) = scope.borrow_mut().as_mut() {
            scope.forbidden_hit.get_or_insert(hit);
        }
    });
    Err(pebble_error(message))
}

/// A nested pass's failure as the caller's error. The depth refusal gets the
/// line of the refused call here, from the engine error that carries it.
fn nested_error(error: PassError) -> MjError {
    match error {
        PassError::Render(e) => pebble_error(e.to_string()),
        PassError::Engine(e) if e.detail() == Some(LINE_PENDING) => pebble_error(format!(
            "Maximum render() nesting depth ({MAX_RENDER_DEPTH}) exceeded at line {} — check \
             for circular render() calls in your template.",
            e.line().unwrap_or(1)
        )),
        // Kestra wraps a nested failure in a `PebbleException` of its own
        // message; the kind is not carried, so nothing outside can read a
        // nested undefined as its own.
        PassError::Engine(e) => {
            pebble_error(e.detail().map_or_else(|| e.to_string(), str::to_string))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::env::{render_str, render_str_guarded, GuardedRender, RenderContext};
    use serde_json::json;

    fn cx(ctx: serde_json::Value) -> RenderContext {
        RenderContext::from_json_str(&ctx.to_string()).expect("a JSON object")
    }

    fn render_in(template: &str, ctx: serde_json::Value) -> Result<String, String> {
        render_str(template, &cx(ctx)).map_err(|e| e.to_string())
    }

    fn render(template: &str) -> Result<String, String> {
        render_in(template, json!({}))
    }

    /// `t0` is `x`; each `tK` calls `function(t{K-1})`.
    fn chain(top: usize, function: &str) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert("t0".into(), json!("x"));
        for k in 1..=top {
            map.insert(
                format!("t{k}"),
                json!(format!("{{{{ {function}(t{}) }}}}", k - 1)),
            );
        }
        serde_json::Value::Object(map)
    }

    fn depth_sentence(line: usize) -> String {
        format!(
            "Maximum render() nesting depth (16) exceeded at line {line} — check for circular \
             render() calls in your template."
        )
    }

    /// Kestra's count (`RenderFunction.java:46-54`, measured through the
    /// harness: `function-render-depth-16`, `-17`, `-circular`): the
    /// top-level call runs at depth 0 and a call at depth 16 fails, so 16
    /// nested calls answer and the 17th fails with Kestra's sentence, naming
    /// the line of the refused call in ITS text. `renderOnce` counts the
    /// same. The variable Kestra reads the depth from is published, but the
    /// count is not read back from it.
    #[test]
    fn render_nesting_stops_where_kestras_does() {
        for function in ["render", "renderOnce"] {
            assert_eq!(
                render_in(&format!("{{{{ {function}(t15) }}}}"), chain(15, function)),
                Ok("x".to_string()),
                "{function}: 16 calls"
            );
            let e = render_in(&format!("{{{{ {function}(t16) }}}}"), chain(16, function))
                .expect_err("17 calls");
            assert!(e.contains(&depth_sentence(1)), "{function}: {e}");
        }
        let e = render_in("{{ render(t) }}", json!({"t": "{{ render(t) }}"})).unwrap_err();
        assert!(e.contains(&depth_sentence(1)), "{e}");
        // Kestra: `at line 3` (the harness, `circular-line2`).
        let e = render_in("a\n{{ render(t) }}", json!({"t": "\n\n{{ render(t) }}"})).unwrap_err();
        assert!(e.contains(&depth_sentence(3)), "{e}");
        // Kestra lets this template reset its own count (116 calls, measured);
        // here the variable is only read.
        let e = render_in(
            "{% set __kestra_render_depth = -100 %}{{ render(t) }}",
            json!({"t": "{{ render(t) }}"}),
        )
        .unwrap_err();
        assert!(e.contains(&depth_sentence(1)), "{e}");
        assert_eq!(
            render("{{ render('{{ __kestra_render_depth }}') }}"),
            Ok("1".into())
        );
        assert_eq!(
            render("{{ render(\"{{ render('{{ __kestra_render_depth }}') }}\") }}"),
            Ok("2".into())
        );
        assert!(render("{{ __kestra_render_depth }}").is_err());
    }

    /// `recursive`: a boolean, or null (= `true`); anything else is Kestra's
    /// sentence. `false` renders one pass.
    #[test]
    fn a_recursive_that_is_not_a_boolean_is_refused() {
        let want = "The 'render' function expects an optional argument 'recursive' with type \
                    boolean.";
        for call in [
            "{{ render('x', recursive='true') }}",
            "{{ render('x', recursive=1) }}",
        ] {
            let e = render(call).unwrap_err();
            assert!(e.contains(want), "{call}: {e}");
        }
        let abc = json!({"a": "{{ b }}", "b": "{{ c }}", "c": "z"});
        assert_eq!(
            render_in("{{ render(a, recursive=null) == 'z' }}", abc.clone()),
            Ok("true".into())
        );
        assert_eq!(
            render_in("{{ render(a) == 'z' }}", abc.clone()),
            Ok("true".into())
        );
        for call in [
            "{{ render(a, recursive=false) }}",
            "{{ render(a, false) }}",
            "{{ renderOnce(a) }}",
        ] {
            assert_eq!(
                render_in(
                    &format!("{{{{ {} == '{{{{ c }}}}' }}}}", &call[3..call.len() - 3]),
                    abc.clone()
                ),
                Ok("true".into()),
                "{call}: one pass"
            );
        }
    }

    /// Pebble's argument binding, with Pebble's sentences, and Kestra's for a
    /// missing `toRender`.
    #[test]
    fn render_binds_its_arguments_as_pebble_does() {
        for (call, want) in [
            (
                "{{ render('x', true, 1) }}",
                "The argument at position 3 is not allowed. Only 2 argument(s) are allowed.",
            ),
            (
                "{{ render('x', depth=1) }}",
                "The following named argument does not exist: depth",
            ),
            (
                "{{ render() }}",
                "The 'render' function expects an argument 'toRender'.",
            ),
            (
                "{{ renderOnce('x', recursive=true) }}",
                "The following named argument does not exist: recursive",
            ),
            (
                "{{ renderOnce('x', true) }}",
                "The argument at position 2 is not allowed. Only 1 argument(s) are allowed.",
            ),
            (
                "{{ renderOnce() }}",
                "The 'render' function expects an argument 'toRender'.",
            ),
        ] {
            let e = render(call).unwrap_err();
            assert!(e.contains(want), "{call}: {e}");
        }
        assert_eq!(render("{{ render(toRender='{{ 1 }}') }}"), Ok("1".into()));
    }

    /// A map renders key by key; a key that is not a string is Kestra's
    /// `ClassCastException`.
    #[test]
    fn a_map_key_that_is_not_a_string_is_refused() {
        let e = render("{{ render({1: '{{ 2 }}'}) }}").unwrap_err();
        assert!(
            e.contains("cannot be cast to class java.lang.String"),
            "{e}"
        );
        assert_eq!(
            render("{{ render({'1': '{{ 2 }}'}) }}"),
            Ok(r#"{"1":"2"}"#.into())
        );
    }

    /// A non-string is returned as it is; a map and a list render inside
    /// (keys in one pass, first of two equal keys kept, a null value kept and
    /// then omitted by the writer); a string's answer is always a string.
    #[test]
    fn a_value_renders_as_renderobject_renders_it() {
        assert_eq!(render("{{ render(5) + 1 }}"), Ok("6".into()));
        assert_eq!(render("{{ render(true) }}"), Ok("true".into()));
        assert_eq!(render("{{ render(null) }}"), Ok("".into()));
        assert_eq!(render("{{ render('{{ 1 + 1 }}') + 1 }}"), Ok("21".into()));
        assert_eq!(
            render("{{ render('{{ [1, 2] }}') | length }}"),
            Ok("5".into())
        );
        assert_eq!(
            render_in(
                "{{ render(m) }}",
                json!({"m": {"{{ 'a' }}": 1, "a": 2, "n": null}})
            ),
            Ok(r#"{"a":1}"#.into())
        );
        assert_eq!(
            render_in(
                "{{ render(l) }}",
                json!({"l": ["{{ 1 + 1 }}", 3, ["{{ 2 }}"], null]})
            ),
            Ok(r#"["2",3,["2"],null]"#.into())
        );
        // The key in ONE pass, the value to a fixpoint.
        assert_eq!(
            render_in(
                "{{ render(m) | keys | first == '{{ j }}' }}",
                json!({"m": {"{{ k }}": "{{ k }}"}, "k": "{{ j }}", "j": "z"})
            ),
            Ok("true".into())
        );
        assert_eq!(
            render_in(
                "{{ render(m)['{{ j }}'] }}",
                json!({"m": {"{{ k }}": "{{ k }}"}, "k": "{{ j }}", "j": "z"})
            ),
            Ok("z".into())
        );
    }

    /// What the nested render sees: the context, and what `{% set %}` and
    /// `{% for %}` bound, as Kestra's global scopes hold them.
    #[test]
    fn a_nested_render_sees_the_callers_variables() {
        assert_eq!(
            render("{% set x = 1 %}{{ render('{{ x }}') }}"),
            Ok("1".into())
        );
        assert_eq!(
            render("{% for i in [7] %}{{ render('{{ i }}') }}{% endfor %}"),
            Ok("7".into())
        );
        assert_eq!(
            render_in("{{ render('{{ a.b }}') }}", json!({"a": {"b": 5}})),
            Ok("5".into())
        );
        assert_eq!(
            render_in(
                "{{ render(a) }}",
                json!({"a": "{{ 7 / 2 }} {{ x ?? 'd' }} {{ [1, 2] contains 2 }}"})
            ),
            Ok("3 d true".into()),
            "the same front end"
        );
    }

    /// A nested failure fails the caller, for Kestra's reason: an undefined
    /// root is egret's "undefined value" where Kestra names the
    /// root attribute (`function-render-missing-variable`, `-missing-argument`,
    /// `-raw`), a parse failure the parser's (`-syntax-error`).
    #[test]
    fn a_nested_render_fails_as_its_text_fails() {
        let e = render("{{ render('{{ nope }}') }}").unwrap_err();
        assert!(e.contains("undefined value"), "{e}");
        let e = render("{{ render(nope) }}").unwrap_err();
        assert!(e.contains("undefined value"), "{e}");
        // The raw region is stripped by `render()`, and the outer pass then
        // evaluates `{{ x }}`, an undefined `x` -- Kestra's harness fails the
        // same way (`function-render-raw`: the root-attribute reason).
        let e = render_in(
            "{{ render(a) }}",
            json!({"a": "{% raw %}{{ x }}{% endraw %}"}),
        )
        .unwrap_err();
        assert!(e.contains("undefined value"), "{e}");
        let e = render_in("{{ render(a) }}", json!({"a": "{{ a }}x"})).unwrap_err();
        assert!(e.contains("Too many rendering attempts"), "{e}");
        // `function-render-syntax-error`: the nested text's parser error
        // (Kestra: `ParserException: Unexpected token "EOF"`).
        let e = render_in("{{ render(a) }}", json!({"a": "{{"})).unwrap_err();
        assert!(
            e.contains("unexpected end of input, expected expression"),
            "{e}"
        );
    }

    /// D9 rule 1 on the shared environment (the plugin-task path is
    /// `secrets::tests::render_never_resolves_a_secret_from_run_time_text`).
    #[test]
    fn secret_stays_refused_in_text_render_evaluates() {
        let e = render_in("{{ render(a) }}", json!({"a": "{{ secret('db') }}"})).unwrap_err();
        assert!(e.contains(crate::runners::functions::SECRET_REFUSAL), "{e}");
    }

    /// D9 rule 4: a webhook's `inputs:` render with `trigger` forbidden, and
    /// a nested pass asks the same question of its own text. The context
    /// below BINDS `trigger`, so without the check the input would launder
    /// it: `render(vars.tpl)` names only `vars` at the top level.
    #[test]
    fn the_forbidden_roots_carry_into_a_nested_render() {
        let context = cx(json!({
            "vars": {"tpl": "{{ trigger.body }}", "deep": "{{ render(vars.tpl) }}", "m": {"k": "{{ trigger }}"}},
            "trigger": {"body": "laundered"},
        }));
        for template in [
            "{{ render(vars.tpl) }}",
            "{{ renderOnce(vars.tpl) }}",
            "{{ render(vars.deep) }}",
            "{{ render(vars.m) }}",
            "{{ render(vars.tpl) ?? 'x' }}",
        ] {
            assert_eq!(
                render_str_guarded(template, &context, &["trigger"]).expect("refused, not failed"),
                GuardedRender::Forbidden("trigger".into()),
                "{template}"
            );
        }
        // Nothing forbidden: the same text renders.
        assert_eq!(
            render_str("{{ render(vars.tpl) }}", &context).unwrap(),
            "laundered"
        );
    }

    /// Each nested pass has the 1 MiB output cap, also when its answer is
    /// never written out (here only its length is).
    #[test]
    fn a_nested_pass_has_the_output_cap() {
        let big = "a".repeat(600_000);
        let context = json!({"big": big, "x": "{{ big }}{{ big }}", "y": "{{ big }}"});
        let e = render_in("{{ render(x) | length }}", context.clone()).unwrap_err();
        assert!(e.contains("rendered output exceeded 1048576 bytes"), "{e}");
        assert_eq!(
            render_in("{{ render(y) | length }}", context),
            Ok("600000".into())
        );
    }

    /// The nested passes of one render share [`MAX_NESTED_PASSES`], over
    /// every call: depth alone bounds no work, since each level can make
    /// thousands of calls.
    #[test]
    fn the_nested_passes_share_one_budget() {
        let context = json!({"t": "{{ 1 }}"});
        // `render(t)` is three units: the call, and two passes (`{{ 1 }}`,
        // then `1` to see it is fixed). `range(1, n)` is n calls (Pebble's
        // range is inclusive).
        let loop_of =
            |n: usize| format!("{{% for i in range(1, {n}) %}}{{{{ render(t) }}}}{{% endfor %}}");
        assert_eq!(
            render_in(&loop_of(MAX_NESTED_PASSES / 3), context.clone()).map(|s| s.len()),
            Ok(MAX_NESTED_PASSES / 3)
        );
        let e = render_in(&loop_of(MAX_NESTED_PASSES / 3 + 1), context).unwrap_err();
        assert!(
            e.contains("egret refuses more than 1000 render() passes in one render"),
            "{e}"
        );
    }

    /// Fix wave #5b, I2: a call is charged even when its value holds no `{`
    /// and so runs no pass.
    #[test]
    fn a_call_without_a_brace_is_charged() {
        let loop_of =
            |n: usize| format!("{{% for i in range(1, {n}) %}}{{{{ render(5) }}}}{{% endfor %}}");
        assert_eq!(
            render(&loop_of(MAX_NESTED_PASSES)).map(|s| s.len()),
            Ok(MAX_NESTED_PASSES)
        );
        let e = render(&loop_of(MAX_NESTED_PASSES + 1)).unwrap_err();
        assert!(
            e.contains("egret refuses more than 1000 render() passes in one render"),
            "{e}"
        );
    }

    /// Fix wave #5b, I2: the walk of a map or a list charges every element
    /// to the render's list work, so a loop over `render(big)` meets the
    /// shared 10 M. 3 000 x 3 000 = 9 M are spent first; the 20 000 keys of
    /// `m` then cross it on the 51st call.
    #[test]
    fn a_walk_charges_the_list_work_budget() {
        let l: Vec<u32> = (0..3_000).collect();
        let m: serde_json::Map<String, serde_json::Value> =
            (0..20_000).map(|i| (format!("k{i}"), json!(i))).collect();
        let context = json!({"l": l, "m": m.clone(), "n": [m]});
        assert_eq!(
            render_in("{{ render(m) | length }}", context.clone()),
            Ok("20000".into())
        );
        for walk in ["render(m) | length", "render(n) | length"] {
            let template = format!(
                "{{% set a = (l - l) | length %}}\
                 {{% for i in range(1, 60) %}}{{{{ {walk} }}}}{{% endfor %}}"
            );
            let e = render_in(&template, context.clone()).unwrap_err();
            assert!(e.contains("egret refuses list operations"), "{walk}: {e}");
        }
    }

    /// Fix wave #5b, I2: equal keys are found by a hash lookup, not a scan
    /// of the keys kept so far (200 000 keys were 2 x 10^10 comparisons).
    /// First wins, in insertion order, as before.
    #[test]
    fn deduping_map_keys_is_linear() {
        let m: serde_json::Map<String, serde_json::Value> =
            (0..200_000).map(|i| (format!("k{i}"), json!(i))).collect();
        let started = std::time::Instant::now();
        assert_eq!(
            render_in("{{ render(m) | length }}", json!({"m": m})),
            Ok("200000".into())
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            render_in(
                "{{ render(m) }}",
                json!({"m": {"b": 0, "{{ 'a' }}": 1, "a": 2, "{{ 'b' }}": 3}})
            ),
            Ok(r#"{"b":0,"a":1}"#.into())
        );
    }

    /// Fix wave #5b, M3: the list work is ONE budget for the whole
    /// top-level render, its fixpoint passes included. Each pass here does
    /// 6.25 M; the second pass used to start a fresh tally.
    #[test]
    fn the_fixpoint_passes_share_the_list_work_budget() {
        let l: Vec<u32> = (0..2_500).collect();
        let context = json!({"l": l, "t": "{{ (l - l) | length }}"});
        assert_eq!(render_in("{{ t }}", context.clone()), Ok("0".into()));
        let e = render_in("{% set a = (l - l) | length %}{{ t }}", context).unwrap_err();
        assert!(e.contains("egret refuses list operations"), "{e}");
    }

    /// Fix wave #5b, M1: what the nested passes produce is bounded in total,
    /// not only per pass. 17 passes of 1 000 000 bytes cross 16 MiB.
    #[test]
    fn the_nested_outputs_share_one_byte_budget() {
        let context = json!({"big": "a".repeat(1_000_000), "t": "{{ big }}"});
        let of = |n: usize| {
            format!(
                "{{{{ renderOnce([{}]) | length }}}}",
                vec!["t"; n].join(", ")
            )
        };
        assert_eq!(render_in(&of(16), context.clone()), Ok("16".into()));
        let e = render_in(&of(17), context).unwrap_err();
        assert!(
            e.contains(&format!(
                "egret refuses render() passes that produce more than \
                 {MAX_NESTED_OUTPUT_BYTES} bytes in one render"
            )),
            "{e}"
        );
    }

    /// Fix wave #5b, M2: the nested passes share [`MAX_NESTED_FUEL`]; each
    /// used to get a fresh `FUEL`. `t` is a loop of 15 000 empty iterations,
    /// under one pass's 50 000.
    #[test]
    fn the_nested_passes_share_one_fuel_budget() {
        assert!(NESTED_FUEL_REFUSAL.contains(&MAX_NESTED_FUEL.to_string()));
        let context = json!({"t": "{% for i in range(1, 15000) %}{% endfor %}x"});
        let of = |n: usize| {
            format!("{{% for i in range(1, {n}) %}}{{{{ renderOnce(t) }}}}{{% endfor %}}")
        };
        assert_eq!(render_in(&of(5), context.clone()), Ok("xxxxx".into()));
        let e = render_in(&of(100), context).unwrap_err();
        assert!(e.contains(NESTED_FUEL_REFUSAL), "{e}");
    }

    /// The list operators' work budget is the CALLER's: a nested render
    /// charges it rather than starting its own, so `render()` cannot
    /// multiply `arith::MAX_LIST_OPERATOR_WORK`. 2 500 x 2 500 is 6.25 M,
    /// under the 10 M alone and over it twice.
    #[test]
    fn a_nested_render_shares_the_list_work_budget() {
        let l: Vec<u32> = (0..2_500).collect();
        let context = json!({"l": l, "t": "{{ (l - l) | length }}"});
        assert_eq!(
            render_in("{{ (l - l) | length }}", context.clone()),
            Ok("0".into())
        );
        assert_eq!(
            render_in("{{ render(t) }}", context.clone()),
            Ok("0".into())
        );
        for template in [
            "{% set a = (l - l) | length %}{{ render(t) }}",
            "{{ render(t) }}{{ render(t) }}",
        ] {
            let e = render_in(template, context.clone()).unwrap_err();
            assert!(
                e.contains("egret refuses list operations"),
                "{template}: {e}"
            );
        }
    }

    /// Sixteen nested renders, each of a deep expression, on the one render
    /// thread (`env::RENDER_STACK_BYTES`): a nested render runs on its
    /// caller's stack, and this is the deepest the cap lets it go.
    #[test]
    fn sixteen_nested_renders_of_a_deep_expression_fit_the_render_stack() {
        let deep = " | lower".repeat(1_600);
        let mut map = serde_json::Map::new();
        map.insert("v".into(), json!("A"));
        map.insert("t0".into(), json!(format!("{{{{ v{deep} }}}}")));
        for k in 1..=15 {
            map.insert(
                format!("t{k}"),
                json!(format!("{{{{ render(t{}){deep} }}}}", k - 1)),
            );
        }
        assert_eq!(
            render_in(
                &format!("{{{{ render(t15){deep} }}}}"),
                serde_json::Value::Object(map)
            ),
            Ok("a".into())
        );
    }
}
