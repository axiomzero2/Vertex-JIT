// CEP:FILE: tools/cep_lint/tests/cases/constant_return.cpp
// CEP:WHAT: Self-test case: constant-return functions.
// CEP:WHY: Exercises CEP-LINT-CONSTANT-RETURN.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: offline.
// CEP:EVIDENCE: this file.

namespace demo {

// CEP:WHAT: Always true.
// CEP:WHY: The body computes nothing from inputs.
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: this file.
auto always_true() -> bool {
    return true;
}

}  // namespace demo
