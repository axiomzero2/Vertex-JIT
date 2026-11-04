// CEP:FILE: tools/cep_lint/tests/cases/placeholder_text.cpp
// CEP:WHAT: Self-test case: marker text in string literals.
// CEP:WHY: Exercises CEP-LINT-PLACEHOLDER-TEXT.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: offline.
// CEP:EVIDENCE: this file.

namespace demo {

// CEP:WHAT: Return a marker message.
// CEP:WHY: Placeholders must fail loudly, not leak.
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: this file.
auto message() -> const char* {
    const char* leak = "feature not implemented";
    return leak;
}

}  // namespace demo
