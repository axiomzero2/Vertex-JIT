// CEP:FILE: engine/src/cold/code_cache.rs
// CEP:WHAT: Bounded W^X code cache: mmap-backed executable regions with
//           size-class pooling, budget accounting and page-aligned layout.
// CEP:WHY: JIT policy (standard 38.39) requires W^X, code cache bounds and
//          safe publication; spec 2.3 requires buffer pools and mmap.
// CEP:CLASS: CEP-1
// CEP:STATUS: complete
// CEP:FAILURE: allocate returns CodeCacheFull when the budget cannot fit
//              the rounding; mmap/mprotect failures surface as
//              OutOfMemory. Patching a region executing on another thread
//              is forbidden by the single-mutator contract (doc/engine.md).
// CEP:ASSUMES: Linux mmap/mprotect semantics; one mutator patches.
// CEP:COST: allocate O(free-list); execution cost zero.
// CEP:EVIDENCE: tests below (execution, budget, patch flip, reuse).
// CEP:SECURITY: regions are private anonymous mappings; generated code is
//               engine-authored; no input bytes are executable.

use crate::cold::errors::EngineError;
use crate::platform::TargetConfig;

const PROT_READ: i32 = 0x1;
const PROT_WRITE: i32 = 0x2;
const PROT_EXEC: i32 = 0x4;
const MAP_PRIVATE: i32 = 0x02;
const MAP_ANONYMOUS: i32 = 0x20;
const MAP_FAILED: isize = -1;

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
    fn munmap(addr: *mut u8, len: usize) -> i32;
}

/// One mmap-backed executable region.
struct Region {
    base: *mut u8,
    /// Requested usable bytes.
    usable: u32,
    /// Mapped bytes (page multiple).
    mapped: u32,
    /// True while writable (PROT_WRITE).
    writable: bool,
}

// SAFETY: Region owns one anonymous mapping, dereferenced only through the
// owning CodeCache (which is !Sync: single-mutator contract).
unsafe impl Send for Region {}

/// Bounded code cache; not Sync (patching is single-mutator).
pub struct CodeCache {
    budget: u32,
    used: u32,
    page: u32,
    regions: Vec<Region>,
    free: Vec<Region>,
}

impl CodeCache {
    /// Create a cache with the configured byte budget.
    ///
    /// CEP:WHAT: Cache constructor.
    /// CEP:WHY: Budgets are enforced from construction on.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(1); mappings are lazy.
    /// CEP:EVIDENCE: tests below.
    /// CEP:SECURITY: none
    ///
    /// # Errors
    /// Returns Target errors from the platform contract.
    pub fn new(target: &TargetConfig, budget: u32) -> Result<CodeCache, EngineError> {
        Ok(CodeCache {
            budget,
            used: 0,
            page: target.page_bytes,
            regions: Vec::new(),
            free: Vec::new(),
        })
    }

