# Value Representation

## Abstract

Every runtime value is one 64-bit word plus a static type tag. The
interpreter stores `Value { word, tag }`; compiled tiers move raw words and
recover the tag from the compiled function's static return type
(`T0Code::return_tag`, `CompiledCode::return_tag`). The representation is
shared by the interpreter, T0 and the IR tiers, so a change here invalidates
every tier at once.

## Formal Definitions

A word is 64 bits. A tag is one of `Number`, `Bool`, `String`, `Null`
(`TypeTag`). Numbers are IEEE-754 binary64 stored in host order. Booleans
are words `0` and `1`. Strings are boxed: the word carries an interned
handle with a tag prefix (`box_word(ITAG_STR, id)`).

NaN canonicalization: any double whose bit pattern is a NaN is replaced by
`CANONICAL_NAN` after arithmetic. Two values compare equal only when both
word and tag agree, which makes NaN equality deterministic across tiers.

## Algorithm

`Value::decode_typed(word, tag)` reconstructs a `Value` from a machine word
and the static tag of the return site. Arithmetic helpers canonicalize
results; comparisons produce canonical booleans. The T0 DIV template and the
interpreter's division path both route through the same canonicalization
constant, which the differential suite checks bit-for-bit.

## Invariants

1. A canonical NaN is the only NaN a caller can observe after an operation.
2. Booleans are exactly the words 0 and 1; every producer normalizes to
   these before storage.
3. The halt value of a function is decoded with the static tag computed by
   the tier that produced it; the T0 lattice and the IR value types must
   agree for any program both accept (see doc/differential.md).
4. Raw words are ambiguous without a tag; no component interprets a word
   without its tag.

## Integration Points

`hot::value` is used by the interpreter, the heap (boxing), the runtime
helpers (canonicalization) and both code generators. `doc/tier0.md` documents
the lattice that mirrors these tags at T0, `doc/codegen.md` the tag
construction at T2/T3.

## Known Limitations

1. Tagged words are not self-describing; a lost tag is unrecoverable. The
   deopt records carry tags for exactly this reason (doc/deopt.md).
2. String interning is engine-lifetime; there is no collection of interned
   strings yet (ticket CEP-ENGINE-015).
