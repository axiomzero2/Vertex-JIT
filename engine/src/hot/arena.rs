// CEP:FILE: engine/src/hot/arena.rs
// CEP:WHAT: One mmap'd generation arena: bump/free-list allocation, the
//           object-start bitmap, mark bits, aging and the walk/sweep
//           primitives the collector drives.
// CEP:WHY: Spec section 7: the generations are raw anonymous mappings
//          with a start-bitmap object grammar (doc/gc.md); keeping the
//          arena mechanics separate from the Heap facade keeps the
//          address-arithmetic surface auditable.
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: map and alloc return EngineError (mmap failure, OOM);
//              every pointer query answers false outside the span.
// CEP:ASSUMES: Linux mmap/munmap semantics; 8-byte slots; single-mutator
//              access (doc/gc.md).
// CEP:COST: alloc is bump or first-fit over the free list; walk is
//           O(objects); sweep O(slots) over the bitmaps.
// CEP:EVIDENCE: tests/gc_tests.rs; sanitizer runs in CI.
// CEP:SECURITY: pointers are range-checked against the span before every
//               dereference; foreign words are never dereferenced.

use crate::cold::errors::EngineError;
use crate::hot::heap::HEADER_BYTES;

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
}

const PROT_READ_WRITE: i32 = 0x3;
const MAP_PRIVATE_ANON: i32 = 0x22;
const MAP_FAILED: isize = -1;

/// One arena backing a generation.
pub struct Arena {
    pub(crate) base: *mut u8,
    pub(crate) len: usize,
    /// Young only: next free offset in the virgin region.
    pub(crate) bump: usize,
    /// Free list of (offset, size) runs sorted by offset.
    pub(crate) free: Vec<(u32, u32)>,
    /// Object-start bitmap: one bit per 8-byte slot. The walk iterates the
    /// bitmap, so free-list splitting cannot break the header chain.
    pub(crate) starts: Box<[u64]>,
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
    pub(crate) fn map(len: usize) -> Result<Arena, EngineError> {
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
    pub(crate) fn contains(&self, ptr: usize) -> bool {
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
    pub(crate) fn alloc(&mut self, size: usize) -> Result<usize, EngineError> {
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

    /// Mark the object start at a slot offset.
/// CEP:WHAT: Object-start bit set.
/// CEP:WHY: The walk and root filters trust the bitmap;
///              alloc_array writes one start bit per object.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: one ORW.
/// CEP:EVIDENCE: tests/gc_tests.rs walk invariants.
    #[inline]
    pub(crate) fn set_start(&mut self, off: usize) {
        let bit = off / 8;
        self.starts[bit / 64] |= 1u64 << (bit % 64);
    }

    /// Clear the object start bit at a slot offset (sweep reclaim).
    ///
    /// CEP:WHAT: Object-start bit clear.
    /// CEP:WHY: Sweep reclamation must retire the start bit together with
    ///          the free-list run.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none.
    /// CEP:ASSUMES: the offset is inside the span.
    /// CEP:COST: one ANDW.
    /// CEP:EVIDENCE: tests/gc_tests.rs sweep cases.
    #[inline]
    pub(crate) fn clear_start(&mut self, off: usize) {
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
    pub(crate) fn walk(&self, f: &mut dyn FnMut(usize, u64, usize)) {
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
    pub(crate) fn clear_marks(&mut self) {
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
    pub(crate) fn marked_bytes(&self) -> u64 {
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
    pub(crate) fn sweep(&mut self) -> u64 {
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
