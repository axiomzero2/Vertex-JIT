// CEP:FILE: tools/cep_lint/tests/cases/enum_class_bad.cpp
// CEP:WHAT: Self-test case: snake_case names behind enum class / enum struct
//           are still violations.
// CEP:WHY: The repaired NAMING-TYPE pattern must keep flagging non-PascalCase
//           names on every declaration form, including scoped enums.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: offline.
// CEP:EVIDENCE: this file.

namespace demo {

enum class kind_bad { One, Two };

enum struct other_bad { A, B };

enum plain_bad { C, D };

using bad_alias_t = long;

}  // namespace demo
