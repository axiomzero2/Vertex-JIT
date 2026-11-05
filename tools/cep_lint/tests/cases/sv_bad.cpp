// CEP:FILE: tools/cep_lint/tests/cases/sv_bad.cpp
// CEP:WHAT: Unsanctioned status value.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
int separator = 0;
/// CEP:STATUS: someday
int a = 0;
