// CEP:FILE: tools/cep_lint/tests/cases/cc_bad.cpp
// CEP:WHAT: Commented-out code line.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
// return v + v;
int a = 0;
