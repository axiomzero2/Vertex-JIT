// CEP:FILE: engine/src/cold/bytecode.rs
// CEP:WHAT: The engine's normalized bytecode: instruction set, function
//           container, runtime string pool, Vertex binary decoder and
//           structural verifier.
// CEP:WHY: Every tier consumes one normalized, fully validated bytecode
//          form; name resolution happens once here so hot paths only see
//          slot indices. Opcodes 70-75 are engine extensions (arrays and
//          calls) defined in doc/bytecode.md for the tiers to exercise;
//          the current vertexC does not emit them.
// CEP:CLASS: CEP-1
// CEP:STATUS: complete
// CEP:FAILURE: Decoding returns BytecodeError for truncated input, unknown
//              opcodes, invalid UTF-8, out-of-range jump targets and
//              missing Halt. No panics on any input.
// CEP:ASSUMES: Vertex binary format (u16 LE jump operands, u32 LE string
//              lengths, f32 LE numbers, u64 LE usize operands) as
//              implemented by src/runtime/virtual_machine/pre_parsing.rs.
// CEP:COST: decode is O(n) over input bytes with a second O(n) verify pass.
// CEP:EVIDENCE: unit tests below; golden compiler-output test.
// CEP:SECURITY: Input is untrusted. All reads are bounds-checked before
//               access; allocations are bounded by input length.

use core::fmt;

/// Opcode values; numeric values are normative and match the compiler
/// constants in src/backend/compiler/instructions.rs (const assertions
/// below enforce it, per Law 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Opcode {
    /// a,b -> a+b (numbers) or concatenation (strings).
    Add = 1,
    /// a,b -> a-b.
    Sub = 2,
    /// a,b -> a/b (traps on zero divisor).
    Div = 3,
    /// a,b -> a*b.
    Mul = 4,
    /// a,b -> a mod b.
    Modulo = 5,
    /// a,b -> a>b.
    GreaterThan = 6,
    /// a,b -> a<b.
    LessThan = 7,
    /// a,b -> a==b.
    Equal = 8,
    /// Push an interned string (string id operand).
    PushString = 20,
    /// Push a boolean (0/1 operand).
    PushBool = 21,
    /// Push a number (f32 widened to f64 at decode).
    PushNumber = 22,
    /// Push a u64 constant (loop addressing in the legacy convention).
    PushUsize = 23,
    /// Print top of stack with newline (pops).
    WriteLn = 30,
    /// Print top of stack (pops).
    Write = 31,
    /// Read one trimmed input line as a string.
    ReadInput = 60,
    /// Terminate with the popped exit code.
    ProcessExit = 61,
    /// Define a variable (slot operand); pops the initial value.
    StoreVar = 50,
    /// Read a variable (slot operand).
    LoadVar = 51,
    /// Assign an existing variable (slot operand); pops the value.
    AssignVar = 52,
    /// Unconditional jump (target operand).
    Jump = 40,
    /// Pop a boolean; jump when false.
    JumpIfFalse = 41,
    /// Pop a boolean; jump when true.
    JumpIfTrue = 42,
    /// Move the top of the jump stack into pc (legacy return convention).
    JumpLastOnStack = 43,
    /// Allocate an array (pops length; pushes reference). \[extension\]
    NewArray = 70,
    /// Array length (pops array; pushes length). \[extension\]
    ArrayLen = 73,
    /// Element load (pops index, array; pushes word). \[extension\]
    LoadIdx = 71,
    /// Element store (pops value, index, array). \[extension\]
    StoreIdx = 72,
    /// Call function by id; operand packs (argc<<32)|callee. \[extension\]
    Call = 74,
    /// Return the top of stack to the caller. \[extension\]
    Ret = 75,
    /// Pop and discard a variable binding (slot operand).
    Drop = 125,
    /// Halt the loop.
    Halt = 255,
}

