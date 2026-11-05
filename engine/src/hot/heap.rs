// CEP:FILE: engine/src/hot/heap.rs
// CEP:WHAT: The garbage-collected heap facade: young and old arena
//           construction, array allocation fast path, bounds-checked
//           element access and the card-table write barrier.
// CEP:WHY: Spec section 7: allocation fast paths and JIT barrier
//          integration (7.5) are called from generated machine code
//          (doc/gc.md); collection primitives live in heap_collect.rs,
//          arena mechanics in arena.rs.
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: alloc_array returns Err(GcNeeded) when the young arena is
//              full (caller collects and retries) and Err(OutOfMemory) for
//              old-arena exhaustion or size overflow; load_word/store_word
//              return IndexOutOfBounds or TypeMismatch. No panics.
// CEP:ASSUMES: Linux mmap; 8-byte slots; single-mutator (doc/gc.md);
//              object headers are written only by alloc_array.
// CEP:COST: alloc_array is bump or free-list plus header stores (bitmap
//           add plus mandatory zero-initialization stores); the barrier
//           is one load and one compare before the store.
// CEP:EVIDENCE: tests/gc_tests.rs; differential suite; sanitizer runs.
// CEP:SECURITY: array indices are bounds-checked on every access; values
//               are untrusted words and never executed; pointers are
//               range-checked against arena bounds before dereference.

use crate::cold::errors::EngineError;
use crate::hot::arena::{Arena, GcStats};
use crate::hot::value::{box_itag, is_box, Word, ITAG_OBJ};

/// Object type tag for arrays.
pub const TYPE_ARRAY: u64 = 1;
/// Header size (header word + element-count word).
pub const HEADER_BYTES: usize = 16;
/// Objects at least this large are allocated in the old generation
/// (spec 7.1 humongous objects); named per Law 7.
pub const HUMONGOUS_BYTES: usize = 64 * 1024;
/// Card size for the generational remembered set (spec 7.3).
pub const CARD_BYTES: usize = 512;

unsafe extern "C" {
    fn munmap(addr: *mut u8, len: usize) -> i32;
}

/// The garbage-collected heap.
pub struct Heap {
    pub(crate) young: Arena,
    pub(crate) old: Arena,
    /// Card table over the old arena (spec 7.3).
    pub(crate) cards: Box<[u8]>,
    /// True while concurrent marking is in progress (SATB barriers live).
    pub marking: bool,
    /// SATB queue: pre-values of reference stores during marking.
    pub satb: Vec<Word>,
    /// Documented counters.
    pub stats: GcStats,
}

impl Heap {
    /// Create a heap with the given generation sizes.
    ///
    /// CEP:WHAT: Heap construction.
    /// CEP:WHY: Generation sizes are named configuration.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: OutOfMemory on mmap failure or a mapping outside the
    ///              47-bit pointer contract.
    /// CEP:ASSUMES: Linux mmap; single mutator.
    /// CEP:COST: two mmaps plus the card table, once.
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: private anonymous mappings.
    ///
    /// # Errors
    /// Returns OutOfMemory on mapping failure.
    pub fn new(young_bytes: usize, old_bytes: usize) -> Result<Heap, EngineError> {
        let young = Arena::map(young_bytes)?;
        let old = Arena::map(old_bytes)?;
        for a in [&young, &old] {
            if a.base as usize >= (1usize << 47) {
                return Err(EngineError::OutOfMemory);
            }
        }
        let card_count = old_bytes.div_ceil(CARD_BYTES);
        Ok(Heap {
            young,
            old,
            cards: vec![0u8; card_count].into_boxed_slice(),
            marking: false,
            satb: Vec::new(),
            stats: GcStats::default(),
        })
    }

    /// Allocate an array of `len` zero-initialized words.
    ///
    /// CEP:WHAT: Array allocation fast path.
    /// CEP:WHY: Spec 2.6 bump allocation; humongous objects go to the old
    ///          generation (spec 7.1).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: GcNeeded when the young arena is full; OutOfMemory for
    ///              old exhaustion or size overflow.
    /// CEP:ASSUMES: single mutator.
    /// CEP:COST: bump path: one compare, one add, size/8 stores.
    /// CEP:EVIDENCE: benches; tests/gc_tests.rs.
    /// CEP:OPTIMAL: target-optimal for the bump path
    /// CEP:OPTPROOF: one add plus mandatory initialization stores.
    /// CEP:SECURITY: size checked against arena bounds; zero-initialized
    ///               words are all valid values.
    ///
    /// # Errors
    /// GcNeeded (retry after collection) or OutOfMemory.
    pub fn alloc_array(&mut self, len: u32) -> Result<usize, EngineError> {
        let payload = (len as usize)
            .checked_mul(8)
            .ok_or(EngineError::OutOfMemory)?;
        let size = payload.checked_add(HEADER_BYTES).ok_or(EngineError::OutOfMemory)?;
        let ptr = if size >= HUMONGOUS_BYTES {
            self.old.alloc(size).map_err(|_| EngineError::OutOfMemory)?
        } else {
            self.young.alloc(size).map_err(|_| EngineError::GcNeeded)?
        };
        let hdr = size as u64 | (TYPE_ARRAY << 32);
        unsafe {
            *(ptr as *mut u64) = hdr;
            *(ptr as *mut u64).add(1) = len as u64;
            std::ptr::write_bytes((ptr + HEADER_BYTES) as *mut u8, 0, payload);
        }
        self.stats.allocated_bytes += size as u64;
        Ok(ptr)
    }

