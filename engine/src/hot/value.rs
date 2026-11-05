// CEP:FILE: engine/src/hot/value.rs
// CEP:WHAT: The 64-bit NaN-boxed machine value representation: encode,
//           decode, the box classifier and NaN canonicalization.
// CEP:WHY: One canonical representation prevents cross-tier value
//          confusion; the box classifier is the single branch the GC
//          barriers and deoptimizer need (doc/value.md).
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: none for the value primitives; boundary behavior of the
//              value stack lives in hot/stack.rs.
// CEP:ASSUMES: f64 NaN words >= BOX_BASE never occur as legitimate doubles;
//              enforced by NaN canonicalization at every creation site
//              (Div, Modulo, PushNumber) and checked by tests.
// CEP:COST: is_box is one unsigned compare; encode/decode one branch plus
//           shift/mask. Measured in benches/engine_bench.rs (artifact
//           benches/EVIDENCE.md).
// CEP:EVIDENCE: benches/engine_bench.rs; tests/differential.rs (bit-exact
//               across tiers); unit tests below.
// CEP:OPTIMAL: target-optimal for the classification and box primitives
// CEP:OPTPROOF: is_box compiles to one cmp (disassembly reviewed in
//               benches/EVIDENCE.md); no load, no allocation, cannot be
//               fewer instructions.
// CEP:SECURITY: heap references are masked to the 47-bit payload at the box
//               boundary; wide pointers cannot leak into value words.
//! Machine value representation (doc/value.md).

/// A 64-bit machine word holding one engine value.
pub type Word = u64;

/// Lowest possible boxed word; any word at or above it is a box.
pub const BOX_BASE: Word = 0xFFF8_0000_0000_0000;

/// Mask for the 47-bit payload (bits 0..=46) of a boxed word.
pub const PAYLOAD_MASK: Word = 0x0000_7FFF_FFFF_FFFF;

/// Canonical quiet NaN used for every floating NaN value.
pub const CANONICAL_NAN: Word = 0x7FF8_0000_0000_0000;

/// Box tag for interned string references (payload = string id).
pub const ITAG_STR: u64 = 2;

/// Box tag for heap object references (payload = 47-bit heap pointer).
pub const ITAG_OBJ: u64 = 3;

/// Engine value in the interpreter domain: the typed form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    /// A double-precision number (Vertex widens f32 sources losslessly).
    Number(f64),
    /// A boolean.
    Bool(bool),
    /// An interned string id.
    Str(u32),
    /// A heap object reference.
    Obj(usize),
    /// The unit/null value.
    Null,
}

/// Static type tag of an interpreter value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeTag {
    /// f64 number.
    Number,
    /// Boolean.
    Bool,
    /// String id.
    Str,
    /// Heap object reference.
    Obj,
    /// Unit value.
    Null,
}

impl Value {
    /// Type tag of this value.
    ///
    /// CEP:WHAT: Returns the static type tag.
    /// CEP:WHY: The profiler and IR builder need cheap type reads.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: one discriminant test.
    /// CEP:EVIDENCE: unit tests; differential suite.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: single discriminant compare.
    /// CEP:SECURITY: none
    #[inline]
    #[must_use]
    pub const fn tag(&self) -> TypeTag {
        match self {
            Value::Number(_) => TypeTag::Number,
            Value::Bool(_) => TypeTag::Bool,
            Value::Str(_) => TypeTag::Str,
            Value::Obj(_) => TypeTag::Obj,
            Value::Null => TypeTag::Null,
        }
    }

    /// Encode this value into a machine word.
    ///
    /// CEP:WHAT: Interpreter-to-machine conversion.
    /// CEP:WHY: Tier boundaries pass values as words.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: Debug panic when an Obj payload exceeds 47 bits; the
    ///              heap validates pointer width at mapping time.
    /// CEP:ASSUMES: Obj pointers fit 47 bits (runtime-checked by the heap).
    /// CEP:COST: one tag test plus constant work.
    /// CEP:EVIDENCE: tests/value_roundtrip.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: one compare-and-branch per variant.
    /// CEP:SECURITY: payload masked to 47 bits.
    #[inline]
    #[must_use]
    pub fn encode(&self) -> Word {
        match self {
            Value::Number(f) => f.to_bits(),
            Value::Bool(b) => u64::from(*b),
            Value::Str(id) => box_word(ITAG_STR, u64::from(*id)),
            Value::Obj(ptr) => {
                debug_assert!((*ptr as u64) <= PAYLOAD_MASK, "Obj pointer must fit 47 bits");
                box_word(ITAG_OBJ, *ptr as u64 & PAYLOAD_MASK)
            }
            Value::Null => box_word(1, 0),
        }
    }

