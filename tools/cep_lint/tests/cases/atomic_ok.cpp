// CEP:FILE: tools/cep_lint/tests/cases/atomic_ok.cpp
// CEP:WHAT: Right-sized fixture.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-1
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
int a = 0;
