// CEP:FILE: tools/cep_lint/tests/cases/passes/one_pass.cpp
// CEP:WHAT: One registered pass under passes/.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
// CEP:PASS: fixture-fold
/// CEP:WHAT: fixture function
/// CEP:WHY: fixture
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: manifest
int run_fold(Graph& g) {
    g.touch();
    return 0;
}