    /// Decode a machine word into the interpreter domain.
    ///
    /// CEP:WHAT: Machine-to-interpreter conversion.
    /// CEP:WHY: Deoptimization rebuilds frames from words.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: Raw words decode as numbers; booleans use decode_typed.
    /// CEP:ASSUMES: the word was produced by encode or a template.
    /// CEP:COST: one compare plus shift/mask.
    /// CEP:EVIDENCE: tests/value_roundtrip.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: one branch on the box range.
    /// CEP:SECURITY: reserved tags never leak into the interpreter.
    #[inline]
    #[must_use]
    pub fn decode(word: Word) -> Value {
        if word >= BOX_BASE {
            let itag = (word >> 47) & 0xF;
            let payload = word & PAYLOAD_MASK;
            match itag {
                ITAG_STR => Value::Str(payload as u32),
                ITAG_OBJ => Value::Obj(payload as usize),
                _ => Value::Null,
            }
        } else {
            Value::Number(f64::from_bits(word))
        }
    }

    /// Decode a word whose static type is known.
    ///
    /// CEP:WHAT: Type-directed decode for the deoptimizer.
    /// CEP:WHY: Raw words are ambiguous between number and boolean.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none for records produced by codegen.
    /// CEP:ASSUMES: tag matches the generator's record.
    /// CEP:COST: one match.
    /// CEP:EVIDENCE: tests/deopt_frame_reconstruction.
    /// CEP:SECURITY: none
    #[inline]
    #[must_use]
    pub fn decode_typed(word: Word, tag: TypeTag) -> Value {
        match tag {
            TypeTag::Number => Value::Number(f64::from_bits(word)),
            TypeTag::Bool => Value::Bool(word & 1 == 1),
            _ => Value::decode(word),
        }
    }
}

/// Build a boxed word from a tag and a payload.
///
/// CEP:WHAT: Box word constructor.
/// CEP:WHY: Single place enforces the box layout.
/// CEP:STATUS: complete
/// CEP:FAILURE: none; payload is masked.
/// CEP:ASSUMES: itag fits 4 bits (masked).
/// CEP:COST: three ALU ops.
/// CEP:EVIDENCE: tests/value_roundtrip.
/// CEP:OPTIMAL: target-optimal
/// CEP:OPTPROOF: or of three immediates.
/// CEP:SECURITY: payload masked to 47 bits.
#[inline]
#[must_use]
pub const fn box_word(itag: u64, payload: u64) -> Word {
    BOX_BASE | ((itag & 0xF) << 47) | (payload & PAYLOAD_MASK)
}

/// True when the word is a box (never a legitimate double or boolean).
///
/// CEP:WHAT: Box classifier.
/// CEP:WHY: GC barriers and root scans branch on this.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: NaN canonicalization invariant.
/// CEP:COST: 1 cycle expected (one cmp); benches/EVIDENCE.md.
/// CEP:EVIDENCE: benches/engine_bench.rs; disassembly review.
/// CEP:OPTIMAL: target-optimal
/// CEP:OPTPROOF: single unsigned compare; lower bound is one compare.
/// CEP:SECURITY: none
#[inline]
#[must_use]
pub const fn is_box(word: Word) -> bool {
    word >= BOX_BASE
}

/// Extract the box tag.
///
/// CEP:WHAT: Box tag extraction.
/// CEP:WHY: GC and runtime paths need the type of a boxed word.
/// CEP:STATUS: complete
/// CEP:FAILURE: Raw words yield unspecified tags; callers check is_box.
/// CEP:ASSUMES: boxed input.
/// CEP:COST: shift plus mask.
/// CEP:EVIDENCE: tests/value_roundtrip.
/// CEP:OPTIMAL: target-optimal
/// CEP:OPTPROOF: two ALU ops.
/// CEP:SECURITY: none
#[inline]
#[must_use]
pub const fn box_itag(word: Word) -> u64 {
    (word >> 47) & 0xF
}

