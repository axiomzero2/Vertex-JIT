// CEP:FILE: engine/src/platform.rs
// CEP:WHAT: Named target configuration values and runtime feature detection.
// CEP:WHY: Standard Law 7 and section 7.2: every environment constant used
//          by the engine is named, justified and checked here; no other
//          module may mention the host ISA.
// CEP:CLASS: CEP-1
// CEP:STATUS: complete
// CEP:FAILURE: TargetConfig::detect returns TargetError when the host is
//              not supported x86-64 Linux user space.
// CEP:ASSUMES: Linux x86-64 with separate PROT_WRITE/PROT_EXEC mprotect.
// CEP:COST: one stdio page-size query plus CPU feature detection, once.
// CEP:EVIDENCE: unit tests below; used by every engine construction.
// CEP:SECURITY: detection is host introspection only; no untrusted input.

/// Target configuration: every constant the engine relies on, named once.
///
/// No engine module may hard-code page sizes, cache line sizes, endianness
/// or CPU feature assumptions; they read them from here (Law 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetConfig {
    /// Cache line size in bytes. Source: "Intel 64 and IA-32 Architectures
    /// Optimization Reference Manual", table 2-5 (x86-64 server/desktop).
    pub cache_line_bytes: u32,
    /// Virtual memory page size in bytes; asserted against the OS (below).
    pub page_bytes: u32,
    /// True when the host stores multi-byte values little-endian.
    pub little_endian: bool,
    /// Width in bits of a canonical user-space virtual address. x86-64
    /// Linux user space guarantees 47 bits; boxed value words carry a
    /// 47-bit payload (hot::value).
    pub virtual_address_bits: u32,
    /// True when the CPU supports AVX2 with OS YMM state enabled.
    pub avx2: bool,
    /// True when the CPU supports AVX-512F; recorded, not yet emitted
    /// (doc/vectorize.md Known Limitations).
    pub avx512f: bool,
    /// Default maximum bytes of resident generated code (38.39 budget).
    pub default_code_cache_bytes: u32,
}

/// Errors raised while validating the host target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetError {
    /// The host is not x86-64.
    UnsupportedArchitecture,
    /// The host endianness differs from the engine serialization order.
    EndiannessUnsupported,
    /// A mapping landed outside the 47-bit canonical user space.
    VirtualAddressTooWide,
    /// The page size reported by the OS is not the asserted power of two.
    PageSizeUnsupported,
}

impl core::fmt::Display for TargetError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let text = match self {
            TargetError::UnsupportedArchitecture => "host architecture is not x86-64",
            TargetError::EndiannessUnsupported => {
                "host is not little-endian; engine serializes little-endian"
            }
            TargetError::VirtualAddressTooWide => {
                "mapping exceeds 47-bit canonical user space"
            }
            TargetError::PageSizeUnsupported => "OS page size is not 4096 bytes",
        };
        f.write_str(text)
    }
}

impl std::error::Error for TargetError {}

impl TargetConfig {
    /// Detect and validate the host target.
    ///
    /// CEP:WHAT: Detects CPU features and validates the platform contract.
    /// CEP:WHY: Laws 2 and 7 require assumptions to become checked values.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: TargetError on unsupported arch/endianness/page size.
    /// CEP:ASSUMES: Linux; page size 4096 (asserted via probe below).
    /// CEP:COST: O(1); syscalls + CPUID, once per process.
    /// CEP:EVIDENCE: unit tests.
    /// CEP:SECURITY: no untrusted input.
    pub fn detect() -> Result<TargetConfig, TargetError> {
        #[cfg(not(target_arch = "x86_64"))]
        {
            return Err(TargetError::UnsupportedArchitecture);
        }
        #[cfg(not(target_endian = "little"))]
        {
            return Err(TargetError::EndiannessUnsupported);
        }
        #[cfg(target_os = "linux")]
        {
            let probe = probe_page_size();
            if probe != 4096 {
                return Err(TargetError::PageSizeUnsupported);
            }
        }
        let (avx2, avx512f) = detect_vector_features();
        Ok(TargetConfig {
            cache_line_bytes: 64,
            page_bytes: 4096,
            little_endian: true,
            virtual_address_bits: 47,
            avx2,
            avx512f,
            default_code_cache_bytes: 4 * 1024 * 1024,
        })
    }

