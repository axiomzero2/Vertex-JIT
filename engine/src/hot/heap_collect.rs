// CEP:FILE: engine/src/hot/heap_collect.rs
// CEP:WHAT: Heap-side collection surface: mark-bit operations, object
//           aging, generational sweeps, occupancy queries, object walks
//           and the root-word filter.
// CEP:WHY: The collector (cold/gc.rs) drives these primitives; keeping
//          them out of the allocation fast-path file keeps the CEP-0
//          barrier path auditable apart from collection policy.
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: sweep_* report reclaimed bytes; queries answer false/None
//              outside the arenas; no failure path beyond that.
// CEP:ASSUMES: headers written only by alloc_array; single-mutator
//              access (doc/gc.md).
// CEP:COST: mark/age one bit op; sweeps O(slots); walk O(objects).
// CEP:EVIDENCE: tests/gc_tests.rs.
// CEP:SECURITY: foreign words are ignored, never dereferenced.

use crate::hot::heap::Heap;
use crate::hot::value::{box_itag, is_box, ITAG_OBJ};

impl Heap {
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