/// Extract the box payload.
///
/// CEP:WHAT: Box payload extraction.
/// CEP:WHY: String ids and heap pointers live in the payload.
/// CEP:STATUS: complete
/// CEP:FAILURE: Raw words yield raw bits; callers check is_box.
/// CEP:ASSUMES: boxed input.
/// CEP:COST: one mask.
/// CEP:EVIDENCE: tests/value_roundtrip.
/// CEP:OPTIMAL: target-optimal
/// CEP:OPTPROOF: one and-immediate.
/// CEP:SECURITY: none
#[inline]
#[must_use]
pub const fn box_payload(word: Word) -> u64 {
    word & PAYLOAD_MASK
}

/// Canonicalize an f64: map every NaN to CANONICAL_NAN.
///
/// CEP:WHAT: NaN normalization.
/// CEP:WHY: A hardware negative NaN (0xFFF8..) would fall inside the box
///          range and break the classifier.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: one ordered-compare in the common path.
/// CEP:EVIDENCE: tests/nan_canonicalization.
/// CEP:OPTIMAL: target-optimal
/// CEP:OPTPROOF: one branch on the NaN predicate.
/// CEP:SECURITY: none
#[inline]
#[must_use]
pub const fn canonicalize_f64(f: f64) -> f64 {
    if f.is_nan() {
        f64::from_bits(CANONICAL_NAN)
    } else {
        f
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// CEP:WHAT: Encode/decode round-trip over every value shape.
/// CEP:WHY: One canonical representation (doc/value.md) requires bit-exact
///              round-trips for numbers (incl. inf), bools, strings,
///              objects and Null.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: bit-exact assertions inline; differential suite.
    #[test]
    fn value_roundtrip() {
        let samples = [
            Value::Number(0.0),
            Value::Number(-1.5),
            Value::Number(f64::INFINITY),
            Value::Number(1.0e300),
            Value::Bool(false),
            Value::Bool(true),
            Value::Str(7),
            Value::Obj(0x1234_5678_9ABC),
            Value::Null,
        ];
        for v in samples {
            let w = v.encode();
            match v {
                Value::Number(f) => assert_eq!(Value::decode(w), Value::Number(f)),
                Value::Bool(b) => {
                    assert_eq!(Value::decode_typed(w, TypeTag::Bool), Value::Bool(b))
                }
                _ => assert_eq!(Value::decode(w), v),
            }
        }
    }

    /// CEP:WHAT: The box classifier accepts only NaN-boxed pointers.
/// CEP:WHY: GC barriers and the deoptimizer branch on is_box; doubles and
///              canonical NaNs must classify as non-boxes.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: boundary words asserted inline.
    #[test]
    fn box_classification() {
        assert!(!is_box(0));
        assert!(!is_box(1));
        assert!(!is_box(f64::INFINITY.to_bits()));
        assert!(!is_box(CANONICAL_NAN));
        assert!(is_box(box_word(ITAG_STR, 42)));
        assert_eq!(box_itag(box_word(ITAG_STR, 42)), ITAG_STR);
        assert_eq!(box_payload(box_word(ITAG_STR, 42)), 42);
        assert!(is_box(box_word(ITAG_OBJ, 0x1234)));
    }

    /// CEP:WHAT: canonicalize_f64 maps every NaN to the canonical bit pattern.
/// CEP:WHY: Non-canonical NaNs would collide with the box tag space; every
///              arithmetic creation site canonicalizes.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: bit patterns asserted inline; creation sites listed in
///               the file header.
    #[test]
    fn nan_canonicalization() {
        let neg_nan = f64::from_bits(0xFFF8_0000_0000_0001);
        let canonical = canonicalize_f64(neg_nan);
        assert_eq!(canonical.to_bits(), CANONICAL_NAN);
        assert!(!is_box(canonical.to_bits()));
        assert_eq!(canonicalize_f64(2.5).to_bits(), 2.5f64.to_bits());
    }
}
