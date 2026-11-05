// CEP:FILE: tools/cep_lint/tests/cases/eb_ok.cpp
// CEP:WHAT: Branch with a body.
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
int pick(bool c) {
    if (c) { return 1; }
    return 2;
}
