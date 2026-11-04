# Bytecode: Decoder, Verifier and Function Model

## Abstract

The engine consumes legacy Vertex bytecode: a byte stream of opcodes with
inline operands. `engine/src/cold/bytecode.rs` defines the opcode table, the
decoder (`decode_vertex`), the structural verifier (`verify`) and the
normalized `Function` model used by every tier. The decoder is the only
component that turns untrusted bytes into engine structures, so it performs
all range checks; downstream tiers assume verified input.

## Formal Definitions

A program is a function `Function { name, code, locals, strings }` where
`code` is a sequence of `Insn { op, operand }`. An operand is one of `None`,
`U64`, `F64`, `Slot(u16)`, `StrId(u32)` or `Target(u32)`.

The jump-target convention is instruction-index based: `Target(t)` refers to
`code[t]`, and the verifier rejects `t >= code.len()`. This is the legacy
convention and differs from byte offsets; the regression assembler in
`tests/stack_discipline.rs` mirrors it.

Opcode values are normative and pinned by compile-time assertions in
`bytecode.rs` (Law 3: enforced, not commented). The T0 subset accepted by
`check_legality` is: PushNumber, PushBool, Add, Sub, Mul, Div, Modulo,
GreaterThan, LessThan, Equal, LoadVar, StoreVar, AssignVar, Jump,
JumpIfFalse, JumpIfTrue, Drop, Halt.

## Algorithm

`decode_vertex` walks the byte stream once. For each opcode it reads the
documented operand width (f32 for PushNumber, one byte for PushBool, u16 for
jump targets, u32 length plus bytes for variable names), resolves variable
names to slots in first-use order, and rejects truncation, unknown opcodes
and invalid UTF-8 with typed errors. After decoding it runs `verify`.

## Invariants

1. Every decoded function contains at least one Halt; the verifier rejects
   functions without one.
2. Every Target and Slot operand is in range after `verify`.
3. Slot indices are dense and assigned in first-use order, which makes slot
   numbering a deterministic function of the bytecode.
4. Opcode byte values never change; the compile-time assertions fail the
   build otherwise.

## Integration Points

T0 legality (`tier0::check_legality`), the IR lifter (`ir::lift`), the
interpreter (`hot::interp`) and the deoptimizer all read `Function`. The
differential suite assembles raw bytecode and relies on the decoder's
first-use slot order.

## Known Limitations

1. The operand stack is not constrained at Halt by `verify`; programs with
   an empty stack at Halt decode fine, and the interpreter returns no value
   while compiled tiers return zero. Tracked as CEP-ENGINE-014.
2. `JumpLastOnStack` and the legacy call convention are decoded but not
   lifted to IR (the engine's CALL/RET extension replaces them).
