//! Turning an alert query's result into a fire / don't-fire decision.
//!
//! Pure: no database, no clock, no engine. The value has already been reduced
//! to "the first column of the first row" (§5.2) by the time it arrives here,
//! so every interesting case — a null, a zero, a bound — is a unit test.
//!
//! **A missing value never fires.** An alert whose query returned no rows, or
//! returned NULL, has not observed its condition to be true; firing there would
//! page someone because a table was empty. The one place this is easy to get
//! wrong is a comparison like `> 0`, where a null coerced to zero silently
//! becomes a real answer.

use egret_automation_api::ValidatorType;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ValidatorError {
    #[error("validator_config is not usable: {0}")]
    BadConfig(String),
}

/// Decide whether `value` — the first column of the first row, `None` when the
/// query returned no rows or a NULL — satisfies the schedule's condition.
pub fn should_fire(
    kind: ValidatorType,
    config: &serde_json::Value,
    value: Option<f64>,
) -> Result<bool, ValidatorError> {
    match kind {
        // Superset's rule: non-null AND non-zero. Zero is the "nothing to
        // report" answer for the count(*) that most alerts are.
        ValidatorType::NotNull => Ok(matches!(value, Some(v) if v != 0.0)),
        ValidatorType::Operator => {
            let op = config
                .get("op")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ValidatorError::BadConfig("missing \"op\"".into()))?;
            // A null is not "less than" anything. Bail before reading the
            // operands so a bad config still reports as a bad config.
            let Some(v) = value else {
                // `between` needs its bounds validated even on the null path,
                // so an unusable config never masquerades as "did not fire".
                check_operands(op, config)?;
                return Ok(false);
            };
            match op {
                "between" | "not between" => {
                    let (lo, hi) = bounds(config)?;
                    let inside = v >= lo && v <= hi; // inclusive, both ends
                    Ok(if op == "between" { inside } else { !inside })
                }
                _ => {
                    let t = threshold(config)?;
                    Ok(match op {
                        "<" => v < t,
                        "<=" => v <= t,
                        ">" => v > t,
                        ">=" => v >= t,
                        "==" => v == t,
                        "!=" => v != t,
                        other => {
                            return Err(ValidatorError::BadConfig(format!(
                                "unknown operator: {other}"
                            )))
                        }
                    })
                }
            }
        }
    }
}

fn check_operands(op: &str, config: &serde_json::Value) -> Result<(), ValidatorError> {
    match op {
        "between" | "not between" => bounds(config).map(|_| ()),
        "<" | "<=" | ">" | ">=" | "==" | "!=" => threshold(config).map(|_| ()),
        other => Err(ValidatorError::BadConfig(format!(
            "unknown operator: {other}"
        ))),
    }
}

fn threshold(config: &serde_json::Value) -> Result<f64, ValidatorError> {
    config
        .get("threshold")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| ValidatorError::BadConfig("missing numeric \"threshold\"".into()))
}

