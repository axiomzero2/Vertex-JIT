// CEP:FILE: tools/cep_lint/src/lint.hpp
// CEP:WHAT: The lint engine: comment/string-aware C++ reading, the nine
//           check primitives, doc-rule coverage and the self-test runner.
// CEP:WHY: Standard 50: mechanism lives here, every pattern, threshold,
//           severity and message lives in the configuration.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: findings carry file, line, rule and message; bootstrap
//              failures surface coded CEP-LINT-BOOT-<n> errors.
// CEP:ASSUMES: UTF-8 C++ sources; ECMAScript-grammar config regexes.
// CEP:COST: O(file bytes) per file plus O(rules) per file.
// CEP:EVIDENCE: tools/cep_lint/tests/manifest.json self-test scenarios.
// CEP:SECURITY: only paths given on the command line or the manifest are
//               read; regex compilation is bounded by input size.
#pragma once

#include <map>
#include <string>
#include <vector>

#include <string>
#include <vector>

#include "json.hpp"
#include "reader.hpp"

namespace cep {

/// Default fail_on severities (standard 50.1).
constexpr long kDefaultFailSeverity0 = 0;
constexpr long kDefaultFailSeverity1 = 1;
/// Sentinel severity from the configuration schema.
constexpr long kInvalidSeverity = -1;
/// Severity range (standard 34.2 violation classes).
constexpr long kMinSeverity = 0;
constexpr long kMaxSeverity = 3;

/// One lint finding.
struct Finding {
    /// File the finding came from.
    std::string file;
    /// 1-based line.
    long line = 0;
    /// Rule key as configured under rules.
    std::string rule;
    /// Severity 0..3 from the configuration.
    long severity = 0;
    /// Human message; never contains the matched text beyond the line.
    std::string message;
};

/// A loaded rule: configuration data plus the primitive it drives.
struct Rule {
    /// Rule key (CEP-LINT-*).
    std::string key;
    /// Severity 0..3.
    long severity = 0;
    /// Check primitive name.
    std::string check;
    /// Primitive mode (file_header, block, stub_body, ...).
    std::string mode;
    /// The rule's JSON payload for primitive parameters.
    Json params;
};

/// Loaded configuration.
struct Config {
    /// Standard version the configuration pins (doc-sync).
    std::string standard_version;
    /// Path of the standard document (doc-sync).
    std::string doc_path;
    /// Severities that fail the run.
    std::vector<long> fail_on;
    /// First-party source globs (0.4 applicability); empty disables the
    /// no-target scan mode.
    std::vector<std::string> file_globs;
    /// Rules in document order.
    std::vector<Rule> rules;
};

/// Read a whole file into a string; false when unreadable (BOOT-1).
[[nodiscard]] auto read_file(const std::string& path, std::string& out) -> bool;

/// Load and validate the configuration (BOOT-2..8 on failure).
[[nodiscard]] auto load_config(const std::string& path, Config& out, std::string& err) -> bool;

/// Deep-merge overlay into base (objects merge, arrays replace).
auto json_merge(const Json& base, const Json& overlay) -> Json;

/// Build a Config from a parsed JSON object.
[[nodiscard]] auto config_from_json(const Json& j, const std::string& origin, Config& out,
                                    std::string& err) -> bool;

/// List files under a directory (sorted, recursive) or the file itself.
[[nodiscard]] auto collect_files(const std::string& path, const std::vector<std::string>& exts)
    -> std::vector<std::string>;

/// Expand configuration file_globs from the working directory (sorted,
/// deduplicated); `**` spans any number of directories, `*` one segment.
[[nodiscard]] auto collect_globs(const std::vector<std::string>& globs,
                                 const std::vector<std::string>& exts)
    -> std::vector<std::string>;

/// Run every configured rule over one file; appends findings.
auto lint_file(const Config& cfg, const std::string& path, std::vector<Finding>& out,
               std::string& err) -> bool;

/// Check rule-catalog coverage: every configured rule has a `### Rule`
/// heading in the document, every documented rule exists in the config,
/// and the document's Version equals the configuration's version.
[[nodiscard]] auto doc_rule_coverage(const Config& cfg, std::vector<Finding>& out,
                                     std::string& err) -> bool;

/// Run one self-test scenario; returns the scenario's findings.
[[nodiscard]] auto run_scenario(const Json& base_cfg, const std::string& manifest_dir,
                                const Json& scenario, std::string& err) -> std::vector<Finding>;

/// Execute the whole self-test manifest; true when every scenario matches.
[[nodiscard]] auto self_test(const Json& base_cfg, const std::string& manifest_path,
                             std::string& report) -> bool;

/// Sort findings by file, line, rule, message (determinism, 6.5).
auto sort_findings(std::vector<Finding>& findings) -> void;

}  // namespace cep
