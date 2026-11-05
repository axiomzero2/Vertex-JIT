// CEP:FILE: tools/cep_lint/tests/cases/mn_bad.cpp
// CEP:WHAT: An unsanctioned literal in a statement.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
int a = 0;
int b = a + 7;
