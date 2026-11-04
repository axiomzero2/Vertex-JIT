// CEP:FILE: tools/cep_lint/tests/cases/empty_shapes.cpp
// CEP:WHAT: Self-test case: empty catch and branch bodies.
// CEP:WHY: Exercises CEP-LINT-EMPTY-CATCH and CEP-LINT-EMPTY-BRANCH.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: offline.
// CEP:EVIDENCE: this file.

namespace demo {

// CEP:WHAT: A function with empty control bodies.
// CEP:WHY: Empty bodies do nothing; the rules flag them.
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: this file.
auto shapes(int x) -> int {
    if (x > 0) {}
    try {
        x = x - 1;
    } catch (...) {}
    return x;
}

}  // namespace demo