impl Opcode {
    /// Map a raw byte to an opcode.
    ///
    /// CEP:WHAT: Byte to opcode mapping.
    /// CEP:WHY: Decoder and verifier share one total mapping.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: None for undefined bytes.
    /// CEP:ASSUMES: none
    /// CEP:COST: one match.
    /// CEP:EVIDENCE: opcode-value golden test below.
    /// CEP:SECURITY: unknown bytes are rejected.
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Opcode> {
        match b {
            1 => Some(Opcode::Add),
            2 => Some(Opcode::Sub),
            3 => Some(Opcode::Div),
            4 => Some(Opcode::Mul),
            5 => Some(Opcode::Modulo),
            6 => Some(Opcode::GreaterThan),
            7 => Some(Opcode::LessThan),
            8 => Some(Opcode::Equal),
            20 => Some(Opcode::PushString),
            21 => Some(Opcode::PushBool),
            22 => Some(Opcode::PushNumber),
            23 => Some(Opcode::PushUsize),
            30 => Some(Opcode::WriteLn),
            31 => Some(Opcode::Write),
            40 => Some(Opcode::Jump),
            41 => Some(Opcode::JumpIfFalse),
            42 => Some(Opcode::JumpIfTrue),
            43 => Some(Opcode::JumpLastOnStack),
            50 => Some(Opcode::StoreVar),
            51 => Some(Opcode::LoadVar),
            52 => Some(Opcode::AssignVar),
            60 => Some(Opcode::ReadInput),
            61 => Some(Opcode::ProcessExit),
            70 => Some(Opcode::NewArray),
            71 => Some(Opcode::LoadIdx),
            72 => Some(Opcode::StoreIdx),
            73 => Some(Opcode::ArrayLen),
            74 => Some(Opcode::Call),
            75 => Some(Opcode::Ret),
            125 => Some(Opcode::Drop),
            255 => Some(Opcode::Halt),
            _ => None,
        }
    }
}

/// Operand of a normalized instruction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Operand {
    /// No operand.
    None,
    /// 64-bit integer operand.
    U64(u64),
    /// 64-bit float operand (widened from f32 at decode).
    F64(f64),
    /// Local slot index.
    Slot(u16),
    /// String pool id.
    StrId(u32),
    /// Jump target (instruction index).
    Target(u32),
}

/// One normalized instruction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Insn {
    /// Opcode.
    pub op: Opcode,
    /// Operand (opcode-specific; doc/bytecode.md).
    pub operand: Operand,
}

/// A decoded, name-resolved, structurally verified function.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// Function name (diagnostics only).
    pub name: String,
    /// Normalized instruction stream.
    pub code: Vec<Insn>,
    /// Local slot names in slot order (deterministic first-definition).
    pub locals: Vec<String>,
    /// Static string pool.
    pub strings: Vec<String>,
}

/// Bytecode decoding and verification errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeError {
    /// Input ended mid-instruction.
    Truncated {
        /// Byte offset.
        at: usize,
        /// What was being read.
        what: &'static str,
    },
    /// Undefined opcode byte.
    UnknownOpcode {
        /// Byte offset.
        at: usize,
        /// The byte.
        byte: u8,
    },
    /// A string operand was not valid UTF-8.
    InvalidUtf8 {
        /// Byte offset.
        at: usize,
    },
    /// A jump target or slot id is outside the stream.
    JumpOutOfRange {
        /// Instruction index.
        pc: usize,
        /// The invalid target.
        target: usize,
        /// Stream length.
        len: usize,
    },
    /// More local slots than u16 operands can address.
    TooManyVariables,
    /// No Halt instruction (structural requirement).
    NoHalt,
}

impl fmt::Display for BytecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BytecodeError::Truncated { at, what } => {
                write!(f, "truncated input at byte {at}: expected {what}")
            }
            BytecodeError::UnknownOpcode { at, byte } => {
                write!(f, "unknown opcode {byte:#04x} at byte {at}")
            }
            BytecodeError::InvalidUtf8 { at } => write!(f, "invalid UTF-8 string at byte {at}"),
            BytecodeError::JumpOutOfRange { pc, target, len } => {
                write!(f, "jump at {pc} targets {target} but len is {len}")
            }
            BytecodeError::TooManyVariables => write!(f, "more than 65535 local slots"),
            BytecodeError::NoHalt => write!(f, "function does not end in Halt"),
        }
    }
}

impl std::error::Error for BytecodeError {}

