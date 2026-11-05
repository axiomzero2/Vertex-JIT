// CEP:FILE: tools/cep_lint/tests/cases/hot_clean.cpp
// CEP:WHAT: CEP-0 file with no banned tokens.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
/// CEP:WHAT: fixture function
/// CEP:WHY: fixture
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: manifest
int fast_path(int v) {
    return v + 1;
}
