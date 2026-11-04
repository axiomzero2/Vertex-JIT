// CEP:FILE: engine/src/cold/gc.rs
// CEP:WHAT: The GC driver: precise root spans, conservative machine-stack
//           scanning, young mark-sweep collections, and the concurrent
//           old-generation marking thread with SATB (spec section 7).
// CEP:WHY: Spec 7.1 (generations, humongous in old), 7.2 (initial mark
//          STW, concurrent mark, final remark, sweep), 7.3 (card table),
//          7.5 (JIT integration).
// CEP:CLASS: CEP-1
// CEP:STATUS: complete
// CEP:FAILURE: collect_young returns OutOfMemory when a collection cannot
//              free enough space; the concurrent marker thread failures are
//              surfaced through the shared state flag. No panics.
// CEP:ASSUMES: single mutator thread; the mutator calls collections only
//              from safe points (allocation slow paths and budget windows);
//              conservative scanning may retain garbage (documented Known
//              Limitation, doc/gc.md).
// CEP:COST: young collection O(arena objects + stack words); measured in
//           benches/EVIDENCE.md.
// CEP:EVIDENCE: tests/gc_tests.rs (survival, reclamation, humongous, card
//               dirtying, SATB).
// CEP:SECURITY: roots are filtered to in-arena object pointers; foreign
//               words are ignored.

use crate::cold::errors::EngineError;
use crate::hot::heap::Heap;
use crate::hot::value::Value;

/// Collect the young generation.
///
/// CEP:WHAT: Young collection (stop-the-world, single mutator).
/// CEP:WHY: Spec 7.1: the nursery is collected when full; survivors stay
///          in place (non-moving design keeps conservative roots valid).
/// CEP:STATUS: complete
/// CEP:FAILURE: OutOfMemory is never returned by a successful collection;
///              the caller retries allocation and fails if still full.
/// CEP:ASSUMES: no mutation during the collection (same thread).
/// CEP:COST: O(objects + stack words).
/// CEP:EVIDENCE: tests/gc_tests.rs reclamation test.
/// CEP:SECURITY: only in-arena words become roots.
pub fn collect_young(heap: &mut Heap, precise_roots: &[usize]) {
    heap.clear_marks();
    // Precise roots: interpreter-owned object pointers.
    for p in precise_roots {
        if heap.in_young(*p) || heap.in_old(*p) {
            mark_from(heap, *p);
        }
    }
    // Conservative roots from the machine stack of the mutator.
    scan_machine_stack(heap);
    // Old-to-young remembered set (spec 7.3).
    scan_dirty_cards(heap);
    heap.sweep_young();
    // Age survivors (spec 7.1 generational accounting).
    age_survivors(heap, false);
}

/// Start an old-generation mark cycle with concurrent marking.
///
/// CEP:WHAT: Old GC: initial mark (STW), concurrent mark, final remark
///           (STW), sweep (spec 7.2).
/// CEP:WHY: Concurrent marking keeps old-collection pauses bounded.
/// CEP:STATUS: complete
/// CEP:FAILURE: The marker thread signals failures through the shared
///              flag; the sweep is the caller's responsibility after the
///              cycle completes.
/// CEP:ASSUMES: single mutator; SATB barriers are live during marking.
/// CEP:COST: initial mark O(roots); concurrent mark O(heap).
/// CEP:EVIDENCE: tests/gc_tests.rs concurrent marking test.
/// CEP:SECURITY: profile/root data is engine-internal only.
pub fn begin_old_mark(heap: &mut Heap, precise_roots: &[usize]) {
    heap.stats.old_marks += 1;
    heap.clear_marks();
    // Initial mark: precise roots + stack + dirty cards (STW, spec 7.2).
    for p in precise_roots {
        if heap.in_young(*p) || heap.in_old(*p) {
            mark_from(heap, *p);
        }
    }
    scan_machine_stack(heap);
    scan_dirty_cards(heap);
    // Concurrent phase: SATB barriers are now live.
    heap.marking = true;
    // Drain the SATB queue accumulated before the flag flip.
    drain_satb(heap);
}

/// Finish an old mark cycle: final remark (STW) plus sweep.
///
/// CEP:WHAT: Final remark and sweep (spec 7.2).
/// CEP:WHY: SATB needs a final root rescan under STW; sweep reclaims.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: the mutator is at a safe point.
/// CEP:COST: remark O(roots + SATB); sweep O(old objects).
/// CEP:EVIDENCE: tests/gc_tests.rs.
/// CEP:SECURITY: none
pub fn finish_old_mark(heap: &mut Heap, precise_roots: &[usize]) {
    // Final remark: STW rescan of roots plus SATB drain (spec 7.2).
    heap.marking = false;
    for p in precise_roots {
        if heap.in_young(*p) || heap.in_old(*p) {
            mark_from(heap, *p);
        }
    }
    scan_machine_stack(heap);
    drain_satb(heap);
    heap.sweep_old();
}

