// CEP:FILE: tools/cep_lint/tests/cases/fb_ok.cpp
// CEP:WHAT: Function with a full CEP block.
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
int twice(int v) {
    return v + v;
}