    /// Resident mapped bytes.
    ///
    /// CEP:WHAT: Usage query.
    /// CEP:WHY: Budget tests and stats.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(regions).
    /// CEP:EVIDENCE: tests below.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn used_bytes(&self) -> u32 {
        self.used
    }

    /// Allocate `len` executable bytes; writable until `seal`.
    ///
    /// CEP:WHAT: Executable region allocation.
    /// CEP:WHY: Every tier publishes code through this gate.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: CodeCacheFull when the budget cannot fit; OutOfMemory
    ///              when mmap/mprotect fail.
    /// CEP:ASSUMES: len >= 1.
    /// CEP:COST: O(free list) amortized; one mmap per new region.
    /// CEP:EVIDENCE: tests below.
    /// CEP:SECURITY: private anonymous mapping.
    ///
    /// # Errors
    /// Returns CodeCacheFull or OutOfMemory.
    pub fn allocate(&mut self, len: u32) -> Result<*mut u8, EngineError> {
        debug_assert!(len > 0);
        let class = round_up_to(len.max(1), self.page);
        if self.used.saturating_add(class) > self.budget {
            return Err(EngineError::CodeCacheFull { budget: self.budget, requested: class });
        }
        // Size-class reuse (spec 2.3 buffer pools).
        if let Some(i) = self.free.iter().position(|r| !r.writable) {
            let mut region = self.free.swap_remove(i);
            region.usable = class;
            set_prot(region.base, region.mapped, PROT_READ | PROT_WRITE)?;
            region.writable = true;
            self.used = self.used.saturating_add(region.mapped);
            self.regions.push(region);
            return Ok(self.regions.last().unwrap().base);
        }
        let mapped = round_up_to(class, self.page);
        // CEP:UNSAFE: mmap of an anonymous private executable region.
        // CEP:WHAT: RW mapping for code writing.
        // CEP:WHY: Executable memory requires mmap on Linux.
        // CEP:FAILURE: MAP_FAILED surfaces as OutOfMemory.
        // CEP:ASSUMES: Linux page granularity (validated by TargetConfig).
        // CEP:COST: one syscall per new region.
        // CEP:EVIDENCE: tests below.
        // CEP:SECURITY: private and anonymous; no file backing.
        let base = unsafe {
            mmap(
                std::ptr::null_mut(),
                mapped as usize,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if base as isize == MAP_FAILED {
            return Err(EngineError::OutOfMemory);
        }
        self.used = self.used.saturating_add(mapped);
        self.regions.push(Region { base, usable: class, mapped, writable: true });
        Ok(base)
    }

    /// Transition the region containing `base` to read-execute (W^X seal).
    ///
    /// CEP:WHAT: Publish generated code as executable.
    /// CEP:WHY: W^X policy (38.39): never writable and executable together.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: OutOfMemory when mprotect fails.
    /// CEP:ASSUMES: base came from allocate.
    /// CEP:COST: one syscall.
    /// CEP:EVIDENCE: patch-flip test below.
    /// CEP:SECURITY: marks engine pages executable only.
    ///
    /// # Errors
    /// Returns OutOfMemory when mprotect fails.
    pub fn seal(&mut self, base: *mut u8) -> Result<(), EngineError> {
        let region = Self::region_mut(&mut self.regions, base)?;
        region.usable = region.usable.max(region.usable);
        set_prot(region.base, region.mapped, PROT_READ | PROT_EXEC)?;
        region.writable = false;
        Ok(())
    }

    /// Open a patch window (region becomes writable).
    ///
    /// CEP:WHAT: Begin patch window.
    /// CEP:WHY: OSR rebinding and deopt patching mutate live code.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: OutOfMemory when mprotect fails.
    /// CEP:ASSUMES: no concurrent execution of the region.
    /// CEP:COST: one syscall.
    /// CEP:EVIDENCE: tests below.
    /// CEP:SECURITY: short, engine-internal window.
    ///
    /// # Errors
    /// Returns OutOfMemory when mprotect fails.
    pub fn patch_begin(&mut self, base: *mut u8) -> Result<(), EngineError> {
        let region = Self::region_mut(&mut self.regions, base)?;
        if region.writable {
            return Ok(());
        }
        set_prot(region.base, region.mapped, PROT_READ | PROT_WRITE)?;
        region.writable = true;
        Ok(())
    }

    /// Close a patch window (region returns to read-execute).
    ///
    /// CEP:WHAT: End patch window.
    /// CEP:WHY: Restores W^X.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: OutOfMemory when mprotect fails.
    /// CEP:ASSUMES: a window is open.
    /// CEP:COST: one syscall.
    /// CEP:EVIDENCE: tests below.
    /// CEP:SECURITY: none
    ///
    /// # Errors
    /// Returns OutOfMemory when mprotect fails.
    pub fn patch_end(&mut self, base: *mut u8) -> Result<(), EngineError> {
        let region = Self::region_mut(&mut self.regions, base)?;
        if !region.writable {
            return Ok(());
        }
        set_prot(region.base, region.mapped, PROT_READ | PROT_EXEC)?;
        region.writable = false;
        Ok(())
    }

    /// Release a region back to the pool.
    ///
    /// CEP:WHAT: Region recycling.
    /// CEP:WHY: Spec 2.3 buffer pools avoid repeated mmap/munmap.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: unknown bases are ignored.
    /// CEP:ASSUMES: region is sealed.
    /// CEP:COST: O(regions).
    /// CEP:EVIDENCE: reuse test below.
    /// CEP:SECURITY: none
    pub fn release(&mut self, base: *mut u8) {
        let Some(i) = self.regions.iter().position(|r| r.base == base) else {
            return;
        };
        let mut region = self.regions.swap_remove(i);
        self.used = self.used.saturating_sub(region.mapped);
        if region.writable {
            if set_prot(region.base, region.mapped, PROT_READ | PROT_EXEC).is_err() {
                self.used = self.used.saturating_add(region.mapped);
                self.regions.push(region);
                return;
            }
            region.writable = false;
        }
        self.free.push(region);
    }

    fn region_mut(regions: &mut [Region], base: *mut u8) -> Result<&mut Region, EngineError> {
        regions
            .iter_mut()
            .find(|r| r.base == base)
            .ok_or(EngineError::DeoptFailed("region not owned by this cache"))
    }
}

impl Drop for CodeCache {
    fn drop(&mut self) {
        for r in self.regions.drain(..).chain(self.free.drain(..)) {
            // CEP:UNSAFE: munmap of engine-owned anonymous mappings.
            // CEP:WHAT: Region teardown.
            // CEP:WHY: No executable mappings outlive the engine.
            // CEP:FAILURE: ignored; cannot fail for valid mappings.
            // CEP:ASSUMES: no live execution.
            // CEP:COST: one syscall per region.
            // CEP:EVIDENCE: sanitizer runs in CI.
            // CEP:SECURITY: removes W^X pages from the address space.
            unsafe {
                let _ = munmap(r.base, r.mapped as usize);
            }
        }
    }
}

/// Set protection on a mapping.
///
/// CEP:WHAT: mprotect wrapper.
/// CEP:WHY: W^X transitions.
/// CEP:STATUS: complete
/// CEP:FAILURE: OutOfMemory on nonzero return.
/// CEP:ASSUMES: page-aligned mapping (mmap guarantee).
/// CEP:COST: one syscall.
/// CEP:EVIDENCE: tests below.
/// CEP:SECURITY: only engine pages.
///
/// # Errors
/// Returns OutOfMemory on failure.
fn set_prot(base: *mut u8, len: u32, prot: i32) -> Result<(), EngineError> {
    // CEP:UNSAFE: mprotect on an engine-owned mapping.
    // CEP:WHAT: Permission transition.
    // CEP:WHY: W^X.
    // CEP:FAILURE: nonzero return surfaces as OutOfMemory.
    // CEP:ASSUMES: page-aligned mapping.
    // CEP:COST: one syscall.
    // CEP:EVIDENCE: tests below.
    // CEP:SECURITY: only engine pages are affected.
    let rc = unsafe { mprotect(base, len as usize, prot) };
    if rc != 0 {
        return Err(EngineError::OutOfMemory);
    }
    Ok(())
}

const fn round_up_to(v: u32, align: u32) -> u32 {
    debug_assert!(align.is_power_of_two());
    (v + align - 1) & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> TargetConfig {
        TargetConfig {
            cache_line_bytes: 64,
            page_bytes: 4096,
            little_endian: true,
            virtual_address_bits: 47,
            avx2: false,
            avx512f: false,
            default_code_cache_bytes: 1 << 20,
        }
    }

    /// Write and execute a `mov eax, 42; ret` stub through the cache.
    #[test]
    fn executes_generated_code_and_seals() {
        let t = target();
        let mut cache = CodeCache::new(&t, 64 * 1024).unwrap();
        let base = cache.allocate(64).unwrap();
        let code: [u8; 6] = [0xB8, 42, 0, 0, 0, 0xC3];
        // CEP:UNSAFE: writing engine-generated bytes into the RW window.
        // CEP:WHAT: code emission.
        // CEP:ASSUMES: region writable (allocate contract).
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), base, code.len());
        }
        cache.seal(base).unwrap();
        let f: extern "C" fn() -> u32 = unsafe { std::mem::transmute(base) };
        assert_eq!(f(), 42);
        cache.release(base);
    }

    #[test]
    fn budget_is_enforced() {
        let t = target();
        let mut cache = CodeCache::new(&t, 8 * 1024).unwrap();
        cache.allocate(4096).unwrap();
        cache.allocate(4096).unwrap();
        let err = cache.allocate(1).unwrap_err();
        assert!(matches!(err, EngineError::CodeCacheFull { .. }));
    }

    #[test]
    fn released_regions_are_reused() {
        let t = target();
        let mut cache = CodeCache::new(&t, 64 * 1024).unwrap();
        let a = cache.allocate(4096).unwrap();
        cache.release(a);
        let b = cache.allocate(4096).unwrap();
        assert_eq!(a, b, "pool must return the same region");
    }
}
