// CEP:FILE: tools/cep_lint/tests/cases/rs_macro_bad.rs
// CEP:WHAT: macro_rules! without the CEP_ prefix.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
macro_rules! fixture_inc {
    ($x:expr) => { $x + 1 };
}
