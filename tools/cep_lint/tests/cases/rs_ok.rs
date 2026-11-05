// CEP:FILE: tools/cep_lint/tests/cases/rs_ok.rs
// CEP:WHAT: Compliant Rust fixture.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
/// CEP:WHAT: Double the input.
/// CEP:WHY: fixture
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: manifest
pub fn twice(v: i64) -> i64 {
    v + v
}

/// CEP:WHAT: A sanctioned macro.
/// CEP:WHY: fixture
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: manifest
macro_rules! CEP_FIXTURE {
    ($x:expr) => { $x + 1 };
}
/// CEP:WHAT: Use the macro and the helper.
/// CEP:WHY: fixture
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: manifest
pub fn use_it(v: i64) -> i64 {
    let a = CEP_FIXTURE!(v);
    a + twice(v)
}
