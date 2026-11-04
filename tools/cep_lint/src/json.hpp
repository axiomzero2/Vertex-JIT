// CEP:FILE: tools/cep_lint/src/json.hpp
// CEP:WHAT: Minimal deterministic JSON value parser for the lint
//           configuration, the self-test manifest and evidence metadata.
// CEP:WHY: The linter is data-driven (standard 50: the engine contains
//          mechanism only); a dependency-free parser keeps the tool
//          buildable with one compiler invocation.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: parse errors return std::nullopt with file, line and
//              column; callers surface CEP-LINT-BOOT-2.
// CEP:ASSUMES: UTF-8 input without BOM; ASCII whitespace.
// CEP:COST: O(n) single pass.
// CEP:EVIDENCE: self-test scenarios exercise every production.
// CEP:SECURITY: nesting depth bounded; string length bounded by input.
#pragma once

#include <map>
#include <optional>
#include <string>
#include <vector>

namespace cep {

/// One JSON value: object, array, string, number, bool or null.
struct Json {
    /// Discriminator.
    enum class Kind { Object, Array, String, Number, Bool, Null };
    /// Value kind.
    Kind kind = Kind::Null;
    /// Object members in document order (deterministic iteration).
    std::vector<std::pair<std::string, Json>> members;
    /// Array elements.
    std::vector<Json> items;
    /// String payload.
    std::string text;
    /// Number payload.
    double number = 0.0;
    /// Bool payload.
    bool flag = false;

    /// True when the value is an object.
    [[nodiscard]] auto is_object() const -> bool { return kind == Kind::Object; }
    /// True when the value is an array.
    [[nodiscard]] auto is_array() const -> bool { return kind == Kind::Array; }
    /// True when the value is a string.
    [[nodiscard]] auto is_string() const -> bool { return kind == Kind::String; }
    /// True when the value is a number.
    [[nodiscard]] auto is_number() const -> bool { return kind == Kind::Number; }
    /// True when the value is a bool.
    [[nodiscard]] auto is_bool() const -> bool { return kind == Kind::Bool; }

    /// Member lookup; nullptr when absent.
    [[nodiscard]] auto find(const std::string& key) const -> const Json*;
    /// String member lookup; empty when absent or not a string.
    [[nodiscard]] auto str(const std::string& key) const -> std::string;
    /// Integer member lookup; fallback when absent or not numeric.
    [[nodiscard]] auto integer(const std::string& key, long fallback) const -> long;
    /// Bool member lookup; fallback when absent.
    [[nodiscard]] auto boolean(const std::string& key, bool fallback) const -> bool;
    /// String-array member lookup.
    [[nodiscard]] auto str_array(const std::string& key) const -> std::vector<std::string>;
    /// Number-array member lookup.
    [[nodiscard]] auto int_array(const std::string& key) const -> std::vector<long>;
};

/// Parse a JSON document; nullopt with a diagnostic on failure.
[[nodiscard]] auto json_parse(const std::string& input, const std::string& file)
    -> std::optional<Json>;

/// Diagnostic text of the last parse failure (file:line:column: detail).
[[nodiscard]] auto json_last_error() -> const std::string&;

}  // namespace cep
