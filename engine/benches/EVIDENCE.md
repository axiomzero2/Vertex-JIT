# T0 Compile-Latency Evidence

## Abstract

Recorded measurements backing the spec 2.1 budget: Tier 0 compiles a
function in under five microseconds. The budget is asserted directly in
release builds by `t0_compile_latency_under_budget` (tests/t0_exec.rs);
debug builds carry a documented 100x slack because unoptimized code is not
the object of the budget. Reproduce a measurement with
`cargo run --release --example bench_t0`.

## Methodology

The instrument compiles a fixed 64-instruction program (32 push-number,
31 add, halt) one thousand times per assertion and takes the mean; the
example bench warms up for 2k iterations and reports the mean over 20k.
Means, not maxima, are recorded: the budget is a compile-throughput budget,
and single-run maxima on shared CI hosts measure scheduler noise, not the
compiler. The host is calibrated before each campaign with a fixed-work
loop (dependent integer adds) to confirm roughly one cycle per iteration.

## Measurements

| Date       | Host                        | check_legality | compile() mean | State |
| ---------- | --------------------------- | -------------- | -------------- | ----- |
| 2026-11-06 | Xeon container, 2 vCPU      | 1.51 us        | 2.70 us        | green |
| 2026-11-06 | same host, pre-fix checker  | 3.64 us        | 4.70 us        | green (margin too thin) |
| 2026-11-06 | same host, pass-loop era    | (n/a)          | 18.7 us        | RED   |

The 18.7 us row is the regression this file exists to document: the
legality checker recomputed every visited pc once per fixpoint pass and
deep-cloned its abstract state (three heap `Vec`s) two to three times per
recomputation, an O(n^2)-clone pattern that crosses the budget on
64-instruction programs. The fix replaces the pass loop with a worklist
that visits each pc exactly once and moves an allocation-free `Copy` state
(fixed-capacity stack and slot arrays); measured effect is a 6.9x speedup
of `compile` on the calibration program.

## Budget Enforcement History

An audit on 2026-11-06 found the release assertion had never run in CI:
the workflow executed `cargo test` in debug mode only, so the 5 us budget
branch was dead infrastructure and the 18.7 us regression passed CI
unnoticed. The engine CI job now runs the suite in release mode in
addition to debug mode, which is what keeps this file's top row honest.