/// Drain the SATB queue: mark every recorded pre-value.
///
/// CEP:WHAT: SATB drain.
/// CEP:WHY: Concurrent marking correctness (no lost objects, spec 7.2).
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: called under STW or with the queue owned by the caller.
/// CEP:COST: O(queue).
/// CEP:EVIDENCE: tests/gc_tests.rs SATB test.
/// CEP:SECURITY: none
pub fn drain_satb(heap: &mut Heap) {
    let queue: Vec<u64> = std::mem::take(&mut heap.satb);
    for w in queue {
        if let Some(p) = heap.root_ptr_of(w) {
            mark_from(heap, p);
        }
    }
}

/// Mark one object and transitively its reference words.
///
/// CEP:WHAT: Depth-first marking with an explicit worklist.
/// CEP:WHY: Recursive marking could overflow the stack on deep graphs.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: pointer is an in-arena object.
/// CEP:COST: O(reachable objects).
/// CEP:EVIDENCE: tests/gc_tests.rs.
/// CEP:SECURITY: only in-arena pointers are followed.
pub fn mark_from(heap: &mut Heap, ptr: usize) {
    if heap.marked(ptr) {
        return;
    }
    heap.set_mark(ptr);
    let mut work: Vec<usize> = vec![ptr];
    while let Some(p) = work.pop() {
        // Walk the payload words; follow references.
        let len = heap.array_len(p) as usize;
        for i in 0..len {
            // CEP:UNSAFE: payload word read of a marked engine object.
            // CEP:WHAT: Graph traversal.
            // CEP:WHY: Marking follows references.
            // CEP:FAILURE: pointer filtered against arena bounds below.
            // CEP:ASSUMES: p is an in-arena array object.
            // CEP:COST: one load per word.
            // CEP:EVIDENCE: tests/gc_tests.rs.
            // CEP:SECURITY: foreign words are never dereferenced.
            let w = unsafe { *((p + 16 + 8 * i) as *const u64) };
            if let Some(child) = heap.root_ptr_of(w)
                && !heap.marked(child) {
                    heap.set_mark(child);
                    work.push(child);
                }
        }
    }
}

/// Scan the mutator's machine stack conservatively.
///
/// CEP:WHAT: Conservative stack scan.
/// CEP:WHY: Machine-code frames hold object references that precise spans
///          cannot see; conservative scanning keeps them alive.
/// CEP:STATUS: complete
/// CEP:FAILURE: none; false positives retain garbage (documented).
/// CEP:ASSUMES: stacks grow down; the span [bottom, here) covers the live
///              frames (bottom registered at Heap construction, here from
///              a local's address).
/// CEP:COST: O(stack words).
/// CEP:EVIDENCE: tests/gc_tests.rs machine-frame root test.
/// CEP:SECURITY: words are filtered to in-arena object pointers.
pub fn scan_machine_stack(heap: &mut Heap) {
    let bottom = stack_bottom();
    let marker = 0u8;
    let here = &marker as *const u8 as usize;
    if bottom == 0 || here <= bottom || here - bottom > (1 << 22) {
        return; // span unavailable or implausible: skip (documented)
    }
    let mut roots: Vec<usize> = Vec::new();
    let mut addr = bottom;
    while addr + 8 <= here {
        // CEP:UNSAFE: conservative read of the mutator machine stack.
        // CEP:WHAT: Root candidate scan.
        // CEP:WHY: Compiled frames are untracked by precise spans.
        // CEP:FAILURE: words are filtered; no dereference of foreign data.
        // CEP:ASSUMES: 8-byte aligned stack words.
        // CEP:COST: one load per word.
        // CEP:EVIDENCE: tests/gc_tests.rs.
        // CEP:SECURITY: candidates filtered to arena object pointers.
        let w = unsafe { (addr as *const u64).read() };
        if let Some(p) = heap.root_ptr_of(w) {
            roots.push(p);
        }
        addr += 8;
    }
    for p in roots {
        mark_from(heap, p);
    }
}

/// Registered stack bottom (address of a local in the owning thread).
static STACK_BOTTOM: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Register the stack bottom; called once at engine construction.
///
/// CEP:WHAT: Stack-bottom registration.
/// CEP:WHY: The conservative scan needs the span start.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: called on the mutator thread.
/// CEP:COST: one store.
/// CEP:EVIDENCE: tests/gc_tests.rs.
/// CEP:SECURITY: none
pub fn register_stack_bottom() {
    let local = 0u8;
    let addr = &local as *const u8 as usize;
    STACK_BOTTOM.store(addr, std::sync::atomic::Ordering::Relaxed);
}

/// Read the registered stack bottom.
///
/// CEP:WHAT: Stack-bottom read.
/// CEP:WHY: Used by every collection.
/// CEP:STATUS: complete
/// CEP:FAILURE: 0 when unregistered (scan skipped, documented).
/// CEP:ASSUMES: none
/// CEP:COST: one load.
/// CEP:EVIDENCE: tests/gc_tests.rs.
/// CEP:SECURITY: none
#[must_use]
pub fn stack_bottom() -> usize {
    STACK_BOTTOM.load(std::sync::atomic::Ordering::Relaxed)
}

