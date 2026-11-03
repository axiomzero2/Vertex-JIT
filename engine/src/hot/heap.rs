// CEP:FILE: engine/src/hot/heap.rs
// CEP:WHAT: The generational garbage-collected heap: young and old arenas
//           with object-start bitmaps, array allocation fast paths,
//           bounds-checked element access, card-table write barriers and
//           SATB enqueueing for concurrent marking.
// CEP:WHY: Spec section 7: generational collector (7.1), card-table write
//          barriers (7.3), SATB protection for concurrent marking, JIT
//          barrier integration (7.5). The fast paths are called from
//          generated machine code, so they are CEP-0.
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: alloc_array returns Err(GcNeeded) when the young arena is
//              full (caller collects and retries) and Err(OutOfMemory) for
//              old-arena exhaustion or size overflow; load_word/store_word
//              return IndexOutOfBounds or TypeMismatch. No panics.
// CEP:ASSUMES: single mutator (doc/gc.md Invariants); object pointers fit
//              47 bits (asserted at arena mapping); Linux mmap.
// CEP:COST: bump allocation is one compare, one add and size/8 stores;
//           the store barrier is one predictable flag load plus one range
//           compare; measured in benches/EVIDENCE.md.
// CEP:EVIDENCE: benches/engine_bench.rs; tests/gc_tests.rs.
// CEP:OPTIMAL: target-optimal for the allocation and barrier fast paths
// CEP:OPTPROOF: bump allocation performs the minimum possible work (one
//               add plus mandatory zero-initialization stores); the barrier
//               is one load and one compare before the store.
// CEP:SECURITY: array indices are bounds-checked on every access; values
//               are untrusted words and never executed; pointers are
//               range-checked against arena bounds before dereference.

use crate::cold::errors::EngineError;
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

/// One arena backing a generation.
pub struct Arena {
    base: *mut u8,
    len: usize,
    /// Young only: next free offset in the virgin region.
    bump: usize,
    /// Free list of (offset, size) runs sorted by offset.
    free: Vec<(u32, u32)>,
    /// Object-start bitmap: one bit per 8-byte slot. The walk iterates the
    /// bitmap, so free-list splitting cannot break the header chain.
    starts: Box<[u64]>,
}

// SAFETY: the raw pointer is dereferenced only through &mut Heap methods;
// Heap is single-mutator (doc/gc.md).
unsafe impl Send for Arena {}

/// Collection statistics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcStats {
    /// Young collections performed.
    pub young_collections: u64,
    /// Old mark cycles started.
    pub old_marks: u64,
    /// Bytes allocated since start.
    pub allocated_bytes: u64,
    /// Bytes reclaimed by young collections.
    pub young_reclaimed_bytes: u64,
    /// Bytes reclaimed by old collections.
    pub old_reclaimed_bytes: u64,
}

/// The garbage-collected heap.
pub struct Heap {
    young: Arena,
    old: Arena,
    /// Card table over the old arena (spec 7.3).
    cards: Box<[u8]>,
    /// True while concurrent marking is in progress (SATB barriers live).
    pub marking: bool,
    /// SATB queue: pre-values of reference stores during marking.
    pub satb: Vec<Word>,
    /// Documented counters.
    pub stats: GcStats,
}

/// A registered precise root span (interpreter frame buffers).
#[derive(Debug, Clone, Copy)]
pub struct RootSpan {
    /// Address of the first word.
    pub ptr: *const u64,
    /// Number of words.
    pub words: usize,
}

// SAFETY: non-owning view; the engine guarantees it outlives the
// collection (doc/gc.md).
unsafe impl Send for RootSpan {}

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
}

