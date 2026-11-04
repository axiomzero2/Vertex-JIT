// CEP:FILE: tools/cep_lint/src/json.cpp
// CEP:WHAT: Single-pass JSON parser with deterministic member order and
//           bounded nesting.
// CEP:WHY: Configuration and manifest loading need predictable, auditable
//           parsing; failures must carry file, line and column (standard
//           50.4 bootstrap code 2).
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: Every malformed production returns nullopt and records the
//              position; the parser never throws and never loops.
// CEP:ASSUMES: UTF-8 without BOM; no trailing garbage tolerance.
// CEP:COST: O(n) single pass; depth bound 64.
// CEP:EVIDENCE: self-test manifest json cases.
// CEP:SECURITY: nesting depth bounded; string length bounded by input.
#include "json.hpp"

#include <cctype>
#include <cstdlib>

namespace cep {

/// Maximum object/array nesting (bootstrap guard).
constexpr int kMaxDepth = 64;

/// Length of the true literal.
constexpr std::size_t kTrueLen = 4;
/// Length of the false literal.
constexpr std::size_t kFalseLen = 5;
/// Length of the null literal.
constexpr std::size_t kNullLen = 4;
/// Hex digits of a \u escape.
constexpr std::size_t kUnicodeDigits = 4;
/// ASCII/Unicode boundary for single-byte emission.
constexpr unsigned long kAsciiMax = 0x80;

namespace {

/// Parser state.
struct Parser {
    /// Whole input.
    const std::string& in;
    /// Origin file name for diagnostics.
    const std::string& file;
    /// Byte offset.
    std::size_t pos = 0;
    /// Current line (1-based).
    long line = 1;
    /// Current column (1-based).
    long col = 1;
    /// Last error text.
    std::string error;
};

// CEP:WHAT: Advance one byte, tracking line and column.
// CEP:WHY: Diagnostics must point at the failing position.
// CEP:STATUS: complete
// CEP:FAILURE: none (position bookkeeping).
// CEP:ASSUMES: pos <= in.size().
// CEP:COST: O(1).
// CEP:EVIDENCE: json self-test cases.
auto advance(Parser& p) -> void {
    if (p.pos < p.in.size()) {
        if (p.in[p.pos] == '\n') {
            ++p.line;
            p.col = 1;
        } else {
            ++p.col;
        }
        ++p.pos;
    }
}

// CEP:WHAT: Skip ASCII whitespace.
// CEP:WHY: JSON tokens may be separated by any whitespace run.
// CEP:STATUS: complete
// CEP:FAILURE: none.
// CEP:ASSUMES: ASCII space/tab/newline/carriage-return.
// CEP:COST: O(gap).
// CEP:EVIDENCE: json self-test cases.
auto skip_ws(Parser& p) -> void {
    while (p.pos < p.in.size() && std::isspace(static_cast<unsigned char>(p.in[p.pos])) != 0) {
        advance(p);
    }
}

// CEP:WHAT: Record an error at the current position.
// CEP:WHY: The first error wins; later recovery would mislead.
// CEP:STATUS: complete
// CEP:FAILURE: none (diagnostic bookkeeping).
// CEP:ASSUMES: file name non-empty.
// CEP:COST: O(1) amortized (string append).
// CEP:EVIDENCE: json self-test cases.
auto fail(Parser& p, const std::string& what) -> void {
    if (p.error.empty()) {
        p.error = p.file + ":" + std::to_string(p.line) + ":" + std::to_string(p.col) + ": "
                  + what;
    }
}

// CEP:WHAT: Expect one literal byte.
// CEP:WHY: Structure productions share the single-byte lookahead.
// CEP:STATUS: complete
// CEP:FAILURE: false with a recorded diagnostic when the byte differs.
// CEP:ASSUMES: none.
// CEP:COST: O(1).
// CEP:EVIDENCE: json self-test cases.
auto expect(Parser& p, char c) -> bool {
    if (p.pos < p.in.size() && p.in[p.pos] == c) {
        advance(p);
        return true;
    }
    fail(p, std::string("expected '") + c + "'");
    return false;
}

// CEP:WHAT: Parse a string literal with escape handling.
// CEP:WHY: Keys and string values share one production; \u escapes are
//          emitted as ASCII when possible and kept verbatim otherwise.
// CEP:STATUS: complete
// CEP:FAILURE: false on unterminated or invalid escapes.
// CEP:ASSUMES: input bytes are single-byte characters outside escapes.
// CEP:COST: O(length).
// CEP:EVIDENCE: json self-test cases.
auto parse_string(Parser& p, std::string& out) -> bool {
    if (!expect(p, '"')) {
        return false;
    }
    out.clear();
    while (p.pos < p.in.size()) {
        const char c = p.in[p.pos];
        if (c == '"') {
            advance(p);
            return true;
        }
        if (c == '\\') {
            advance(p);
            if (p.pos >= p.in.size()) {
                break;
            }
            const char e = p.in[p.pos];
            switch (e) {
                case '"': out.push_back('"'); break;
                case '\\': out.push_back('\\'); break;
                case '/': out.push_back('/'); break;
                case 'b': out.push_back('\b'); break;
                case 'f': out.push_back('\f'); break;
                case 'n': out.push_back('\n'); break;
                case 'r': out.push_back('\r'); break;
                case 't': out.push_back('\t'); break;
                case 'u': {
                    if (p.pos + kUnicodeDigits < p.in.size()) {
                        const std::string hex = p.in.substr(p.pos + 1, kUnicodeDigits);
                        const unsigned long cp = std::strtoul(hex.c_str(), nullptr, 16);
                        if (cp < kAsciiMax && cp != 0) {
                            out.push_back(static_cast<char>(cp));
                        } else {
                            out.append("\\u").append(hex);
                        }
                        for (std::size_t k = 0; k < kUnicodeDigits; ++k) {
                            advance(p);
                        }
                    } else {
                        fail(p, "truncated unicode escape");
                        return false;
                    }
                    break;
                }
                default:
                    fail(p, "invalid escape");
                    return false;
            }
            advance(p);
            continue;
        }
        out.push_back(c);
        advance(p);
    }
    fail(p, "unterminated string");
    return false;
}

// CEP:WHAT: Parse a number literal.
// CEP:WHY: Numbers appear as severities, thresholds and counts.
// CEP:STATUS: complete
// CEP:FAILURE: false when the run holds no digits.
// CEP:ASSUMES: the literal fits double precision.
// CEP:COST: O(length).
// CEP:EVIDENCE: json self-test cases.
auto parse_number(Parser& p, double& out) -> bool {
    const std::size_t begin = p.pos;
    if (p.pos < p.in.size() && (p.in[p.pos] == '-' || p.in[p.pos] == '+')) {
        advance(p);
    }
    bool digits = false;
    while (p.pos < p.in.size()) {
        const char c = p.in[p.pos];
        const bool num_char = std::isdigit(static_cast<unsigned char>(c)) != 0 || c == '.'
                              || c == 'e' || c == 'E' || c == '+' || c == '-';
        if (!num_char) {
            break;
        }
        if (std::isdigit(static_cast<unsigned char>(c)) != 0) {
            digits = true;
        }
        advance(p);
    }
    if (!digits) {
        fail(p, "invalid number");
        return false;
    }
    out = std::strtod(p.in.substr(begin, p.pos - begin).c_str(), nullptr);
    return true;
}

// CEP:WHAT: Parse one JSON value; depth-bounded.
// CEP:WHY: The shared dispatch for all productions.
// CEP:STATUS: complete
// CEP:FAILURE: false past the depth bound or on malformed input.
// CEP:ASSUMES: depth < kMaxDepth at the top level.
// CEP:COST: O(production).
// CEP:EVIDENCE: json self-test cases.
auto parse_value(Parser& p, Json& out, int depth) -> bool;

// CEP:WHAT: Parse a JSON object in document order.
// CEP:WHY: Deterministic member order makes config iteration reproducible.
// CEP:STATUS: complete
// CEP:FAILURE: false on malformed structure.
// CEP:ASSUMES: depth bound holds.
// CEP:COST: O(members).
// CEP:EVIDENCE: json self-test cases.
auto parse_object(Parser& p, Json& out, int depth) -> bool {
    out.kind = Json::Kind::Object;
    if (!expect(p, '{')) {
        return false;
    }
    skip_ws(p);
    if (p.pos < p.in.size() && p.in[p.pos] == '}') {
        advance(p);
        return true;
    }
    while (p.pos < p.in.size()) {
        skip_ws(p);
        std::string key;
        if (!parse_string(p, key)) {
            return false;
        }
        skip_ws(p);
        if (!expect(p, ':')) {
            return false;
        }
        skip_ws(p);
        Json value;
        if (!parse_value(p, value, depth + 1)) {
            return false;
        }
        out.members.emplace_back(std::move(key), std::move(value));
        skip_ws(p);
        if (p.pos < p.in.size() && p.in[p.pos] == ',') {
            advance(p);
            continue;
        }
        if (p.pos < p.in.size() && p.in[p.pos] == '}') {
            advance(p);
            return true;
        }
        fail(p, "expected ',' or '}'");
        return false;
    }
    fail(p, "unterminated object");
    return false;
}

// CEP:WHAT: Parse a JSON array.
// CEP:WHY: Arrays carry field lists, token lists and expectations.
// CEP:STATUS: complete
// CEP:FAILURE: false on malformed structure.
// CEP:ASSUMES: depth bound holds.
// CEP:COST: O(items).
// CEP:EVIDENCE: json self-test cases.
auto parse_array(Parser& p, Json& out, int depth) -> bool {
    out.kind = Json::Kind::Array;
    if (!expect(p, '[')) {
        return false;
    }
    skip_ws(p);
    if (p.pos < p.in.size() && p.in[p.pos] == ']') {
        advance(p);
        return true;
    }
    while (p.pos < p.in.size()) {
        skip_ws(p);
        Json value;
        if (!parse_value(p, value, depth + 1)) {
            return false;
        }
        out.items.push_back(std::move(value));
        skip_ws(p);
        if (p.pos < p.in.size() && p.in[p.pos] == ',') {
            advance(p);
            continue;
        }
        if (p.pos < p.in.size() && p.in[p.pos] == ']') {
            advance(p);
            return true;
        }
        fail(p, "expected ',' or ']'");
        return false;
    }
    fail(p, "unterminated array");
    return false;
}

// CEP:WHAT: Parse one JSON value; depth-bounded dispatch.
// CEP:WHY: Shared dispatch for objects, arrays, strings, literals, numbers.
// CEP:STATUS: complete
// CEP:FAILURE: false past the depth bound or on malformed input.
// CEP:ASSUMES: depth < kMaxDepth at the top level.
// CEP:COST: O(production).
// CEP:EVIDENCE: json self-test cases.
auto parse_value(Parser& p, Json& out, int depth) -> bool {
    if (depth > kMaxDepth) {
        fail(p, "nesting too deep");
        return false;
    }
    skip_ws(p);
    if (p.pos >= p.in.size()) {
        fail(p, "unexpected end of input");
        return false;
    }
    const char c = p.in[p.pos];
    if (c == '{') {
        return parse_object(p, out, depth);
    }
    if (c == '[') {
        return parse_array(p, out, depth);
    }
    if (c == '"') {
        out.kind = Json::Kind::String;
        return parse_string(p, out.text);
    }
    if (c == 't' && p.in.compare(p.pos, kTrueLen, "true") == 0) {
        out.kind = Json::Kind::Bool;
        out.flag = true;
        for (std::size_t k = 0; k < kTrueLen; ++k) {
            advance(p);
        }
        return true;
    }
    if (c == 'f' && p.in.compare(p.pos, kFalseLen, "false") == 0) {
        out.kind = Json::Kind::Bool;
        out.flag = false;
        for (std::size_t k = 0; k < kFalseLen; ++k) {
            advance(p);
        }
        return true;
    }
    if (c == 'n' && p.in.compare(p.pos, kNullLen, "null") == 0) {
        out.kind = Json::Kind::Null;
        for (std::size_t k = 0; k < kNullLen; ++k) {
            advance(p);
        }
        return true;
    }
    out.kind = Json::Kind::Number;
    return parse_number(p, out.number);
}

}  // namespace

// CEP:WHAT: Member lookup; nullptr when absent.
// CEP:WHY: Callers distinguish absent from empty without exceptions.
// CEP:STATUS: complete
// CEP:FAILURE: nullptr when the key is absent.
// CEP:ASSUMES: object kind when members are expected.
// CEP:COST: O(members).
// CEP:EVIDENCE: json self-test cases.
auto Json::find(const std::string& key) const -> const Json* {
    for (const auto& kv : members) {
        if (kv.first == key) {
            return &kv.second;
        }
    }
    return nullptr;
}

// CEP:WHAT: String member lookup; empty when absent or not a string.
// CEP:WHY: Rule parameters are predominantly strings.
// CEP:STATUS: complete
// CEP:FAILURE: empty result for absent or mistyped members.
// CEP:ASSUMES: none.
// CEP:COST: O(members).
// CEP:EVIDENCE: json self-test cases.
auto Json::str(const std::string& key) const -> std::string {
    const Json* v = find(key);
    return (v != nullptr && v->is_string()) ? v->text : std::string();
}

// CEP:WHAT: Integer member lookup; fallback when absent or not numeric.
// CEP:WHY: Thresholds are configured as numbers.
// CEP:STATUS: complete
// CEP:FAILURE: fallback value for absent or mistyped members.
// CEP:ASSUMES: value fits long.
// CEP:COST: O(members).
// CEP:EVIDENCE: json self-test cases.
auto Json::integer(const std::string& key, long fallback) const -> long {
    const Json* v = find(key);
    return (v != nullptr && v->is_number()) ? static_cast<long>(v->number) : fallback;
}

// CEP:WHAT: Bool member lookup; fallback when absent.
// CEP:WHY: Case sensitivity and icase flags are configured booleans.
// CEP:STATUS: complete
// CEP:FAILURE: fallback value for absent or mistyped members.
// CEP:ASSUMES: none.
// CEP:COST: O(members).
// CEP:EVIDENCE: json self-test cases.
auto Json::boolean(const std::string& key, bool fallback) const -> bool {
    const Json* v = find(key);
    return (v != nullptr && v->is_bool()) ? v->flag : fallback;
}

// CEP:WHAT: String-array member lookup.
// CEP:WHY: Field and class lists are string arrays.
// CEP:STATUS: complete
// CEP:FAILURE: empty result for absent or mistyped members; non-string
//              items are skipped.
// CEP:ASSUMES: none.
// CEP:COST: O(items).
// CEP:EVIDENCE: json self-test cases.
auto Json::str_array(const std::string& key) const -> std::vector<std::string> {
    std::vector<std::string> out;
    const Json* v = find(key);
    if (v == nullptr || !v->is_array()) {
        return out;
    }
    for (const Json& item : v->items) {
        if (item.is_string()) {
            out.push_back(item.text);
        }
    }
    return out;
}

// CEP:WHAT: Number-array member lookup.
// CEP:WHY: fail_on severities and allowed numeric literals are arrays.
// CEP:STATUS: complete
// CEP:FAILURE: empty result for absent or mistyped members.
// CEP:ASSUMES: values fit long.
// CEP:COST: O(items).
// CEP:EVIDENCE: json self-test cases.
auto Json::int_array(const std::string& key) const -> std::vector<long> {
    std::vector<long> out;
    const Json* v = find(key);
    if (v == nullptr || !v->is_array()) {
        return out;
    }
    for (const Json& item : v->items) {
        if (item.is_number()) {
            out.push_back(static_cast<long>(item.number));
        }
    }
    return out;
}

/// Diagnostic text of the last parse failure (file:line:column: detail).
namespace {
std::string g_json_error;
}  // namespace

// CEP:WHAT: Parse a JSON document; nullopt with a diagnostic on failure.
// CEP:WHY: Entry point for configuration and manifest loading.
// CEP:STATUS: complete
// CEP:FAILURE: nullopt with json_last_error set; trailing content rejects.
// CEP:ASSUMES: input is one complete JSON document.
// CEP:COST: O(input).
// CEP:EVIDENCE: json self-test cases.
auto json_parse(const std::string& input, const std::string& file) -> std::optional<Json> {
    Parser p{input, file, 0, 1, 1, {}};
    Json out;
    if (!parse_value(p, out, 0)) {
        g_json_error = p.error.empty() ? (file + ": parse failed") : p.error;
        return std::nullopt;
    }
    skip_ws(p);
    if (p.pos != input.size()) {
        g_json_error = file + ":" + std::to_string(p.line) + ":" + std::to_string(p.col)
                       + ": trailing content after document";
        return std::nullopt;
    }
    return out;
}

// CEP:WHAT: Diagnostic text of the last parse failure.
// CEP:WHY: Bootstrap reporting surfaces the parse position (50.4 code 2).
// CEP:STATUS: complete
// CEP:FAILURE: none.
// CEP:ASSUMES: single-threaded use.
// CEP:COST: O(1).
// CEP:EVIDENCE: json self-test cases.
auto json_last_error() -> const std::string& {
    return g_json_error;
}

}  // namespace cep
