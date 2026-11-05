// CEP:FILE: tools/cep_lint/tests/cases/hot_banned.cpp
// CEP:WHAT: CEP-0 file with a banned token.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
int make() {
    return 0;
}
int* g = new int{3};
