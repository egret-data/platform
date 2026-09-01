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

use serde::de::{DeserializeOwned, Error as SerdeError};
use serde_json::{Map, Value};

/// A required field: missing key is `missing_field(key)`; a present value of
/// the wrong type is a real type error, not a second `missing_field`.
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

/// An optional field: missing key is `None`; a present value of the wrong
/// type is still a real type error, not a silent `None`.
pub(super) fn optional<T, E>(map: &mut Map<String, Value>, key: &str) -> Result<Option<T>, E>
where
    T: DeserializeOwned,
    E: SerdeError,
{
    match map.remove(key) {
        Some(v) => serde_json::from_value(v).map(Some).map_err(E::custom),
        None => Ok(None),
    }
}

/// A field with a Kestra-side default for the *absent* case (e.g.
/// `disabled` defaulting to `false`, `behavior` defaulting to
/// `RetryFailedTask`): missing key takes `default`; a present value of the
/// wrong type is still a real type error, never silently coerced to
/// `default`.
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
        Some(v) => serde_json::from_value(v).map_err(E::custom),
        None => Ok(default),
    }
}
