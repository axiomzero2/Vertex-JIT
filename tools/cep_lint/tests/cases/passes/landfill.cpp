// CEP:FILE: tools/cep_lint/tests/cases/passes/landfill.cpp
// CEP:WHAT: Two registered passes under passes/.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
// CEP:PASS: fixture-fold
// CEP:PASS: fixture-move
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
/// CEP:WHAT: fixture function
/// CEP:WHY: fixture
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: manifest
int run_move(Graph& g) {
    g.touch();
    return 0;
}
