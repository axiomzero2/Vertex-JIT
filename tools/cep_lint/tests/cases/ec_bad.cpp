// CEP:FILE: tools/cep_lint/tests/cases/ec_bad.cpp
// CEP:WHAT: Empty catch.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-2
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
int guard() {
    int r = 0;
    try { r = 1; } catch (...) {}
    return r;
}
