// CEP:FILE: tools/cep_lint/tests/cases/rs_lifetime_ok.rs
// CEP:WHAT: Lifetimes and raw strings lex cleanly.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
/// CEP:WHAT: Borrower fixture.
/// CEP:WHY: lifetimes must not swallow code
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: manifest
pub fn pick<'a>(a: &'a str, r: &'a str) -> &'a str {
    let raw = r"not\implemented";
    if a.len() > raw.len() { a } else { r }
}
