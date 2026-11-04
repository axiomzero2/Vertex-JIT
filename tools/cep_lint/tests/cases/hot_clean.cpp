// CEP:FILE: tools/cep_lint/tests/cases/hot_clean.cpp
// CEP:WHAT: Self-test case: class-0 file without banned tokens.
// CEP:WHY: Negative control for CEP-LINT-HOT-BANNED.
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: offline.
// CEP:EVIDENCE: this file.

namespace demo {

// CEP:WHAT: Hot function with plain arithmetic.
// CEP:WHY: Class zero allows allocation-free arithmetic.
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: this file.
auto hot(int x) -> int {
    const int a = x + 1;
    return a * 2;
}

}  // namespace demo
