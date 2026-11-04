// CEP:FILE: tools/cep_lint/tests/cases/stub_body.cpp
// CEP:WHAT: Self-test case: empty function bodies.
// CEP:WHY: Exercises CEP-LINT-STUB-BODY.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: offline.
// CEP:EVIDENCE: this file.

namespace demo {

// CEP:WHAT: Empty body without sanctioned status.
// CEP:WHY: Empty bodies are violations unless declared stubs.
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: this file.
auto hollow() -> void {
}

}  // namespace demo