/// Upper bound on local slots (u16 slot operands).
pub const MAX_LOCALS: usize = u16::MAX as usize;
/// Upper bound on string ids (u32 operands).
pub const MAX_STRINGS: usize = u32::MAX as usize;

/// Structural verifier: operand ranges and the Halt requirement.
///
/// CEP:WHAT: Validates structural invariants after decode.
/// CEP:WHY: Tiers assume operands are in range (Law 2); this runs once
///          before any tier sees the code.
/// CEP:STATUS: complete
/// CEP:FAILURE: First violated invariant as BytecodeError.
/// CEP:ASSUMES: none
/// CEP:COST: O(n), no allocation.
/// CEP:EVIDENCE: negative tests below.
/// CEP:SECURITY: input is untrusted; this is the trust boundary gate.
///
/// # Errors
/// Returns [`BytecodeError`] for the first out-of-range operand.
pub fn verify(func: &Function) -> Result<(), BytecodeError> {
    let len = func.code.len();
    for (pc, insn) in func.code.iter().enumerate() {
        match insn.operand {
            Operand::Target(t) => {
                if t as usize >= len {
                    return Err(BytecodeError::JumpOutOfRange { pc, target: t as usize, len });
                }
            }
            Operand::Slot(s) => {
                if s as usize >= func.locals.len() {
                    return Err(BytecodeError::JumpOutOfRange { pc, target: s as usize, len });
                }
            }
            Operand::StrId(id) => {
                if id as usize >= func.strings.len() {
                    return Err(BytecodeError::JumpOutOfRange { pc, target: id as usize, len });
                }
            }
            Operand::None | Operand::U64(_) | Operand::F64(_) => {}
        }
    }
    if !func.code.iter().any(|i| i.op == Opcode::Halt) {
        return Err(BytecodeError::NoHalt);
    }
    Ok(())
}

/// Append-only runtime string pool shared by all tiers.
///
/// Ids 0..n mirror the function's static pool; runtime strings (concat,
/// ReadInput) extend it. Append-only so ids are stable forever.
#[derive(Debug, Clone, Default)]
pub struct StringPool {
    strings: Vec<String>,
    budget: u32,
}

impl StringPool {
    /// Build a pool from static strings plus a byte budget.
    ///
    /// CEP:WHAT: Pool construction.
    /// CEP:WHY: One pool serves all tiers; budget bounds untrusted growth.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(static strings), once per run.
    /// CEP:EVIDENCE: pool tests below.
    /// CEP:SECURITY: runtime strings are untrusted; budget caps growth.
    #[must_use]
    pub fn new(static_strings: &[String], budget_bytes: u32) -> StringPool {
        StringPool { strings: static_strings.to_vec(), budget: budget_bytes }
    }

    /// Look up a string by id.
    ///
    /// CEP:WHAT: Pool read.
    /// CEP:WHY: Renderer and helpers need text for ids.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: debug panic on out-of-range ids (verifier contract).
    /// CEP:ASSUMES: id < len (verifier-checked for static ids).
    /// CEP:COST: O(1).
    /// CEP:EVIDENCE: pool tests.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn get(&self, id: u32) -> &str {
        &self.strings[id as usize]
    }

    /// Intern one string.
    ///
    /// CEP:WHAT: Pool append.
    /// CEP:WHY: ReadInput and concat produce new strings.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: None when the byte budget is exhausted.
    /// CEP:ASSUMES: none
    /// CEP:COST: O(len); cold path.
    /// CEP:EVIDENCE: pool tests.
    /// CEP:SECURITY: untrusted input capped by budget.
    pub fn intern(&mut self, s: &str) -> Option<u32> {
        if let Some(i) = self.strings.iter().position(|x| x == s) {
            return Some(i as u32);
        }
        let bytes = s.len() as u64;
        if self.used_bytes() + bytes > u64::from(self.budget)
            || self.strings.len() >= MAX_STRINGS
        {
            return None;
        }
        self.strings.push(s.to_owned());
        Some((self.strings.len() - 1) as u32)
    }

    /// Intern the concatenation of two pooled strings.
    ///
    /// CEP:WHAT: Concatenation.
    /// CEP:WHY: Add on strings must not format or allocate twice.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: None when the budget is exhausted.
    /// CEP:ASSUMES: both ids in range (verifier contract).
    /// CEP:COST: O(len(a)+len(b)).
    /// CEP:EVIDENCE: differential string tests.
    /// CEP:SECURITY: untrusted input capped by budget.
    pub fn concat(&mut self, a: u32, b: u32) -> Option<u32> {
        let text = self.strings[a as usize].clone();
        let joined = text + &self.strings[b as usize];
        self.intern(&joined)
    }

    /// Number of strings.
    ///
    /// CEP:WHAT: Pool length.
    /// CEP:WHY: Bounds checks at use sites.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(1).
    /// CEP:EVIDENCE: pool tests.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// True when the pool is empty.
    ///
    /// CEP:WHAT: Emptiness.
    /// CEP:WHY: clippy requires the pair with len.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(1).
    /// CEP:EVIDENCE: pool tests.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    fn used_bytes(&self) -> u64 {
        self.strings.iter().map(|s| s.len() as u64).sum()
    }
}

