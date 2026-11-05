// CEP:FILE: tools/cep_lint/tests/cases/mac_bad.cpp
// CEP:WHAT: Unprefixed object-like macro.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
#define FIXTURE_LIMIT 8
int a = FIXTURE_LIMIT;