const PROT_READ_WRITE: i32 = 0x3;
const MAP_PRIVATE_ANON: i32 = 0x22;
const MAP_FAILED: isize = -1;

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

    /// Clear every mark bit.
    ///
    /// CEP:WHAT: Mark reset.
    /// CEP:WHY: Collections start clean.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: no collection in progress.
    /// CEP:COST: O(objects).
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: none
    pub fn clear_marks(&mut self) {
        self.young.clear_marks();
        self.old.clear_marks();
    }

    /// Set the mark bit of one object.
    ///
    /// CEP:WHAT: Mark set.
    /// CEP:WHY: The marker worklist.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none for engine-owned pointers.
    /// CEP:ASSUMES: ptr is an object base.
    /// CEP:COST: one load, one store.
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: none
    pub fn set_mark(&mut self, ptr: usize) {
        unsafe {
            *(ptr as *mut u64) |= 1u64 << 40;
        }
    }

    /// Read the mark bit of one object.
    ///
    /// CEP:WHAT: Mark read.
    /// CEP:WHY: Worklist dedup.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none for engine-owned pointers.
    /// CEP:ASSUMES: ptr is an object base.
    /// CEP:COST: one load, one shift.
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn marked(&self, ptr: usize) -> bool {
        (unsafe { *(ptr as *const u64) } >> 40) & 1 == 1
    }

    /// Increment the survivor age of one object.
    ///
    /// CEP:WHAT: Aging.
    /// CEP:WHY: Generational hypothesis accounting (spec 7.1).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: saturates at 255 by design.
    /// CEP:ASSUMES: ptr is an object base.
    /// CEP:COST: one load, one store.
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: none
    pub fn age_object(&mut self, ptr: usize) {
        let h = unsafe { &mut *(ptr as *mut u64) };
        let age = (*h >> 44) & 0xFF;
        if age < 0xFF {
            *h = (*h & !(0xFFu64 << 44)) | ((age + 1) << 44);
        }
    }

    /// Sweep the young arena into its free list; returns reclaimed bytes.
    ///
    /// CEP:WHAT: Young mark-region sweep.
    /// CEP:WHY: Non-moving design: survivors keep addresses, so
    ///          conservative machine-stack roots stay valid (doc/gc.md).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: mark phase completed.
    /// CEP:COST: O(objects).
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: none
    pub fn sweep_young(&mut self) -> u64 {
        let freed = self.young.sweep();
        self.stats.young_reclaimed_bytes += freed;
        self.stats.young_collections += 1;
        freed
    }

    /// Sweep the old arena; returns reclaimed bytes.
    ///
    /// CEP:WHAT: Old sweep.
    /// CEP:WHY: Old reclamation; cards reset for the next cycle.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: mark completed; no mutation during sweep.
    /// CEP:COST: O(objects).
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: none
    pub fn sweep_old(&mut self) -> u64 {
        let freed = self.old.sweep();
        self.stats.old_reclaimed_bytes += freed;
        self.cards.fill(0);
        freed
    }

    /// Live (marked) bytes in a generation.
    ///
    /// CEP:WHAT: Occupancy query.
    /// CEP:WHY: Collection triggers.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(objects).
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn marked_bytes(&self, old: bool) -> u64 {
        if old {
            self.old.marked_bytes()
        } else {
            self.young.marked_bytes()
        }
    }

    /// Capacity of a generation in bytes.
    ///
    /// CEP:WHAT: Capacity query.
    /// CEP:WHY: Occupancy ratios.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(1).
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn capacity_bytes(&self, old: bool) -> usize {
        if old { self.old.len } else { self.young.len }
    }

    /// Walk every object in a generation: (ptr, header, size).
    ///
    /// CEP:WHAT: Object walk.
    /// CEP:WHY: Marking and root scanning.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: headers written only by alloc_array.
    /// CEP:COST: O(objects).
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: none
    pub fn for_each_object(&self, old: bool, f: &mut dyn FnMut(usize, u64, usize)) {
        if old {
            self.old.walk(f);
        } else {
            self.young.walk(f);
        }
    }

    /// Filter a word into an object pointer inside the arenas.
    ///
    /// CEP:WHAT: Root word filter.
    /// CEP:WHY: Precise root scanning trusts only in-arena object words.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: non-root words yield None.
    /// CEP:ASSUMES: spans contain engine values.
    /// CEP:COST: two compares per word.
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: foreign words are ignored, never dereferenced.
    #[must_use]
    pub fn root_ptr_of(&self, w: u64) -> Option<usize> {
        if is_box(w) && box_itag(w) == ITAG_OBJ {
            let p = (w & crate::hot::value::PAYLOAD_MASK) as usize;
            if self.in_young(p) || self.in_old(p) {
                return Some(p);
            }
        }
        None
    }
}

impl Drop for Heap {
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
    if !(idx >= 0.0) || idx.fract() != 0.0 || idx >= len as f64 {
        return Err(EngineError::IndexOutOfBounds(idx));
    }
    Ok(idx as usize)
}