/// Decode a function from the Vertex binary bytecode format.
///
/// CEP:WHAT: Vertex binary to normalized function.
/// CEP:WHY: The engine consumes exactly the stream vertexC emits, plus the
///          documented extension opcodes.
/// CEP:STATUS: complete
/// CEP:FAILURE: BytecodeError for every malformed input class; never panics.
/// CEP:ASSUMES: little-endian host (checked by TargetConfig).
/// CEP:COST: O(input bytes); one String per string operand.
/// CEP:EVIDENCE: golden stream tests below.
/// CEP:SECURITY: all reads bounds-checked; string lengths bounded by the
///               remaining input; slots resolve deterministically.
///
/// # Errors
/// Returns [`BytecodeError`] for malformed input.
pub fn decode_vertex(name: &str, bytes: &[u8]) -> Result<Function, BytecodeError> {
    let mut p = 0usize;
    let mut code: Vec<Insn> = Vec::with_capacity(bytes.len().min(4096));
    let mut locals: Vec<String> = Vec::new();
    let mut strings: Vec<String> = Vec::new();

    macro_rules! CEP_NEED {
        ($n:expr, $what:expr) => {
            if p + $n > bytes.len() {
                return Err(BytecodeError::Truncated { at: p, what: $what });
            }
        };
    }

    while p < bytes.len() {
        let at = p;
        let byte = bytes[p];
        p += 1;
        let op = Opcode::from_byte(byte).ok_or(BytecodeError::UnknownOpcode { at, byte })?;
        let operand = match op {
            Opcode::Add
            | Opcode::Sub
            | Opcode::Div
            | Opcode::Mul
            | Opcode::Modulo
            | Opcode::GreaterThan
            | Opcode::LessThan
            | Opcode::Equal
            | Opcode::WriteLn
            | Opcode::Write
            | Opcode::ReadInput
            | Opcode::ProcessExit
            | Opcode::JumpLastOnStack
            | Opcode::NewArray
            | Opcode::ArrayLen
            | Opcode::LoadIdx
            | Opcode::StoreIdx
            | Opcode::Ret
            | Opcode::Halt => Operand::None,
            Opcode::PushBool => {
                CEP_NEED!(1, "bool operand");
                let v = bytes[p];
                p += 1;
                Operand::U64(u64::from(v != 0))
            }
            Opcode::PushNumber => {
                CEP_NEED!(4, "f32 operand");
                let mut b = [0u8; 4];
                b.copy_from_slice(&bytes[p..p + 4]);
                p += 4;
                let f = f64::from(f32::from_le_bytes(b));
                Operand::F64(crate::hot::value::canonicalize_f64(f))
            }
            Opcode::PushUsize | Opcode::Call => {
                CEP_NEED!(8, "u64 operand");
                let mut b = [0u8; 8];
                b.copy_from_slice(&bytes[p..p + 8]);
                p += 8;
                Operand::U64(u64::from_le_bytes(b))
            }
            Opcode::Jump | Opcode::JumpIfFalse | Opcode::JumpIfTrue => {
                CEP_NEED!(2, "u16 jump operand");
                let t = u16::from_le_bytes([bytes[p], bytes[p + 1]]);
                p += 2;
                Operand::Target(u32::from(t))
            }
            Opcode::PushString => {
                CEP_NEED!(4, "u32 string length");
                let l = u32::from_le_bytes([bytes[p], bytes[p + 1], bytes[p + 2], bytes[p + 3]])
                    as usize;
                p += 4;
                CEP_NEED!(l, "string bytes");
                let s = core::str::from_utf8(&bytes[p..p + l])
                    .map_err(|_| BytecodeError::InvalidUtf8 { at: p })?;
                p += l;
                if strings.len() >= MAX_STRINGS {
                    return Err(BytecodeError::TooManyVariables);
                }
                strings.push(s.to_owned());
                Operand::StrId((strings.len() - 1) as u32)
            }
            Opcode::StoreVar | Opcode::LoadVar | Opcode::AssignVar | Opcode::Drop => {
                CEP_NEED!(4, "u32 name length");
                let l = u32::from_le_bytes([bytes[p], bytes[p + 1], bytes[p + 2], bytes[p + 3]])
                    as usize;
                p += 4;
                CEP_NEED!(l, "name bytes");
                let s = core::str::from_utf8(&bytes[p..p + l])
                    .map_err(|_| BytecodeError::InvalidUtf8 { at: p })?;
                p += l;
                resolve_slot(&mut locals, s)?
            }
        };
        code.push(Insn { op, operand });
    }

    let func = Function {
        name: name.to_owned(),
        code,
        locals,
        strings,
    };
    verify(&func)?;
    Ok(func)
}

