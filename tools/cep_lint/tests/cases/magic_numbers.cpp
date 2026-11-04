// CEP:FILE: tools/cep_lint/tests/cases/magic_numbers.cpp
// CEP:WHAT: Self-test case: numeric literal policy.
// CEP:WHY: Exercises CEP-LINT-MAGIC-NUMBER sanctioned and unsanctioned
//          contexts.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: offline.
// CEP:EVIDENCE: this file.

namespace demo {

constexpr long kLegal = 7;
enum class Face : long { One = 1, Two = 2 };

// CEP:WHAT: Mix sanctioned and unsanctioned literals.
// CEP:WHY: The rule allows constants, enums and static asserts only.
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: this file.
auto mixed(long x) -> long {
    const long a = x + 5;
    static_assert(sizeof(long) >= 8, "platform");
    long b = a * 3;
    b = b + 6;
    return b;
}

}  // namespace demo