fn bounds(config: &serde_json::Value) -> Result<(f64, f64), ValidatorError> {
    let lo = config
        .get("low")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| ValidatorError::BadConfig("missing numeric \"low\"".into()))?;
    let hi = config
        .get("high")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| ValidatorError::BadConfig("missing numeric \"high\"".into()))?;
    if lo > hi {
        return Err(ValidatorError::BadConfig("low must be <= high".into()));
    }
    Ok((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(o: &str, t: f64) -> serde_json::Value {
        serde_json::json!({ "op": o, "threshold": t })
    }

    fn fires(kind: ValidatorType, cfg: &serde_json::Value, v: Option<f64>) -> bool {
        should_fire(kind, cfg, v).expect("config should be valid")
    }

    #[test]
    fn not_null_fires_only_on_a_present_non_zero_value() {
        let cfg = serde_json::json!({});
        assert!(!fires(ValidatorType::NotNull, &cfg, None), "no rows");
        assert!(!fires(ValidatorType::NotNull, &cfg, Some(0.0)), "zero");
        assert!(fires(ValidatorType::NotNull, &cfg, Some(1.0)));
        assert!(
            fires(ValidatorType::NotNull, &cfg, Some(-3.0)),
            "non-zero includes negatives — the rule is not_null, not is_positive"
        );
    }

    #[test]
    fn a_missing_value_never_fires_whatever_the_operator() {
        // THE null trap: `> 0` against a null must not fire. If the value were
        // coerced to 0 somewhere, `< 1` would fire on an empty result set and
        // page someone because a table was empty.
        for o in ["<", "<=", ">", ">=", "==", "!="] {
            assert!(
                !fires(ValidatorType::Operator, &op(o, 0.0), None),
                "{o} must not fire on a missing value"
            );
        }
        let between = serde_json::json!({"op": "between", "low": -1, "high": 1});
        assert!(!fires(ValidatorType::Operator, &between, None));
        let not_between = serde_json::json!({"op": "not between", "low": -1, "high": 1});
        assert!(
            !fires(ValidatorType::Operator, &not_between, None),
            "`not between` must not fire on a missing value either — absence is \
             not evidence of being outside the range"
        );
    }

    #[test]
    fn every_comparison_operator_is_exact_at_its_boundary() {
        // Each operator asserted just below, exactly at, and just above the
        // threshold, so an off-by-one in either direction is caught.
        let cases: &[(&str, [bool; 3])] = &[
            //          (9.0,  10.0,  11.0)
            ("<", [true, false, false]),
            ("<=", [true, true, false]),
            (">", [false, false, true]),
            (">=", [false, true, true]),
            ("==", [false, true, false]),
            ("!=", [true, false, true]),
        ];
        for (o, expected) in cases {
            for (i, v) in [9.0, 10.0, 11.0].into_iter().enumerate() {
                assert_eq!(
                    fires(ValidatorType::Operator, &op(o, 10.0), Some(v)),
                    expected[i],
                    "{v} {o} 10 should be {}",
                    expected[i]
                );
            }
        }
    }

    #[test]
    fn between_is_inclusive_at_both_ends() {
        let cfg = serde_json::json!({"op": "between", "low": 10, "high": 20});
        assert!(!fires(ValidatorType::Operator, &cfg, Some(9.999)));
        assert!(
            fires(ValidatorType::Operator, &cfg, Some(10.0)),
            "low bound"
        );
        assert!(fires(ValidatorType::Operator, &cfg, Some(15.0)));
        assert!(
            fires(ValidatorType::Operator, &cfg, Some(20.0)),
            "high bound"
        );
        assert!(!fires(ValidatorType::Operator, &cfg, Some(20.001)));
    }

    #[test]
    fn not_between_is_the_exact_complement_of_between() {
        // Asserted as a property rather than by example: any value where both
        // agree would mean one of them is wrong at a bound.
        let inside = serde_json::json!({"op": "between", "low": 10, "high": 20});
        let outside = serde_json::json!({"op": "not between", "low": 10, "high": 20});
        for v in [0.0, 9.999, 10.0, 15.0, 20.0, 20.001, 100.0] {
            assert_ne!(
                fires(ValidatorType::Operator, &inside, Some(v)),
                fires(ValidatorType::Operator, &outside, Some(v)),
                "between and not between must disagree at {v}"
            );
        }
    }

    #[test]
    fn a_single_point_range_still_works() {
        let cfg = serde_json::json!({"op": "between", "low": 5, "high": 5});
        assert!(fires(ValidatorType::Operator, &cfg, Some(5.0)));
        assert!(!fires(ValidatorType::Operator, &cfg, Some(5.001)));
    }

    #[test]
    fn an_unusable_config_is_an_error_not_a_silent_no_fire() {
        // A no-fire would make a misconfigured alert indistinguishable from a
        // working one whose condition is never met — the failure mode that
        // makes people stop trusting alerts.
        assert!(matches!(
            should_fire(ValidatorType::Operator, &serde_json::json!({}), Some(1.0)),
            Err(ValidatorError::BadConfig(_))
        ));
        assert!(matches!(
            should_fire(
                ValidatorType::Operator,
                &serde_json::json!({"op": ">"}),
                Some(1.0)
            ),
            Err(ValidatorError::BadConfig(_))
        ));
        assert!(matches!(
            should_fire(
                ValidatorType::Operator,
                &serde_json::json!({"op": "≥", "threshold": 1}),
                Some(1.0)
            ),
            Err(ValidatorError::BadConfig(_))
        ));
        assert!(matches!(
            should_fire(
                ValidatorType::Operator,
                &serde_json::json!({"op": "between", "low": 20, "high": 10}),
                Some(15.0)
            ),
            Err(ValidatorError::BadConfig(_))
        ));
    }

    #[test]
    fn an_unusable_config_is_still_an_error_when_the_value_is_missing() {
        // Otherwise a broken alert reads as "ran fine, condition false" forever,
        // and nobody ever learns the config is wrong.
        assert!(matches!(
            should_fire(
                ValidatorType::Operator,
                &serde_json::json!({"op": ">"}),
                None
            ),
            Err(ValidatorError::BadConfig(_))
        ));
        assert!(matches!(
            should_fire(
                ValidatorType::Operator,
                &serde_json::json!({"op": "between", "low": 20, "high": 10}),
                None
            ),
            Err(ValidatorError::BadConfig(_))
        ));
    }

    #[test]
    fn not_null_ignores_the_config_entirely() {
        // It takes no operands, so a leftover `op` from switching the validator
        // in the editor must not change its behaviour.
        let leftover = serde_json::json!({"op": "between", "low": 20, "high": 10});
        assert!(fires(ValidatorType::NotNull, &leftover, Some(1.0)));
        assert!(!fires(ValidatorType::NotNull, &leftover, None));
    }
}