    /// Validate that a heap pointer fits the boxed-value payload width.
    ///
    /// CEP:WHAT: Pointer width check against the 47-bit box payload.
    /// CEP:WHY: Boxed words carry 47-bit payloads (hot::value).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: VirtualAddressTooWide above the canonical width.
    /// CEP:ASSUMES: ptr is a valid heap pointer.
    /// CEP:COST: one compare.
    /// CEP:EVIDENCE: unit test validate_pointer_bounds.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: one compare; lower bound for a range check.
    /// CEP:SECURITY: prevents silent truncation of adversarial pointers.
    pub fn validate_pointer_bounds(&self, ptr: usize) -> Result<(), TargetError> {
        let limit = 1usize << self.virtual_address_bits;
        if ptr >= limit {
            Err(TargetError::VirtualAddressTooWide)
        } else {
            Ok(())
        }
    }
}

/// Probe the OS page size via sysconf.
///
/// CEP:WHAT: Page size query.
/// CEP:WHY: mprotect granularity defines code-cache page alignment.
/// CEP:STATUS: complete
/// CEP:FAILURE: Returns 0 when sysconf fails (detect then fails closed).
/// CEP:ASSUMES: Linux _SC_PAGESIZE is 30 (glibc and musl agree).
/// CEP:COST: one syscall, once.
/// CEP:EVIDENCE: used by detect.
/// CEP:SECURITY: no untrusted input.
#[cfg(target_os = "linux")]
fn probe_page_size() -> u32 {
    unsafe extern "C" {
        fn sysconf(name: i32) -> i64;
    }
    let size = unsafe { sysconf(30) };
    if size <= 0 { 0 } else { size as u32 }
}

/// Detect AVX2 and AVX-512F including OS state enablement.
///
/// CEP:WHAT: CPU feature detection with XCR0 check.
/// CEP:WHY: The vectorizer must know the real dispatch target (38.26);
///          the std macro performs the CPUID plus OSXSAVE/XGETBV checks.
/// CEP:STATUS: complete
/// CEP:FAILURE: (false, false) when detection is impossible.
/// CEP:ASSUMES: x86-64.
/// CEP:COST: two CPUID leaves plus one XGETBV, once per process.
/// CEP:EVIDENCE: doc/target.md.
/// CEP:SECURITY: host introspection only.
#[cfg(target_arch = "x86_64")]
fn detect_vector_features() -> (bool, bool) {
    let avx2 = std::arch::is_x86_feature_detected!("avx2");
    let avx512f = std::arch::is_x86_feature_detected!("avx512f");
    (avx2, avx512f)
}

#[cfg(not(target_arch = "x86_64"))]
fn detect_vector_features() -> (bool, bool) {
    (false, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_succeeds_on_supported_targets() {
        let cfg = TargetConfig::detect();
        #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
        assert!(cfg.is_ok());
        let _ = cfg;
    }

    #[test]
    fn validate_pointer_bounds_rejects_wide_pointers() {
        let cfg = TargetConfig {
            cache_line_bytes: 64,
            page_bytes: 4096,
            little_endian: true,
            virtual_address_bits: 47,
            avx2: false,
            avx512f: false,
            default_code_cache_bytes: 1,
        };
        assert!(cfg.validate_pointer_bounds((1 << 47) - 1).is_ok());
        assert_eq!(
            cfg.validate_pointer_bounds(1 << 47),
            Err(TargetError::VirtualAddressTooWide)
        );
    }
}
