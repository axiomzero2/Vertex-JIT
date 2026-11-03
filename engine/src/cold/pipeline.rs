// CEP:FILE: engine/src/cold/pipeline.rs
// CEP:WHAT: The explicit, versioned pass pipeline descriptor (standard
//           15.2, 38.20) and its documentation entry points.
// CEP:WHY: Pass order is a semantic event; the pipeline is versioned and
//          every pass is listed with its tier mapping.
// CEP:CLASS: CEP-1
// CEP:STATUS: complete
// CEP:FAILURE: none; pure data.
// CEP:ASSUMES: none
// CEP:COST: zero at runtime.
// CEP:EVIDENCE: tests/ir_passes.rs pipeline tests.
// CEP:SECURITY: none

/// Pipeline version identifier (38.20: versioned pipelines).
pub const PIPELINE_ID: &str = "vertex-jit-opt-2026-11";

/// The canonical pass order, shared with cold::passes::PIPELINE_V1.
///
/// CEP:WHAT: Pipeline descriptor.
/// CEP:WHY: 38.20 requires pass order, versions and gates to be explicit.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: passes exist for every listed name (run_pass fails closed
///              otherwise).
/// CEP:COST: O(1).
/// CEP:EVIDENCE: pipeline tests.
/// CEP:SECURITY: none
#[must_use]
pub fn describe() -> String {
    format!(
        "pipeline: {PIPELINE_ID}\npasses: {}",
        crate::cold::passes::PIPELINE_V1.join(", ")
    )
}
