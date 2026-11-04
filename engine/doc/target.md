# Target Detection and Feature Gating

## Abstract

`engine/src/platform.rs` detects the execution target and exposes the
constants the engine needs: page size, cache-line size, endianness, virtual
address bits, AVX2 and AVX-512 availability. Codegen consumes the flags;
the code cache consumes the page size.

## Formal Definitions

`TargetConfig { cache_line_bytes, page_bytes, little_endian,
virtual_address_bits, avx2, avx512f, default_code_cache_bytes }`. Detection
is deterministic on a given machine: the same host yields the same config,
which keeps publication reproducible.

## Algorithm

Detection uses CPUID leaves for the vector features and `sysconf`-equivalent
queries for the page size. Values are sanity-bounded; a host reporting
absurd constants fails the target contract with `TargetError` instead of
producing misaligned code.

## Invariants

1. The config is immutable after detection; no component mutates it.
2. Every alignment decision (code cache, heap) derives from the detected
   page and cache-line constants, never from hard-coded numbers.
3. Feature flags gate code paths only; a flag does not change value
   semantics.

## Integration Points

The engine passes the config to the code cache and codegen; the vector path
(documented in doc/vectorize.md) keys on `avx2` and `avx512f`; the tests
assume x86-64 Linux.

## Known Limitations

1. Non-x86-64 hosts are unsupported; detection fails loudly.
2. AVX-512 gating is conservative: detection reports it, the vector path
   currently keys on AVX2 shapes.
