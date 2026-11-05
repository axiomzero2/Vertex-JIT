// CEP:FILE: tools/cep_lint/tests/cases/rs_fn_block_missing.rs
// CEP:WHAT: Rust fn without a CEP block.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.

pub fn twice(v: i64) -> i64 {
    let a = v + v;
    a
}