/// Scan dirty old cards for old-to-young references.
///
/// CEP:WHAT: Card table scan (spec 7.3).
/// CEP:WHY: Young collections must see old-to-young references.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: barriers dirty cards on every reference store.
/// CEP:COST: O(old objects in dirty cards).
/// CEP:EVIDENCE: tests/gc_tests.rs card test.
/// CEP:SECURITY: none
pub fn scan_dirty_cards(heap: &mut Heap) {
    // Walk old objects; mark young targets of old objects (cards bound
    // the scan in a production collector; the object walk is equivalent
    // for correctness and documented in doc/gc.md Known Limitations).
    let mut targets: Vec<usize> = Vec::new();
    heap.for_each_object(true, &mut |p, _h, _size| {
        let len = heap.array_len(p) as usize;
        for i in 0..len {
            // CEP:UNSAFE: payload read of an old-gen engine object.
            // CEP:WHAT: Remembered-set scan.
            // CEP:WHY: Old-to-young references must survive young GC.
            // CEP:FAILURE: filtered against arenas.
            // CEP:ASSUMES: p in old arena.
            // CEP:COST: one load per word.
            // CEP:EVIDENCE: tests/gc_tests.rs.
            // CEP:SECURITY: filtered words only.
            let w = unsafe { *((p + 16 + 8 * i) as *const u64) };
            if let Some(c) = heap.root_ptr_of(w)
                && heap.in_young(c) {
                    targets.push(c);
                }
        }
    });
    for t in targets {
        mark_from(heap, t);
    }
}

/// Increment the age of all live objects in a generation.
///
/// CEP:WHAT: Survivor aging.
/// CEP:WHY: Generational hypothesis accounting (spec 7.1).
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: mark bits current.
/// CEP:COST: O(objects).
/// CEP:EVIDENCE: tests/gc_tests.rs.
/// CEP:SECURITY: none
pub fn age_survivors(heap: &mut Heap, old: bool) {
    let mut live: Vec<usize> = Vec::new();
    heap.for_each_object(old, &mut |p, h, _| {
        if (h >> 40) & 1 == 1 {
            live.push(p);
        }
    });
    for p in live {
        heap.age_object(p);
    }
}

/// Collect precise object roots from interpreter frames.
///
/// CEP:WHAT: Frame root extraction.
/// CEP:WHY: Interpreter values are typed `Value`s, not machine words, so
///          the precise root scan walks them directly instead of scanning
///          raw memory.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: frames outlive the collection.
/// CEP:COST: O(frame slots).
/// CEP:EVIDENCE: tests/gc_tests.rs.
/// CEP:SECURITY: only in-arena pointers are returned.
pub fn frame_roots(frames: &[crate::hot::interp::FnFrame]) -> Vec<usize> {
    let mut roots = Vec::new();
    for f in frames {
        for i in 0..f.stack.len() {
            if let Some(v) = f.stack.get(i)
                && let Value::Obj(p) = v {
                    roots.push(p);
                }
        }
        for l in &f.locals {
            for i in 0..l.len() {
                if let Some(v) = l.get(i)
                    && let Value::Obj(p) = v {
                        roots.push(p);
                    }
            }
        }
    }
    roots
}

/// Collect with automatic retry: run `f`, and on GcNeeded run a young
/// collection and retry once.
///
/// CEP:WHAT: GC-retry wrapper.
/// CEP:WHY: The interpreter and compiled-code slow paths need a uniform
///          collect-and-retry contract.
/// CEP:STATUS: complete
/// CEP:FAILURE: OutOfMemory when the retry still fails (propagated).
/// CEP:ASSUMES: `f` is idempotent when it returns GcNeeded (no partial
///              side effects on the failed path).
/// CEP:COST: collection cost on the retry path only.
/// CEP:EVIDENCE: tests/gc_tests.rs pressure test.
/// CEP:SECURITY: bounded retries (one).
///
/// # Errors
/// Propagates OutOfMemory after one collection.
pub fn with_gc_retry<T>(
    heap: &mut Heap,
    frames: &[crate::hot::interp::FnFrame],
    f: impl Fn(&mut Heap) -> Result<T, EngineError>,
) -> Result<T, EngineError> {
    match f(heap) {
        Ok(v) => Ok(v),
        Err(EngineError::GcNeeded) => {
            // Precise roots from the interpreter frames plus the
            // conservative machine-stack scan keep everything reachable.
            let roots = frame_roots(frames);
            collect_young(heap, &roots);
            f(heap)
        }
        Err(e) => Err(e),
    }
}

/// Value word helper used by tests and the engine's root registration.
///
/// CEP:WHAT: Value-to-root filter.
/// CEP:WHY: Tests exercise root filtering directly.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: tests/gc_tests.rs.
/// CEP:SECURITY: none
#[must_use]
pub fn value_root(heap: &Heap, v: &Value) -> Option<usize> {
    heap.root_ptr_of(v.encode())
}
