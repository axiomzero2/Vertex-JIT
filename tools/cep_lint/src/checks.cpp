// CEP:FILE: tools/cep_lint/src/checks.cpp
// CEP:WHAT: The nine check primitives, lint_file, doc_rule_coverage and
//           the self-test runner.
// CEP:WHY: Standard 50: one primitive per check kind; all thresholds,
//           severities, patterns and messages come from the configuration.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: Invalid configured regexes fail bootstrap (BOOT-8); scan
//              errors surface through err and abort the run (exit 2).
// CEP:ASSUMES: ECMAScript regex grammar in configuration patterns.
// CEP:COST: O(bytes x rules) worst case; bounded per file.
// CEP:EVIDENCE: tests/manifest.json scenarios with exact counts.
// CEP:SECURITY: reads only the paths handed to it.
#include "lint.hpp"

#include <algorithm>
#include <cctype>
#include <cstdio>
#include <regex>
#include <set>
#include <sstream>

#include "reader.hpp"

namespace cep {

/// Default max_start_line for the file header rule.
constexpr long kDefaultMaxStartLine = 20;
/// Default min_body_statements for the function block rule.
constexpr long kDefaultMinStatements = 2;
/// Default statement-context window for numeric literals.
constexpr long kDefaultContextWindow = 12;
/// The class field marker (repeated for banned-token echo).
constexpr const char* kClassField = "CEP:CLASS:";
/// Length of the :: qualifier separator.
constexpr std::size_t kQualifierSepLen = 2;
/// Subscript of the function-like group in the define regex.
constexpr std::size_t kGroupFunctionLike = 2;

namespace {

// CEP:WHAT: Append one finding.
// CEP:WHY: Findings carry file, 1-based line, rule, severity and message.
// CEP:STATUS: complete
// CEP:FAILURE: none (append).
// CEP:ASSUMES: line is 0-based and converted here.
// CEP:COST: O(1) amortized.
// CEP:EVIDENCE: all self-test scenarios.
auto add(std::vector<Finding>& out, const std::string& file, long line, const std::string& rule,
         long severity, const std::string& message) -> void {
    Finding f;
    f.file = file;
    f.line = line + 1;
    f.rule = rule;
    f.severity = severity;
    f.message = message;
    out.push_back(std::move(f));
}

// CEP:WHAT: Compile a configured regex; BOOT-8 on failure.
// CEP:WHY: Pattern compilation is centralized for coded diagnostics.
// CEP:STATUS: complete
// CEP:FAILURE: false with a BOOT-8 error naming the rule and pattern.
// CEP:ASSUMES: ECMAScript grammar.
// CEP:COST: O(pattern).
// CEP:EVIDENCE: bootstrap self-test scenarios.
auto compile_re(const std::string& pattern, const std::string& rule, std::regex& out,
                std::string& err) -> bool {
    try {
        out.assign(pattern);
        return true;
    } catch (const std::regex_error&) {
        err = "CEP-LINT-BOOT-8: invalid regex in rule " + rule + ": " + pattern;
        return false;
    }
}

/// One comment block: a full-line run, or a single inline segment.
struct Block {
    /// First line (0-based).
    long start = 0;
    /// Last line (0-based).
    long end = 0;
    /// Joined comment text.
    std::string text;
};

// CEP:WHAT: Collect comment blocks (full-line runs merged).
// CEP:WHY: Header/status/stub checks operate on blocks, not lines.
// CEP:STATUS: complete
// CEP:FAILURE: none.
// CEP:ASSUMES: inline segments act as one-line blocks.
// CEP:COST: O(comments).
// CEP:EVIDENCE: self-test header scenarios.
auto collect_blocks(const SourceView& v) -> std::vector<Block> {
    std::vector<Block> out;
    long run_start = -1;
    long run_end = -1;
    std::string run_text;
    auto close_run = [&]() {
        if (run_start >= 0) {
            Block b;
            b.start = run_start;
            b.end = run_end;
            b.text = run_text;
            out.push_back(std::move(b));
            run_start = -1;
            run_text.clear();
        }
    };
    for (std::size_t i = 0; i < v.comments.size(); ++i) {
        const bool full = is_comment_or_blank(v, i) && !v.comments[i].empty();
        if (full) {
            if (run_start < 0) {
                run_start = static_cast<long>(i);
                run_text.clear();
            }
            run_end = static_cast<long>(i);
            for (const std::string& c : v.comments[i]) {
                if (!run_text.empty()) {
                    run_text.push_back('\n');
                }
                run_text += c;
            }
            continue;
        }
        close_run();
        for (const std::string& c : v.comments[i]) {
            Block b;
            b.start = static_cast<long>(i);
            b.end = static_cast<long>(i);
            b.text = c;
            out.push_back(std::move(b));
        }
    }
    close_run();
    return out;
}

// CEP:WHAT: Primitive comment_block_schema (header/status/stub modes).
// CEP:WHY: FILE-HEADER, STATUS-VALUE and STUB-DECL share one primitive.
// CEP:STATUS: complete
// CEP:FAILURE: findings name the missing or unsanctioned item.
// CEP:ASSUMES: parameters per .cep/cep_lint.json.
// CEP:COST: O(blocks).
// CEP:EVIDENCE: header self-test scenarios.
auto check_comment_block_schema(const Rule& r, const SourceView& v, std::vector<Finding>& out)
    -> void {
    const std::vector<Block> blocks = collect_blocks(v);
    if (r.mode == "file_header") {
        const long max_start = r.params.integer("max_start_line", kDefaultMaxStartLine);
        const std::vector<std::string> required = r.params.str_array("required_fields");
        const std::string class_field = r.params.str("class_field");
        const std::vector<std::string> allowed = r.params.str_array("allowed_classes");
        std::size_t idx = 0;
        while (idx < v.comments.size() && v.comments[idx].empty()) {
            ++idx;
        }
        if (idx >= v.comments.size() || !is_comment_or_blank(v, idx)
            || static_cast<long>(idx) >= max_start) {
            add(out, v.path, 0, r.key, r.severity, "missing CEP file header block");
            return;
        }
        std::string text;
        std::size_t end = idx;
        while (end < v.comments.size() && is_comment_or_blank(v, end)
               && !v.comments[end].empty()) {
            for (const std::string& c : v.comments[end]) {
                if (!text.empty()) {
                    text.push_back('\n');
                }
                text += c;
            }
            ++end;
        }
        for (const std::string& field : required) {
            if (!has_field(text, field)) {
                add(out, v.path, static_cast<long>(idx), r.key, r.severity,
                    "file header missing " + field);
            }
        }
        if (!class_field.empty()) {
            const std::string cls = field_value(text, class_field);
            bool ok = false;
            for (const std::string& a : allowed) {
                if (cls.rfind(a, 0) == 0) {
                    ok = true;
                }
            }
            if (!ok) {
                add(out, v.path, static_cast<long>(idx), r.key, r.severity,
                    "declared class is not an allowed conformance class");
            }
        }
        return;
    }
    if (r.mode == "status_value") {
        const std::string field = r.params.str("status_field");
        const std::vector<std::string> allowed = r.params.str_array("allowed_values");
        for (const Block& b : blocks) {
            const std::string val = field_value(b.text, field);
            if (val.empty()) {
                continue;
            }
            bool ok = false;
            for (const std::string& a : allowed) {
                if (val == a) {
                    ok = true;
                }
            }
            if (!ok) {
                add(out, v.path, b.start, r.key, r.severity,
                    "status value is not sanctioned");
            }
        }
        return;
    }
    if (r.mode == "stub_requirements") {
        const std::string field = r.params.str("status_field");
        const std::vector<std::string> statuses = r.params.str_array("statuses");
        const std::vector<std::string> required = r.params.str_array("required_fields");
        for (const Block& b : blocks) {
            const std::string val = field_value(b.text, field);
            bool is_stub = false;
            for (const std::string& s : statuses) {
                if (val == s) {
                    is_stub = true;
                }
            }
            if (!is_stub) {
                continue;
            }
            for (const std::string& f : required) {
                if (!has_field(b.text, f)) {
                    add(out, v.path, b.start, r.key, r.severity,
                        "stub-like block missing " + f);
                }
            }
        }
    }
}

// CEP:WHAT: Primitive function_policy (block/stub_body/constant_return/naming).
// CEP:WHY: FUNC-BLOCK, STUB-BODY, CONSTANT-RETURN and NAMING-FUNC share
//          the function extraction.
// CEP:STATUS: complete
// CEP:FAILURE: findings name the function and the missing item.
// CEP:ASSUMES: extractor limits documented in standard 50.3.
// CEP:COST: O(functions).
// CEP:EVIDENCE: function self-test scenarios.
auto check_function_policy(const Rule& r, const SourceView& v, std::vector<Finding>& out)
    -> void {
    const std::vector<FuncDef> funcs = extract_functions(v);
    if (r.mode == "block") {
        const std::vector<std::string> required = r.params.str_array("required_fields");
        const long min_stmt = r.params.integer("min_body_statements", kDefaultMinStatements);
        const std::string gap = r.params.str("gap_pattern");
        std::regex gap_re;
        std::string err;
        if (!gap.empty() && !compile_re(gap, r.key, gap_re, err)) {
            std::fprintf(stderr, "%s\n", err.c_str());
            return;
        }
        for (const FuncDef& f : funcs) {
            if (f.statements < min_stmt) {
                continue;
            }
            // Walk up through the contiguous comment region above the
            // signature (the CEP block and any doc lines form one region),
            // over gap lines, and over at most one blank line.
            long probe = f.header_line - 1;
            while (probe >= 0) {
                const std::string& cl = v.code[static_cast<std::size_t>(probe)];
                if (is_comment_or_blank(v, static_cast<std::size_t>(probe))) {
                    if (!v.comments[static_cast<std::size_t>(probe)].empty()) {
                        --probe;
                        continue;
                    }
                    break;
                }
                if (std::regex_search(cl, gap_re)) {
                    --probe;
                    continue;
                }
                break;
            }
            // The comment region spans (probe, header_line); gather it
            // directly so every doc line inside it counts toward the
            // field requirements.
            std::string text;
            long block_start = probe + 1;
            for (long ln = block_start; ln < f.header_line; ++ln) {
                for (const std::string& c : v.comments[static_cast<std::size_t>(ln)]) {
                    text += c;
                    text.push_back('\n');
                }
            }
            if (text.empty()) {
                add(out, v.path, f.header_line, r.key, r.severity,
                    "function '" + f.name + "' lacks a CEP comment block");
                continue;
            }
            for (const std::string& field : required) {
                if (!has_field(text, field)) {
                    add(out, v.path, block_start, r.key, r.severity,
                        "function block for '" + f.name + "' missing " + field);
                }
            }
        }
        return;
    }
    if (r.mode == "stub_body") {
        const std::string field = r.params.str("suppress_status_field");
        const std::vector<std::string> statuses = r.params.str_array("suppress_statuses");
        for (const FuncDef& f : funcs) {
            if (!f.empty) {
                continue;
            }
            long block_start = 0;
            std::string text;
            const bool have = block_above(v, f.header_line, block_start, text);
            bool suppressed = false;
            if (have) {
                const std::string val = field_value(text, field);
                for (const std::string& s : statuses) {
                    if (val == s) {
                        suppressed = true;
                    }
                }
            }
            if (!suppressed) {
                add(out, v.path, f.header_line, r.key, r.severity,
                    "function '" + f.name + "' has an empty body");
            }
        }
        return;
    }
    if (r.mode == "constant_return") {
        const std::string pattern = r.params.str("pattern");
        std::regex re;
        std::string err;
        if (!compile_re(pattern, r.key, re, err)) {
            std::fprintf(stderr, "%s\n", err.c_str());
            return;
        }
        for (const FuncDef& f : funcs) {
            if (f.statements != 1) {
                continue;
            }
            std::string body;
            for (long ln = f.body_line; ln <= f.end_line && ln < static_cast<long>(v.code.size());
                 ++ln) {
                body += v.code[static_cast<std::size_t>(ln)];
                body.push_back(' ');
            }
            if (std::regex_search(body, re)) {
                add(out, v.path, f.header_line, r.key, r.severity,
                    "function '" + f.name + "' returns a constant");
            }
        }
        return;
    }
    if (r.mode == "naming") {
        const std::string name_re_s = r.params.str("name_pattern");
        std::regex re;
        std::string err;
        if (!compile_re(name_re_s, r.key, re, err)) {
            std::fprintf(stderr, "%s\n", err.c_str());
            return;
        }
        for (const FuncDef& f : funcs) {
            if (f.exempt_name || f.name.empty()) {
                continue;
            }
            const std::size_t qual = f.name.rfind("::");
            const std::string bare =
                (qual == std::string::npos) ? f.name : f.name.substr(qual + kQualifierSepLen);
            if (!std::regex_match(bare, re)) {
                add(out, v.path, f.header_line, r.key, r.severity,
                    "function name '" + bare + "' must be snake_case");
            }
        }
    }
}

// CEP:WHAT: Primitive comment_regex (line scoped).
// CEP:WHY: the owner-marker, placeholder-comment and commented-code
//           rules share it.
// CEP:STATUS: complete
// CEP:FAILURE: invalid patterns abort bootstrap upstream of the scan.
// CEP:ASSUMES: suppression patterns are optional.
// CEP:COST: O(comment lines).
// CEP:EVIDENCE: comment self-test scenarios.
auto check_comment_regex(const Rule& r, const SourceView& v, std::vector<Finding>& out) -> void {
    const std::string pattern = r.params.str("pattern");
    const bool icase = r.params.boolean("icase", false);
    const std::string suppression = r.params.str("suppression_pattern");
    std::regex re;
    std::regex sup;
    std::string err;
    {
        const auto flags =
            icase ? (std::regex::ECMAScript | std::regex::icase) : std::regex::ECMAScript;
        try {
            re.assign(pattern, flags);
        } catch (const std::regex_error&) {
            std::fprintf(stderr, "CEP-LINT-BOOT-8: invalid regex in rule %s\n", r.key.c_str());
            return;
        }
    }
    if (!suppression.empty()) {
        try {
            sup.assign(suppression, std::regex::ECMAScript);
        } catch (const std::regex_error&) {
            std::fprintf(stderr, "CEP-LINT-BOOT-8: invalid regex in rule %s\n", r.key.c_str());
            return;
        }
    }
    for (std::size_t i = 0; i < v.comments.size(); ++i) {
        for (const std::string& c : v.comments[i]) {
            if (!suppression.empty() && std::regex_search(c, sup)) {
                continue;
            }
            if (std::regex_search(c, re)) {
                add(out, v.path, static_cast<long>(i), r.key, r.severity,
                    "comment violates the configured pattern");
            }
        }
    }
}

// CEP:WHAT: Primitive string_literal_regex.
// CEP:WHY: PLACEHOLDER-TEXT inspects literals without leaking them.
// CEP:STATUS: complete
// CEP:FAILURE: invalid patterns abort bootstrap upstream of the scan.
// CEP:ASSUMES: literals captured by the lexer.
// CEP:COST: O(literals).
// CEP:EVIDENCE: string self-test scenarios.
auto check_string_literal_regex(const Rule& r, const SourceView& v, std::vector<Finding>& out)
    -> void {
    const std::string pattern = r.params.str("pattern");
    const bool icase = r.params.boolean("icase", false);
    std::regex re;
    {
        const auto flags =
            icase ? (std::regex::ECMAScript | std::regex::icase) : std::regex::ECMAScript;
        try {
            re.assign(pattern, flags);
        } catch (const std::regex_error&) {
            std::fprintf(stderr, "CEP-LINT-BOOT-8: invalid regex in rule %s\n", r.key.c_str());
            return;
        }
    }
    for (std::size_t i = 0; i < v.strings.size(); ++i) {
        for (const std::string& s : v.strings[i]) {
            if (std::regex_search(s, re)) {
                add(out, v.path, static_cast<long>(i), r.key, r.severity,
                    "string literal carries unsanctioned marker text");
            }
        }
    }
}

// CEP:WHAT: Primitive numeric_literal_policy.
// CEP:WHY: MAGIC-NUMBER allows only sanctioned values in sanctioned
//          statement contexts, bounded to a twelve-line window.
// CEP:STATUS: complete
// CEP:FAILURE: findings name the literal; comparison is numeric.
// CEP:ASSUMES: leading-dot literals rely on review (documented limit).
// CEP:COST: O(literals x window).
// CEP:EVIDENCE: numeric self-test scenarios.
auto check_numeric_literal_policy(const Rule& r, const SourceView& v, std::vector<Finding>& out)
    -> void {
    const std::vector<std::string> allowed_raw = r.params.str_array("allowed_values");
    const std::string exempt_s = r.params.str("exempt_contexts");
    std::vector<double> allowed;
    for (const std::string& a : allowed_raw) {
        double d = 0.0;
        if (literal_value(a, d)) {
            allowed.push_back(d);
        }
    }
    std::regex exempt_re;
    std::string err;
    if (!exempt_s.empty() && !compile_re(exempt_s, r.key, exempt_re, err)) {
        std::fprintf(stderr, "%s\n", err.c_str());
        return;
    }
    static const std::regex num_re(
        "\\b(0[xX][0-9a-fA-F']+|[0-9][0-9']*(\\.[0-9]+)?([eE][+-]?[0-9]+)?)[uUlLfF]*");
    const long window = r.params.integer("context_window", kDefaultContextWindow);
    for (std::size_t i = 0; i < v.code.size(); ++i) {
        const std::string& line = v.code[i];
        if (line.find_first_not_of(" \t\r") == std::string::npos) {
            continue;
        }
        std::string ctx;
        long lo = static_cast<long>(i);
        long taken = 0;
        while (lo >= 0 && taken < window) {
            const std::string& l = v.code[static_cast<std::size_t>(lo)];
            ctx = l + " " + ctx;
            ++taken;
            if (l.find(';') != std::string::npos || l.find('{') != std::string::npos
                || l.find('}') != std::string::npos) {
                break;
            }
            --lo;
        }
        if (!exempt_s.empty() && std::regex_search(ctx, exempt_re)) {
            continue;
        }
        for (std::sregex_iterator it = std::sregex_iterator(line.begin(), line.end(), num_re);
             it != std::sregex_iterator(); ++it) {
            const std::string lit = (*it)[1].str();
            double d = 0.0;
            if (!literal_value(lit, d)) {
                continue;
            }
            bool ok = false;
            for (const double a : allowed) {
                if (d == a) {
                    ok = true;
                }
            }
            if (!ok) {
                add(out, v.path, static_cast<long>(i), r.key, r.severity,
                    "unsanctioned numeric literal");
            }
        }
    }
}

// CEP:WHAT: Primitive macro_prefix_policy.
// CEP:WHY: MACRO-PREFIX enforces the CEP_ prefix on object-like macros.
// CEP:STATUS: complete
// CEP:FAILURE: findings name the macro; function-like macros rely on
//              review (documented limit 50.3).
// CEP:ASSUMES: name and guard patterns from the configuration.
// CEP:COST: O(lines).
// CEP:EVIDENCE: macro self-test scenarios.
auto check_macro_prefix_policy(const Rule& r, const SourceView& v, std::vector<Finding>& out)
    -> void {
    const std::string name_pat = r.params.str("name_pattern");
    const std::string guard_pat = r.params.str("guard_pattern");
    std::regex name_re;
    std::regex guard_re;
    std::string err;
    if (!compile_re(name_pat, r.key, name_re, err) || !compile_re(guard_pat, r.key, guard_re, err)) {
        std::fprintf(stderr, "%s\n", err.c_str());
        return;
    }
    static const std::regex define_re("^\\s*#\\s*define\\s+([A-Za-z_][A-Za-z0-9_]*)\\s*(\\()?");
    for (std::size_t i = 0; i < v.code.size(); ++i) {
        std::smatch m;
        if (!std::regex_search(v.code[i], m, define_re)) {
            continue;
        }
        const std::string name = m[1].str();
        if (m[kGroupFunctionLike].matched) {
            continue;
        }
        if (std::regex_match(name, name_re) || std::regex_match(name, guard_re)) {
            continue;
        }
        add(out, v.path, static_cast<long>(i), r.key, r.severity,
            "object-like macro lacks the CEP_ prefix");
    }
}

// CEP:WHAT: Primitive banned_token_policy.
// CEP:WHY: HOT-BANNED exterminates tokens in class-0 files after
//          comment and string removal.
// CEP:STATUS: complete
// CEP:FAILURE: findings name the token and the declared class.
// CEP:ASSUMES: declared class captured by the lexer.
// CEP:COST: O(tokens x lines).
// CEP:EVIDENCE: banned-token self-test scenarios.
auto check_banned_token_policy(const Rule& r, const SourceView& v, std::vector<Finding>& out)
    -> void {
    const std::vector<std::string> classes = r.params.str_array("applies_to_classes");
    const std::string cls = v.declared_class;
    bool applies = false;
    for (const std::string& c : classes) {
        if (cls.rfind(c, 0) == 0) {
            applies = true;
        }
    }
    if (!applies) {
        return;
    }
    const std::vector<std::string> tokens = r.params.str_array("tokens");
    for (const std::string& tok : tokens) {
        static const std::regex esc_re("[.^$|()\\[\\]{}*+?\\\\]");
        const std::string quoted = std::regex_replace(tok, esc_re, "\\$0");
        const std::string word_pat = "(^|[^A-Za-z0-9_])" + quoted + "([^A-Za-z0-9_]|$)";
        std::regex word_re;
        std::string err;
        if (!compile_re(word_pat, r.key, word_re, err)) {
            std::fprintf(stderr, "%s\n", err.c_str());
            return;
        }
        for (std::size_t i = 0; i < v.code.size(); ++i) {
            if (std::regex_search(v.code[i], word_re)) {
                add(out, v.path, static_cast<long>(i), r.key, r.severity,
                    "banned token in class-0 code");
            }
        }
    }
}

// CEP:WHAT: Primitive flat_code_regex.
// CEP:WHY: EMPTY-CATCH, EMPTY-BRANCH and NAMING-TYPE are flat patterns
//          over the stripped code.
// CEP:STATUS: complete
// CEP:FAILURE: invalid patterns abort bootstrap upstream of the scan.
// CEP:ASSUMES: single-line patterns only (documented limit).
// CEP:COST: O(lines).
// CEP:EVIDENCE: flat self-test scenarios.
auto check_flat_code_regex(const Rule& r, const SourceView& v, std::vector<Finding>& out)
    -> void {
    const std::string pattern = r.params.str("pattern");
    std::regex re;
    std::string err;
    if (!compile_re(pattern, r.key, re, err)) {
        std::fprintf(stderr, "%s\n", err.c_str());
        return;
    }
    for (std::size_t i = 0; i < v.code.size(); ++i) {
        if (std::regex_search(v.code[i], re)) {
            add(out, v.path, static_cast<long>(i), r.key, r.severity,
                "code shape violates the configured pattern");
        }
    }
}

}  // namespace

// CEP:WHAT: Run every configured rule over one file; appends findings.
// CEP:WHY: The per-file driver: dispatch on the configured primitive.
// CEP:STATUS: complete
// CEP:FAILURE: unknown primitives abort with BOOT-6; unreadable files
//              with BOOT-1.
// CEP:ASSUMES: the configuration validated at load time.
// CEP:COST: O(rules x bytes).
// CEP:EVIDENCE: every self-test scenario routes through it.
auto lint_file(const Config& cfg, const std::string& path, std::vector<Finding>& out,
               std::string& err) -> bool {
    std::string text;
    if (!read_file(path, text)) {
        err = "CEP-LINT-BOOT-1: cannot read target " + path;
        return false;
    }
    SourceView v;
    lex_source(path, text, v);
    for (const Rule& r : cfg.rules) {
        if (r.check == "doc_rule_coverage") {
            continue;
        }
        if (r.check == "comment_block_schema") {
            check_comment_block_schema(r, v, out);
        } else if (r.check == "function_policy") {
            check_function_policy(r, v, out);
        } else if (r.check == "comment_regex") {
            check_comment_regex(r, v, out);
        } else if (r.check == "string_literal_regex") {
            check_string_literal_regex(r, v, out);
        } else if (r.check == "numeric_literal_policy") {
            check_numeric_literal_policy(r, v, out);
        } else if (r.check == "macro_prefix_policy") {
            check_macro_prefix_policy(r, v, out);
        } else if (r.check == "banned_token_policy") {
            check_banned_token_policy(r, v, out);
        } else if (r.check == "flat_code_regex") {
            check_flat_code_regex(r, v, out);
        } else {
            err = "CEP-LINT-BOOT-6: unknown check primitive '" + r.check + "' for rule " + r.key;
            return false;
        }
    }
    return true;
}

// CEP:WHAT: Rule-catalog coverage between the document and the configuration.
// CEP:WHY: DOC-SYNC enforces Law 8 mechanically: no stale documentation.
// CEP:STATUS: complete
// CEP:FAILURE: unreadable document is BOOT-1; drift becomes findings.
// CEP:ASSUMES: headings use the '### Rule KEY' form; version uses the
//              '**x.y**' emphasis form.
// CEP:COST: O(document).
// CEP:EVIDENCE: doc-sync self-test scenarios.
auto doc_rule_coverage(const Config& cfg, std::vector<Finding>& out, std::string& err) -> bool {
    std::string text;
    if (!read_file(cfg.doc_path, text)) {
        err = "CEP-LINT-BOOT-1: cannot read standard document " + cfg.doc_path;
        return false;
    }
    static const std::regex heading_re("###\\s+Rule\\s+(CEP-LINT-[A-Za-z0-9-]+)");
    static const std::regex version_re("Version:\\s*\\*\\*([0-9][0-9.]*)\\*\\*");
    std::set<std::string> documented;
    std::set<std::string> seen;
    std::string version;
    std::istringstream ss(text);
    std::string line;
    while (std::getline(ss, line)) {
        std::smatch m;
        if (std::regex_search(line, m, heading_re)) {
            const std::string key = m[1].str();
            if (seen.count(key) != 0) {
                add(out, cfg.doc_path, 0, "CEP-LINT-DOC-SYNC", 1,
                    "duplicate rule heading in the document");
            }
            seen.insert(key);
            documented.insert(key);
        }
        if (version.empty() && std::regex_search(line, m, version_re)) {
            version = m[1].str();
        }
    }
    std::set<std::string> configured;
    for (const Rule& r : cfg.rules) {
        configured.insert(r.key);
    }
    for (const std::string& key : configured) {
        if (documented.count(key) == 0) {
            add(out, cfg.doc_path, 0, "CEP-LINT-DOC-SYNC", 1,
                "configured rule has no '### Rule' section in the document: " + key);
        }
    }
    for (const std::string& key : documented) {
        if (configured.count(key) == 0) {
            add(out, cfg.doc_path, 0, "CEP-LINT-DOC-SYNC", 1,
                "documented rule is missing from the configuration: " + key);
        }
    }
    if (!version.empty() && version != cfg.standard_version) {
        add(out, cfg.doc_path, 0, "CEP-LINT-DOC-SYNC", 1,
            "document version differs from the configuration version");
    }
    return true;
}

// CEP:WHAT: Sort findings by file, line, rule, message.
// CEP:WHY: Determinism (6.5): no output depends on traversal order.
// CEP:STATUS: complete
// CEP:FAILURE: none.
// CEP:ASSUMES: strict weak ordering on the four keys.
// CEP:COST: O(n log n).
// CEP:EVIDENCE: CI runs are byte-identical across machines.
auto sort_findings(std::vector<Finding>& findings) -> void {
    std::sort(findings.begin(), findings.end(), [](const Finding& a, const Finding& b) {
        if (a.file != b.file) {
            return a.file < b.file;
        }
        if (a.line != b.line) {
            return a.line < b.line;
        }
        if (a.rule != b.rule) {
            return a.rule < b.rule;
        }
        return a.message < b.message;
    });
}

// CEP:WHAT: Run one self-test scenario; returns the scenario's findings.
// CEP:WHY: Scenarios overlay the base configuration and lint case files.
// CEP:STATUS: complete
// CEP:FAILURE: configuration or lint errors surface through err.
// CEP:ASSUMES: manifest paths are relative to the manifest directory.
// CEP:COST: O(scenario files x rules).
// CEP:EVIDENCE: the manifest itself.
auto run_scenario(const Json& base_cfg, const std::string& manifest_dir, const Json& scenario,
                  std::string& err) -> std::vector<Finding> {
    const Json* overlay = scenario.find("config");
    const Json cfg_json = (overlay != nullptr) ? json_merge(base_cfg, *overlay) : base_cfg;
    Config cfg;
    if (!config_from_json(cfg_json, "manifest-scenario", cfg, err)) {
        return {};
    }
    std::vector<Finding> findings;
    for (const std::string& rel : scenario.str_array("files")) {
        const std::string full = manifest_dir + rel;
        if (!lint_file(cfg, full, findings, err)) {
            return {};
        }
    }
    return findings;
}

// CEP:WHAT: Execute the whole self-test manifest; true when all match.
// CEP:WHY: The self-verification gate (50.6.3): exact per-rule counts.
// CEP:STATUS: complete
// CEP:FAILURE: unreadable or malformed manifests fail with BOOT codes.
// CEP:ASSUMES: every scenario names files and expectations.
// CEP:COST: O(scenarios x files x rules).
// CEP:EVIDENCE: CI runs --self-test on every change.
auto self_test(const Json& base_cfg, const std::string& manifest_path, std::string& report)
    -> bool {
    std::string text;
    if (!read_file(manifest_path, text)) {
        report = "CEP-LINT-BOOT-1: cannot read manifest " + manifest_path;
        return false;
    }
    std::optional<Json> m = json_parse(text, manifest_path);
    if (!m.has_value()) {
        report = "CEP-LINT-BOOT-2: " + json_last_error();
        return false;
    }
    const Json* scenarios = m->find("scenarios");
    if (scenarios == nullptr || !scenarios->is_array()) {
        report = "CEP-LINT-BOOT-4: manifest lacks a scenarios array";
        return false;
    }
    std::string dir;
    {
        const std::size_t slash = manifest_path.rfind('/');
        dir = (slash == std::string::npos) ? std::string() : manifest_path.substr(0, slash + 1);
    }
    std::size_t passed = 0;
    std::ostringstream report_ss;
    for (const Json& sc : scenarios->items) {
        const std::string name = sc.str("name");
        std::string err;
        std::vector<Finding> findings = run_scenario(base_cfg, dir, sc, err);
        if (!err.empty()) {
            report_ss << "FAIL " << name << ": " << err << "\n";
            report = report_ss.str();
            return false;
        }
        std::map<std::string, long> counts;
        for (const Finding& f : findings) {
            ++counts[f.rule];
        }
        bool ok = true;
        const Json* expect = sc.find("expect");
        if (expect == nullptr || !expect->is_array()) {
            ok = false;
            report_ss << "FAIL " << name << ": missing expect array\n";
        } else {
            for (const Json& e : expect->items) {
                const std::string rule = e.str("rule");
                const long want = e.integer("count", 0);
                const long got = counts.count(rule) != 0 ? counts[rule] : 0;
                if (got != want) {
                    ok = false;
                    report_ss << "FAIL " << name << ": " << rule << " expected " << want
                              << " got " << got << "\n";
                }
            }
        }
        if (ok) {
            ++passed;
            report_ss << "PASS " << name << "\n";
        }
    }
    report = report_ss.str();
    return passed == scenarios->items.size();
}

}  // namespace cep
