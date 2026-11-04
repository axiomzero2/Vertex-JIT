// CEP:FILE: tools/cep_lint/src/main.cpp
// CEP:WHAT: Command-line entry: option parsing, exit codes, bootstrap
//           error reporting and the deterministic scan loop.
// CEP:WHY: Standard 50.1: fixed options, coded bootstrap failures and
//           locale-independent output.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: exit 0 clean; 1 failing findings or failed self-test;
//              2 bootstrap (CEP-LINT-BOOT-<n>); 3 usage error.
// CEP:ASSUMES: the repository root is the working directory.
// CEP:COST: O(files x rules).
// CEP:EVIDENCE: self-test manifest; self-lint gate on this directory.
// CEP:SECURITY: reads only the paths handed to it; writes nothing.
#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

#include "json.hpp"
#include "lint.hpp"

/// Tool version, pinned to the standard it enforces.
constexpr const char* kToolVersion = "0.3.0";

/// Default configuration path.
constexpr const char* kDefaultConfig = ".cep/cep_lint.json";

/// Self-test manifest path.
constexpr const char* kManifestPath = "tools/cep_lint/tests/manifest.json";

/// Recognized source extensions.
constexpr const char* kExtensions[] = {".cpp", ".hpp", ".cc", ".hh"};

/// Minimum severity that fails a run by default.
constexpr long kFailSeverity0 = 0;
/// Second failing severity by default.
constexpr long kFailSeverity1 = 1;
/// Exit code: clean.
constexpr int kExitClean = 0;
/// Exit code: failing findings or failed self-test.
constexpr int kExitFindings = 1;
/// Exit code: bootstrap failure.
constexpr int kExitBootstrap = 2;
/// Exit code: usage error.
constexpr int kExitUsage = 3;

// CEP:WHAT: Print usage.
// CEP:WHY: Usage errors exit 3 per standard 50.1.
// CEP:STATUS: complete
// CEP:FAILURE: none.
// CEP:ASSUMES: stdout is writable.
// CEP:COST: O(1).
// CEP:EVIDENCE: manual invocation.
auto usage() -> void {
    std::printf("cep_lint [options] <files or directories>\n");
    std::printf("  --config, -c <path>   lint configuration JSON (default %s,\n", kDefaultConfig);
    std::printf("                        override CEP_LINT_CONFIG)\n");
    std::printf("  --self-test, -t       run %s and exit\n", kManifestPath);
    std::printf("  --help, -h            show usage\n");
    std::printf("  --version, -v         print tool and standard versions\n");
    std::printf("exit codes: 0 clean, 1 findings, 2 bootstrap, 3 usage\n");
}

// CEP:WHAT: Program entry: options, exit codes, deterministic scan loop.
// CEP:WHY: The contract of standard 50.1 lives here end to end.
// CEP:STATUS: complete
// CEP:FAILURE: exit codes as documented; bootstrap failures print the
//              coded error on stderr.
// CEP:ASSUMES: targets are files or directories under the repo root.
// CEP:COST: O(targets x rules).
// CEP:EVIDENCE: self-lint gate and self-test in CI.
auto main(int argc, char** argv) -> int {
    std::string config_path = kDefaultConfig;
    const char* env = std::getenv("CEP_LINT_CONFIG");
    if (env != nullptr && env[0] != '\0') {
        config_path = env;
    }
    bool self_test_mode = false;
    std::vector<std::string> targets;
    for (int i = 1; i < argc; ++i) {
        const std::string a = argv[i];
        if (a == "--config" || a == "-c") {
            if (i + 1 >= argc) {
                std::fprintf(stderr, "CEP-LINT usage: option requires a path\n");
                return kExitUsage;
            }
            config_path = argv[++i];
        } else if (a == "--self-test" || a == "-t") {
            self_test_mode = true;
        } else if (a == "--help" || a == "-h") {
            usage();
            return kExitClean;
        } else if (a == "--version" || a == "-v") {
            std::printf("cep_lint %s\n", kToolVersion);
            return 0;
        } else if (!a.empty() && a[0] == '-' && a != "-") {
            std::fprintf(stderr, "CEP-LINT usage: unknown option\n");
            return kExitUsage;
        } else {
            targets.push_back(a);
        }
    }

    cep::Config cfg;
    std::string err;
    if (!cep::load_config(config_path, cfg, err)) {
        std::fprintf(stderr, "%s\n", err.c_str());
        return kExitBootstrap;
    }

    if (self_test_mode) {
        std::string text;
        if (!cep::read_file(config_path, text)) {
            std::fprintf(stderr, "CEP-LINT-BOOT-1: cannot read configuration\n");
            return kExitBootstrap;
        }
        std::optional<cep::Json> base = cep::json_parse(text, config_path);
        if (!base.has_value()) {
            std::fprintf(stderr, "CEP-LINT-BOOT-2: %s\n", cep::json_last_error().c_str());
            return kExitBootstrap;
        }
        std::string report;
        const bool ok = cep::self_test(*base, kManifestPath, report);
        std::fputs(report.c_str(), stdout);
        if (!ok) {
            return kExitFindings;
        }
        std::printf("self-test: all scenarios pass\n");
        return 0;
    }

    if (targets.empty()) {
        std::fprintf(stderr, "CEP-LINT usage: no targets\n");
        return kExitUsage;
    }

    std::vector<std::string> files;
    const std::vector<std::string> exts(std::begin(kExtensions), std::end(kExtensions));
    for (const std::string& t : targets) {
        std::vector<std::string> part = cep::collect_files(t, exts);
        if (part.empty()) {
            std::fprintf(stderr, "CEP-LINT-BOOT-1: cannot read target %s\n", t.c_str());
            return kExitBootstrap;
        }
        for (const std::string& f : part) {
            files.push_back(f);
        }
    }
    std::sort(files.begin(), files.end());

    std::vector<cep::Finding> findings;
    for (const std::string& f : files) {
        if (!cep::lint_file(cfg, f, findings, err)) {
            std::fprintf(stderr, "%s\n", err.c_str());
            return kExitBootstrap;
        }
    }
    if (!cep::doc_rule_coverage(cfg, findings, err)) {
        std::fprintf(stderr, "%s\n", err.c_str());
        return kExitBootstrap;
    }
    cep::sort_findings(findings);

    for (const cep::Finding& f : findings) {
        std::printf("%s:%ld: %s [sev %ld]: %s\n", f.file.c_str(), f.line, f.rule.c_str(),
                    f.severity, f.message.c_str());
    }
    bool fail = false;
    for (const cep::Finding& f : findings) {
        if (f.severity == kFailSeverity0 || f.severity == kFailSeverity1) {
            bool configured = false;
            for (const long s : cfg.fail_on) {
                if (s == f.severity) {
                    configured = true;
                }
            }
            if (configured) {
                fail = true;
            }
        }
    }
    std::printf("cep_lint: %zu findings\n", findings.size());
    return fail ? kExitFindings : kExitClean;
}
