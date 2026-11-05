// CEP:FILE: engine/src/hot/stack.rs
// CEP:WHAT: BoundedStack: the fixed-capacity LIFO value stack used by
//           interpreter frames and deopt reconstruction.
// CEP:WHY: CEP-0 loops must not allocate (section 25.5); the backing
//          buffer is a one-time Box at construction (doc/value.md).
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: push returns CapacityFull when full; pop and reads return
//              None at the boundary (Law 6).
// CEP:ASSUMES: capacity is engine-configured and fits memory.
// CEP:COST: push/pop/top/get one compare plus one access; len/is_empty
//           single field read (benches/engine_bench.rs).
// CEP:EVIDENCE: unit tests below; differential suite.
// CEP:OPTIMAL: target-optimal for push/pop/top/get
// CEP:OPTPROOF: one branch and one access; no allocation after
//               construction, cannot be fewer instructions.
// CEP:SECURITY: bounds are retained on every access.
use crate::hot::value::Value;

/// Fixed-capacity LIFO stack of values (no growth, no hot-path allocation).
///
/// The backing buffer is allocated exactly once at construction; push after
/// construction never allocates (section 25.5 arena rule).
pub struct BoundedStack {
    buf: Box<[Value]>,
    len: usize,
}

impl BoundedStack {
    /// Allocate with fixed capacity.
    ///
    /// CEP:WHAT: Constructor with one-time allocation.
    /// CEP:WHY: Allocation before the hot path.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: Fatal at construction only if the host cannot honor
    ///              the reservation (engine config, not input-derived).
    /// CEP:ASSUMES: capacity fits memory.
    /// CEP:COST: one allocation per frame.
    /// CEP:EVIDENCE: differential suite.
    /// CEP:SECURITY: capacity is engine-configured.
    #[must_use]
    pub fn with_capacity(cap: usize) -> BoundedStack {
        BoundedStack { buf: vec![Value::Null; cap].into_boxed_slice(), len: 0 }
    }

    /// Push a value.
    ///
    /// CEP:WHAT: Bounded push.
    /// CEP:WHY: CEP-0 hot loop; overflow is a defined error.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: CapacityFull when full.
    /// CEP:ASSUMES: none
    /// CEP:COST: one compare plus one store.
    /// CEP:EVIDENCE: benches/engine_bench.rs.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: bounds check required by the failure policy.
    /// CEP:SECURITY: bounds retained.
    #[inline]
    pub fn push(&mut self, v: Value) -> Option<()> {
        if self.len >= self.buf.len() {
            return None;
        }
        self.buf[self.len] = v;
        self.len += 1;
        Some(())
    }

    /// Pop a value.
    ///
    /// CEP:WHAT: Bounded pop.
    /// CEP:WHY: Underflow is a defined failure (Law 6).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: None when empty.
    /// CEP:ASSUMES: none
    /// CEP:COST: one compare plus one load.
    /// CEP:EVIDENCE: differential suite.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: one branch and one load.
    /// CEP:SECURITY: none
    #[inline]
    pub fn pop(&mut self) -> Option<Value> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(self.buf[self.len])
    }

    /// Peek the top value.
    ///
    /// CEP:WHAT: Top read.
    /// CEP:WHY: LoadVar reads the top binding.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: None when empty.
    /// CEP:ASSUMES: none
    /// CEP:COST: one compare plus one load.
    /// CEP:EVIDENCE: differential suite.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: one branch and one load.
    /// CEP:SECURITY: none
    #[inline]
    #[must_use]
    pub fn top(&self) -> Option<Value> {
        if self.len == 0 { None } else { Some(self.buf[self.len - 1]) }
    }

    /// Read the element at depth `d` (0 = bottom).
    ///
    /// CEP:WHAT: Depth-indexed read.
    /// CEP:WHY: Deopt reconstruction copies bottom-up.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: None out of range.
    /// CEP:ASSUMES: none
    /// CEP:COST: one compare plus one load.
    /// CEP:EVIDENCE: deopt tests.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: one branch and one load.
    /// CEP:SECURITY: bounds-checked.
    #[inline]
    #[must_use]
    pub fn get(&self, d: usize) -> Option<Value> {
        if d < self.len { Some(self.buf[d]) } else { None }
    }

    /// Current element count.
    ///
    /// CEP:WHAT: Length.
    /// CEP:WHY: Frame migration snapshots.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: one load.
    /// CEP:EVIDENCE: differential suite.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: single field read.
    /// CEP:SECURITY: none
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when empty.
    ///
    /// CEP:WHAT: Emptiness.
    /// CEP:WHY: Statement-boundary detection.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: one compare.
    /// CEP:EVIDENCE: differential suite.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: single compare.
    /// CEP:SECURITY: none
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CEP:WHAT: BoundedStack boundary semantics.
/// CEP:WHY: Push at capacity returns None, pop/get at the boundary return
///              None, and LIFO order holds — the CEP-0 contract.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: assertions inline; differential suite.
    #[test]
    fn bounded_stack_semantics() {
        let mut s = BoundedStack::with_capacity(2);
        assert!(s.is_empty());
        assert!(s.push(Value::Number(1.0)).is_some());
        assert!(s.push(Value::Bool(true)).is_some());
        assert!(s.push(Value::Null).is_none());
        assert_eq!(s.top(), Some(Value::Bool(true)));
        assert_eq!(s.pop(), Some(Value::Bool(true)));
        assert_eq!(s.get(0), Some(Value::Number(1.0)));
        assert_eq!(s.pop(), Some(Value::Number(1.0)));
        assert_eq!(s.pop(), None);
    }
}
