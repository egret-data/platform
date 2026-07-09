//! The `notebook_cells.last_output` contract: a trimmed nbformat-style output
//! list, shared by the E-P2 recorder (writer) and the frontend renderer (reader).
//!
//! The `CellOutput` TYPE moved verbatim to the leaf contract crate
//! `egret-analytics-api` (E5c-1) so `egret-kernel`'s recorder can consume it
//! without depending on this crate; the trimming logic (`cap_outputs`) stays
//! here. Re-exported so existing `crate::output::CellOutput` paths keep working.
pub use egret_analytics_api::CellOutput;

/// Server-side persistence cap for a cell's outputs (before the JSONB write).
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// Keep outputs whose cumulative serialized size is within `MAX_OUTPUT_BYTES`;
/// append a `Truncated` marker if any were dropped.
pub fn cap_outputs(outputs: Vec<CellOutput>) -> Vec<CellOutput> {
    let mut kept: Vec<CellOutput> = Vec::new();
    let mut used = 0usize;
    let mut dropped = 0usize;
    for o in outputs {
        let sz = serde_json::to_vec(&o).map(|v| v.len()).unwrap_or(0);
        if used + sz <= MAX_OUTPUT_BYTES {
            used += sz;
            kept.push(o);
        } else {
            dropped += 1;
        }
    }
    if dropped > 0 {
        kept.push(CellOutput::Truncated { dropped });
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_truncates_oversized_and_marks() {
        let big = CellOutput::Stream {
            name: "stdout".into(),
            text: "x".repeat(MAX_OUTPUT_BYTES + 10),
        };
        let capped = cap_outputs(vec![big]);
        let bytes: usize = serde_json::to_vec(&capped).unwrap().len();
        assert!(bytes <= MAX_OUTPUT_BYTES + 256);
        assert!(matches!(capped.last(), Some(CellOutput::Truncated { .. })));
    }

    #[test]
    fn cap_keeps_small_outputs_verbatim() {
        let small = vec![CellOutput::Stream {
            name: "stdout".into(),
            text: "hi".into(),
        }];
        assert_eq!(cap_outputs(small.clone()).len(), 1);
    }

    #[test]
    fn serde_tag_shape_is_snake_case() {
        let v = serde_json::to_value(CellOutput::ExecuteResult {
            data: serde_json::json!({"text/plain":"1"}),
        })
        .unwrap();
        assert_eq!(v["type"], "execute_result");
    }
}