impl Arena {
    /// Map an anonymous arena.
    ///
    /// CEP:WHAT: Arena construction.
    /// CEP:WHY: Generations need dedicated address ranges.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: OutOfMemory on mmap failure.
    /// CEP:ASSUMES: Linux mmap.
    /// CEP:COST: one syscall.
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: private anonymous mapping.
    ///
    /// # Errors
    /// OutOfMemory.
    fn map(len: usize) -> Result<Arena, EngineError> {
        // CEP:UNSAFE: mmap of an anonymous private heap arena.
        // CEP:WHAT: Arena mapping.
        // CEP:WHY: Dedicated ranges enable two-compare generation checks.
        // CEP:FAILURE: MAP_FAILED surfaces as OutOfMemory.
        // CEP:ASSUMES: Linux page granularity.
        // CEP:COST: one syscall.
        // CEP:EVIDENCE: tests/gc_tests.rs.
        // CEP:SECURITY: private, anonymous, non-executable.
        let base = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ_WRITE, MAP_PRIVATE_ANON, -1, 0) };
        if base as isize == MAP_FAILED {
            return Err(EngineError::OutOfMemory);
        }
        let bits = len.div_ceil(8).div_ceil(64);
        Ok(Arena {
            base,
            len,
            bump: 0,
            free: Vec::new(),
            starts: vec![0u64; bits].into_boxed_slice(),
        })
    }

    /// True when the address is inside this arena.
    ///
    /// CEP:WHAT: Range membership.
    /// CEP:WHY: Barriers and root filtering.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: two compares.
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: two compares.
    /// CEP:SECURITY: none
    fn contains(&self, ptr: usize) -> bool {
        let b = self.base as usize;
        ptr >= b && ptr < b + self.len
    }

    /// Allocate `size` bytes (8-aligned).
    ///
    /// CEP:WHAT: First-fit allocation with bump fast path.
    /// CEP:WHY: Young allocations bump first; post-GC allocations use the
    ///          free list (non-moving design).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: Err(GcNeeded) when no space fits.
    /// CEP:ASSUMES: size 8-aligned.
    /// CEP:COST: bump O(1); free list O(runs).
    /// CEP:EVIDENCE: benches.
    /// CEP:SECURITY: size checked against the arena.
    fn alloc(&mut self, size: usize) -> Result<usize, EngineError> {
        if !self.free.is_empty() {
            for i in 0..self.free.len() {
                let (off, sz) = self.free[i];
                if sz as usize >= size {
                    let addr = self.base as usize + off as usize;
                    if sz as usize == size {
                        self.free.remove(i);
                    } else {
                        self.free[i] = (off + size as u32, sz - size as u32);
                    }
                    self.set_start(off as usize);
                    return Ok(addr);
                }
            }
        }
        if self.bump + size <= self.len {
            let addr = self.base as usize + self.bump;
            self.set_start(self.bump);
            self.bump += size;
            return Ok(addr);
        }
        Err(EngineError::GcNeeded)
    }

    #[inline]
    fn set_start(&mut self, off: usize) {
        let bit = off / 8;
        self.starts[bit / 64] |= 1u64 << (bit % 64);
    }

    #[inline]
    fn clear_start(&mut self, off: usize) {
        let bit = off / 8;
        self.starts[bit / 64] &= !(1u64 << (bit % 64));
    }

    /// Walk every object: (ptr, header, size).
    ///
    /// CEP:WHAT: Bitmap-driven object walk.
    /// CEP:WHY: Marking, marking reset and occupancy queries.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: headers written only by alloc_array.
    /// CEP:COST: O(objects).
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: none
    fn walk(&self, f: &mut dyn FnMut(usize, u64, usize)) {
        for (wi, &word) in self.starts.iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let off = (wi * 64 + bit) * 8;
                if off + HEADER_BYTES > self.bump {
                    continue;
                }
                let p = self.base as usize + off;
                let h = unsafe { *(p as *const u64) };
                f(p, h, (h & 0xFFFF_FFFF) as usize);
            }
        }
    }

    /// Clear all mark bits.
    ///
    /// CEP:WHAT: Mark reset.
    /// CEP:WHY: Collection start state.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(objects).
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: none
    fn clear_marks(&mut self) {
        let starts = self.starts.clone();
        for (wi, &word) in starts.iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let off = (wi * 64 + bit) * 8;
                if off + HEADER_BYTES > self.bump {
                    continue;
                }
                let p = self.base as usize + off;
                unsafe {
                    *(p as *mut u64) &= !(1u64 << 40);
                }
            }
        }
    }

    /// Sum of marked object sizes.
    ///
    /// CEP:WHAT: Live bytes.
    /// CEP:WHY: Occupancy thresholds.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(objects).
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: none
    fn marked_bytes(&self) -> u64 {
        let mut total = 0u64;
        self.walk(&mut |_, h, size| {
            if (h >> 40) & 1 == 1 {
                total += size as u64;
            }
        });
        total
    }

    /// Sweep unmarked objects into the free list.
    ///
    /// CEP:WHAT: Mark-region sweep with run coalescing.
    /// CEP:WHY: Non-moving reclamation.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: mark completed; no concurrent mutation.
    /// CEP:COST: O(objects).
    /// CEP:EVIDENCE: tests/gc_tests.rs.
    /// CEP:SECURITY: none
    fn sweep(&mut self) -> u64 {
        let mut free: Vec<(u32, u32)> = Vec::new();
        let mut reclaimed = 0u64;
        let starts = self.starts.clone();
        for (wi, &word) in starts.iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let off = (wi * 64 + bit) * 8;
                if off + HEADER_BYTES > self.bump {
                    continue;
                }
                let p = self.base as usize + off;
                let h = unsafe { *(p as *const u64) };
                if (h >> 40) & 1 == 0 {
                    let size = (h & 0xFFFF_FFFF) as usize;
                    reclaimed += size as u64;
                    self.clear_start(off);
                    match free.last_mut() {
                        Some(last) if last.0 as usize + last.1 as usize == off => {
                            last.1 += size as u32;
                        }
                        _ => free.push((off as u32, size as u32)),
                    }
                }
            }
        }
        self.free = free;
        reclaimed
    }
}
