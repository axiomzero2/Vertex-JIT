# On-Stack Replacement

## Abstract

OSR moves a long-running interpreter frame into compiled code at a loop
header, so a hot loop that started interpreted still gets compiled
execution. `engine/src/cold/engine.rs` implements the transfer; the
interpreter reports the loop-header pc through the budget window mechanism.

## Formal Definitions

An OSR entry is valid only at a loop header pc (a back-edge target). The
engine requires this and rejects other pcs with `OsrBadEntry`. The transfer
takes the interpreter's operand stack and locals at the header and builds
the compiled frame state from them.

## Algorithm

When the interpreter's budget window exhausts at a back edge, the engine
records the site, maybe compiles the function for the next tier, and, if
compiled code is available, rebuilds the frame: locals move slot for slot,
the operand stack must be empty at the header (the same refusal contract as
the IR lifter), and execution jumps to the loop header's machine address.

## Invariants

1. OSR entries happen only at verified loop headers.
2. The operand stack is empty at the transfer; the interpreter's stack
   discipline at loop headers guarantees it for accepted programs.
3. Values transferred keep their types; the deopt machinery is the inverse
   direction (doc/deopt.md) and shares the frame layout assumptions.

## Integration Points

The engine's run loop calls `try_osr` after each budget window; the
interpreter's back-edge counters feed the decision. Tier thresholds and the
OSR threshold (20000 back edges) live in `cold::tier`.

## Known Limitations

1. OSR from compiled code back into a fresh compiled frame (versioned
   loops) is not implemented; the interpreter path covers the observed
   workloads.
2. OSR of functions with live values across the loop header is refused with
   the same loudness as the IR lifter's unbalanced-stack refusal.
