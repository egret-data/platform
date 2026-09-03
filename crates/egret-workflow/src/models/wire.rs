//! Shared helpers for the hand-written `Deserialize` impls in `task.rs` and
//! `trigger.rs`. Both dispatch a `type` tag by pulling fields out of a
//! `serde_json::Map` one at a time rather than using `#[derive(Deserialize)]`
//! -- and the first pass of both files made the same mistake doing it:
//! `map.remove(key).and_then(|v| v.as_bool()).unwrap_or(false)` treats a
//! *present but wrong-typed* value exactly like an *absent* one, silently
//! substituting the default instead of reporting an error. For a boolean
//! safety flag like `disabled`, that turns `disabled: "true"` (a quoted
//! string -- easy to type by accident in YAML) into a task or trigger that
//! silently runs anyway, with no error and nothing in a validation report.
//!
//! `required`/`optional`/`with_default` below are the fix: they always try to
//! deserialize a present value as `T`, and only fall back (to a
//! `missing_field` error, `None`, or a caller-supplied default,
//! respectively) when the key is genuinely absent. A caller-supplied value
//! of the wrong type always becomes a real deserialize error.
//!
//! Fix round 2: `optional`/`with_default`'s first pass called
//! `serde_json::from_value::<T>(v)` on a *present* `Value::Null` too, which
//! is a hard type error for every `T` used here (`String`, `bool`, ...) --
//! rejecting a `null` Kestra's own Jackson accepts. `Task.java:48`
//! (`description`, a plain nullable reference), `Task.java:63`
//! (`disabled`, boxed `Boolean` with `@Builder.Default`), and
//! `AbstractTrigger.java:60` (`disabled`, primitive `boolean`, and Jackson's
//! default `FAIL_ON_NULL_FOR_PRIMITIVES = false` coerces a `null` there to
//! the type default) all parse an explicit `null` without error in Kestra.
//! `explicit_null_still_parses_on_notnull_property_fields` in `task.rs`
//! already asserts this for the derive-based path; the hand-rolled path
//! must agree. Both functions now treat `Value::Null` as the *absent*
//! branch -- `None` / the given default -- before ever calling
//! `from_value`, so `null` and "key missing" behave identically, and only a
//! value that is present, non-null, AND wrong-typed is still a real error.

use serde::de::{DeserializeOwned, Error as SerdeError};
use serde_json::{Map, Value};

/// A required field: missing key is `missing_field(key)`; a present value of
/// the wrong type is a real type error, not a second `missing_field`.
///
/// Deliberately rejects an explicit `null` too, unlike `optional`/
/// `with_default` below -- decision (a) of the two the round-2 review asked
/// for. `required` is used only for `id`/`type` (`Task::id`/`r#type`,
/// `Trigger::id`/`r#type`): Kestra's own Jackson layer would accept
/// `id: null` and leave rejecting it to `@NotNull` bean validation, so this
/// IS a deliberate divergence from Kestra, not an application of the
/// `@NotNull`-is-bean-validation principle. It is scoped to these
/// *structural identity* fields specifically, for two reasons: (1) nothing
/// downstream in this crate can do anything sensible with a `null` id --
/// `Task::children()`/a future `findTaskByTaskId` index tasks by `id`, and a
/// `null` there is not "a task to validate later", it is a task no code path
/// can address; (2) this keeps the hand-rolled path consistent with the
/// *derive*-based path already established for identity fields elsewhere in
/// this crate -- `Flow::id`/`Flow::namespace` are plain (non-`Option`)
/// `String` fields under `#[derive(Deserialize)]`, and a plain `String`
/// field already rejects `null` there today. Making `id`/`type` `Option`
/// here without also changing every other identity field in the crate would
/// make the exact same kind of field parse differently depending on which
/// struct it's on, which is worse than a single documented divergence.
pub(super) fn required<T, E>(map: &mut Map<String, Value>, key: &'static str) -> Result<T, E>
where
    T: DeserializeOwned,
    E: SerdeError,
{
    match map.remove(key) {
        Some(v) => serde_json::from_value(v).map_err(E::custom),
        None => Err(E::missing_field(key)),
    }
}

/// An optional field: missing key OR an explicit `null` is `None`; a present
/// non-null value of the wrong type is still a real type error, not a
/// silent `None`.
pub(super) fn optional<T, E>(map: &mut Map<String, Value>, key: &str) -> Result<Option<T>, E>
where
    T: DeserializeOwned,
    E: SerdeError,
{
    match map.remove(key) {
        Some(Value::Null) | None => Ok(None),
        Some(v) => serde_json::from_value(v).map(Some).map_err(E::custom),
    }
}

/// A field with a Kestra-side default for the *absent* case (e.g.
/// `disabled` defaulting to `false`, `behavior` defaulting to
/// `RetryFailedTask`): missing key OR an explicit `null` takes `default`; a
/// present non-null value of the wrong type is still a real type error,
/// never silently coerced to `default`.
pub(super) fn with_default<T, E>(
    map: &mut Map<String, Value>,
    key: &str,
    default: T,
) -> Result<T, E>
where
    T: DeserializeOwned,
    E: SerdeError,
{
    match map.remove(key) {
        Some(Value::Null) | None => Ok(default),
        Some(v) => serde_json::from_value(v).map_err(E::custom),
    }
}