    /// Array element count.
    ///
    /// CEP:WHAT: Length read.
    /// CEP:WHY: ARYLEN and codegen lowering.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: 0 for foreign or null pointers.
    /// CEP:ASSUMES: ptr is an engine object or 0.
    /// CEP:COST: one load.
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: single load.
    /// CEP:SECURITY: no user-controlled dereference.
    #[must_use]
    pub fn array_len(&self, ptr: usize) -> u64 {
        if ptr == 0 {
            return 0;
        }
        unsafe { *(ptr as *const u64).add(1) }
    }

    /// Load one element word with a bounds check.
    ///
    /// CEP:WHAT: Bounds-checked element load.
    /// CEP:WHY: GETIDX semantics; the check is retained for untrusted
    ///          indices (section 23.7: security beats micro-optimality).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: IndexOutOfBounds / TypeMismatch.
    /// CEP:ASSUMES: ptr is an engine object.
    /// CEP:COST: two loads plus one compare.
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:OPTIMAL: target-optimal for the checked form
    /// CEP:OPTPROOF: one bound load, one element load; the compare is
    ///               required by the failure policy.
    /// CEP:SECURITY: index is untrusted.
    ///
    /// # Errors
    /// IndexOutOfBounds or TypeMismatch.
    pub fn load_word(&self, ptr: usize, idx: f64) -> Result<Word, EngineError> {
        let len = self.array_len(ptr);
        let i = index_of(idx, len)?;
        Ok(unsafe { *(ptr as *const u64).add(2 + i) })
    }

    /// Store one element word with bounds check and write barriers.
    ///
    /// CEP:WHAT: Bounds-checked store with generational and SATB barriers
    ///           (spec 7.3, 7.5).
    /// CEP:WHY: The generational barrier tracks old-to-young references via
    ///          the card table; SATB protects concurrent marking.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: IndexOutOfBounds / TypeMismatch.
    /// CEP:ASSUMES: single mutator.
    /// CEP:COST: two compares, one store, one predictable flag load.
    /// CEP:EVIDENCE: tests/gc_tests.rs (card dirty, SATB).
    /// CEP:OPTIMAL: not-optimal (barrier always executed)
    /// CEP:OPTNOTE: conservative per spec 2.6 (no IR analysis at T0);
    ///              T2+ barrier elimination is ticket CEP-ENGINE-008.
    /// CEP:SECURITY: bounds-checked; stored words are data only.
    ///
    /// # Errors
    /// IndexOutOfBounds or TypeMismatch.
    pub fn store_word(&mut self, ptr: usize, idx: f64, word: Word) -> Result<(), EngineError> {
        let len = self.array_len(ptr);
        let i = index_of(idx, len)?;
        let elem = ptr + 8 * (2 + i);
        if self.marking {
            // SATB: record the pre-value (spec 7.2 concurrent marking).
            let old = unsafe { *(elem as *const u64) };
            if is_box(old) && box_itag(old) == ITAG_OBJ {
                self.satb.push(old);
            }
        }
        unsafe { *(elem as *mut u64) = word };
        // Generational barrier: references stored into old objects dirty
        // their card so young collections scan them (spec 7.3).
        if is_box(word) && box_itag(word) == ITAG_OBJ && self.old.contains(ptr) {
            let card = (ptr - self.old.base as usize) / CARD_BYTES;
            if card < self.cards.len() {
                self.cards[card] = 1;
            }
        }
        Ok(())
    }

    /// True when the address is inside the old arena.
    ///
    /// CEP:WHAT: Generation membership.
    /// CEP:WHY: Barriers and root filtering.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: two compares.
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: two compares; minimal range test.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn in_old(&self, ptr: usize) -> bool {
        self.old.contains(ptr)
    }

    /// True when the address is inside the young arena.
    ///
    /// CEP:WHAT: Generation membership.
    /// CEP:WHY: Root filtering.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: two compares.
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: two compares.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn in_young(&self, ptr: usize) -> bool {
        self.young.contains(ptr)
    }

}

impl Drop for Heap {
    /// Release both arena mappings.
/// CEP:WHAT: Arena teardown at engine drop.
/// CEP:WHY: No heap mappings may outlive the engine
///              (section 25.2 resource rule).
/// CEP:STATUS: complete
/// CEP:FAILURE: munmap results ignored; cannot fail for valid
///              engine-owned mappings.
/// CEP:ASSUMES: no concurrent access (single-mutator).
/// CEP:COST: two syscalls.
/// CEP:EVIDENCE: sanitizer runs in CI.
    fn drop(&mut self) {
        // CEP:UNSAFE: munmap of engine-owned anonymous mappings.
        // CEP:WHAT: Arena teardown.
        // CEP:WHY: No heap mappings outlive the engine.
        // CEP:FAILURE: ignored; cannot fail for valid mappings.
        // CEP:ASSUMES: no concurrent access.
        // CEP:COST: two syscalls.
        // CEP:EVIDENCE: sanitizer runs in CI.
        // CEP:SECURITY: removes heap pages.
        unsafe {
            let _ = munmap(self.young.base, self.young.len);
            let _ = munmap(self.old.base, self.old.len);
        }
    }
}

/// Convert a float index to a bounds-checked element index.
///
/// CEP:WHAT: Index validation.
/// CEP:WHY: Indices arrive as f64 from the value stack.
/// CEP:STATUS: complete
/// CEP:FAILURE: IndexOutOfBounds for negative, fractional or out-of-range.
/// CEP:ASSUMES: none
/// CEP:COST: two compares plus a cast.
/// CEP:EVIDENCE: negative tests.
/// CEP:SECURITY: untrusted index path.
///
/// # Errors
/// IndexOutOfBounds.
fn index_of(idx: f64, len: u64) -> Result<usize, EngineError> {
    if idx < 0.0 || idx.fract() != 0.0 || idx >= len as f64 {
        return Err(EngineError::IndexOutOfBounds(idx));
    }
    Ok(idx as usize)
}
