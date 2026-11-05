// CEP:FILE: tools/cep_lint/tests/cases/mn_ok.cpp
// CEP:WHAT: Only sanctioned literals.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
constexpr int kOne = 1;
int a = kOne;
