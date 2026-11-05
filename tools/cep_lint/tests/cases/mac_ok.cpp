// CEP:FILE: tools/cep_lint/tests/cases/mac_ok.cpp
// CEP:WHAT: CEP_-prefixed macro.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
#define CEP_FIXTURE_LIMIT 8
int a = CEP_FIXTURE_LIMIT;