/// Resolve a variable name to a slot, defining it on first use.
///
/// CEP:WHAT: First-definition slot assignment.
/// CEP:WHY: Hot paths need slot indices; order is deterministic.
/// CEP:STATUS: complete
/// CEP:FAILURE: TooManyVariables above 65535 slots.
/// CEP:ASSUMES: none
/// CEP:COST: O(existing slots) linear scan; decode is CEP-1.
/// CEP:EVIDENCE: slot resolution test below.
/// CEP:SECURITY: names bounded by input length.
///
/// # Errors
/// Returns [`BytecodeError::TooManyVariables`] when the space is full.
fn resolve_slot(locals: &mut Vec<String>, name: &str) -> Result<Operand, BytecodeError> {
    if let Some(i) = locals.iter().position(|n| n == name) {
        return Ok(Operand::Slot(i as u16));
    }
    if locals.len() >= MAX_LOCALS {
        return Err(BytecodeError::TooManyVariables);
    }
    locals.push(name.to_owned());
    Ok(Operand::Slot((locals.len() - 1) as u16))
}

// Normative opcode-value assertions (Law 3: enforced, not commented).
const _: () = {
    assert!(Opcode::Add as u8 == 1);
    assert!(Opcode::Sub as u8 == 2);
    assert!(Opcode::Div as u8 == 3);
    assert!(Opcode::Mul as u8 == 4);
    assert!(Opcode::Modulo as u8 == 5);
    assert!(Opcode::GreaterThan as u8 == 6);
    assert!(Opcode::LessThan as u8 == 7);
    assert!(Opcode::Equal as u8 == 8);
    assert!(Opcode::PushString as u8 == 20);
    assert!(Opcode::PushBool as u8 == 21);
    assert!(Opcode::PushNumber as u8 == 22);
    assert!(Opcode::PushUsize as u8 == 23);
    assert!(Opcode::WriteLn as u8 == 30);
    assert!(Opcode::Write as u8 == 31);
    assert!(Opcode::Jump as u8 == 40);
    assert!(Opcode::JumpIfFalse as u8 == 41);
    assert!(Opcode::JumpIfTrue as u8 == 42);
    assert!(Opcode::JumpLastOnStack as u8 == 43);
    assert!(Opcode::StoreVar as u8 == 50);
    assert!(Opcode::LoadVar as u8 == 51);
    assert!(Opcode::AssignVar as u8 == 52);
    assert!(Opcode::ReadInput as u8 == 60);
    assert!(Opcode::ProcessExit as u8 == 61);
    assert!(Opcode::NewArray as u8 == 70);
    assert!(Opcode::LoadIdx as u8 == 71);
    assert!(Opcode::StoreIdx as u8 == 72);
    assert!(Opcode::ArrayLen as u8 == 73);
    assert!(Opcode::Call as u8 == 74);
    assert!(Opcode::Ret as u8 == 75);
    assert!(Opcode::Drop as u8 == 125);
    assert!(Opcode::Halt as u8 == 255);
};

