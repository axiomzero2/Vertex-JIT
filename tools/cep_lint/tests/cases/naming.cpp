// CEP:FILE: tools/cep_lint/tests/cases/naming.cpp
// CEP:WHAT: Self-test case: type and function naming.
// CEP:WHY: Exercises CEP-LINT-NAMING-TYPE and CEP-LINT-NAMING-FUNC.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: offline.
// CEP:EVIDENCE: this file.

namespace demo {

struct bad_type_name {
    long value;
};

class Also_Bad { };

// CEP:WHAT: A badly named function.
// CEP:WHY: Function names must be snake_case.
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: this file.
auto BadlyNamed(long x) -> long {
    const long a = x + 1;
    return a;
}

}  // namespace demo