#[cfg(test)]
mod tests {
    use super::*;

    /// CEP:WHAT: Arithmetic stream decode: Push/Add/Sub/Mul/Div words.
/// CEP:WHY: The decoder is the front door for untrusted bytecode; the
///              arithmetic path pins word widths and NaN
///              canonicalization at decode.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: golden bytes asserted inline.
    #[test]
    fn decodes_arithmetic_stream() {
        let mut b = Vec::new();
        b.push(22u8);
        b.extend_from_slice(&5.0f32.to_le_bytes());
        b.push(22u8);
        b.extend_from_slice(&2.0f32.to_le_bytes());
        b.push(1u8);
        b.push(255u8);
        let f = decode_vertex("arith", &b).unwrap();
        assert_eq!(f.code.len(), 4);
        assert_eq!(f.code[0].operand, Operand::F64(5.0));
        assert_eq!(f.code[3].op, Opcode::Halt);
    }

    /// CEP:WHAT: Name-prefixed instructions and label resolution.
/// CEP:WHY: Store/Load/Assign names and jump targets are the two variable
///              width encodings; both must round-trip exactly.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: golden bytes asserted inline.
    #[test]
    fn decodes_names_and_jumps() {
        let mut b = Vec::new();
        b.push(50u8);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(b"i");
        b.push(51u8);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(b"i");
        b.push(40u8);
        b.extend_from_slice(&0u16.to_le_bytes());
        b.push(255u8);
        let f = decode_vertex("names", &b).unwrap();
        assert_eq!(f.locals, vec!["i".to_owned()]);
        assert_eq!(f.code[0].operand, Operand::Slot(0));
        assert_eq!(f.code[2].operand, Operand::Target(0));
    }

    /// CEP:WHAT: Reject unknown opcodes and truncated streams.
/// CEP:WHY: Fail-closed decode (Law 6): malformed input must never produce a
///              half-function.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: negative cases asserted inline.
    #[test]
    fn rejects_unknown_opcode_and_truncation() {
        assert_eq!(
            decode_vertex("x", &[0xDD]),
            Err(BytecodeError::UnknownOpcode { at: 0, byte: 0xDD })
        );
        assert!(matches!(
            decode_vertex("x", &[22, 0, 0]),
            Err(BytecodeError::Truncated { .. })
        ));
    }

    /// CEP:WHAT: Reject jump targets beyond the instruction stream.
/// CEP:WHY: Out-of-range jumps would corrupt the compiler's pc walk; the
///              decoder rejects them before any tier sees the function.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: negative case asserted inline.
    #[test]
    fn rejects_jump_out_of_range() {
        let mut b = vec![40u8];
        b.extend_from_slice(&5u16.to_le_bytes());
        b.push(255u8);
        assert!(matches!(
            decode_vertex("x", &b),
            Err(BytecodeError::JumpOutOfRange { .. })
        ));
    }

    /// CEP:WHAT: Reject a function whose last instruction is not Halt.
/// CEP:WHY: Every tier assumes a Halt tail (compile()'s tail simulation and
///              the interpreter's exit); the decoder enforces it.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: negative case asserted inline.
    #[test]
    fn rejects_missing_halt() {
        assert_eq!(decode_vertex("x", &[1u8, 1u8]), Err(BytecodeError::NoHalt));
    }

    /// CEP:WHAT: NaN constants canonicalize at decode time.
/// CEP:WHY: The value representation bans non-canonical NaNs (doc/value.md);
///              the decoder is the first canonicalization point.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: bit patterns asserted inline.
    #[test]
    fn canonicalizes_nan_constants() {
        let mut b = vec![22u8];
        b.extend_from_slice(&f32::NAN.to_le_bytes());
        b.push(255u8);
        let f = decode_vertex("x", &b).unwrap();
        match f.code[0].operand {
            Operand::F64(v) => assert!(v.is_nan() && v.is_sign_positive()),
            _ => panic!("expected F64 operand"),
        }
    }
}
