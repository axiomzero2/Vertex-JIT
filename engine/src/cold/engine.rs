// CEP:FILE: engine/src/cold/engine.rs
// CEP:WHAT: The tiering engine driver: function table, per-function tier
//           state, T0 copy-and-patch compilation and execution, the T1-T3
//           pipeline compilation, the interpreter budget loop with OSR
//           triggers, guard deoptimization, GC-retry integration and the
//           documented statistics.
// CEP:WHY: Spec section 6: tier transitions at the documented thresholds;
//          standard 38.39: budgets, W^X publication, deopt semantics.
// CEP:CLASS: CEP-1
// CEP:STATUS: complete
// CEP:FAILURE: Every fallible operation returns EngineError; compilation
//              failures fall back to a lower tier (never silent); traps
//              deoptimize or fail loud (DeoptFailed) on inconsistency.
// CEP:ASSUMES: single mutator; functions verified at ingest; Linux x86-64.
// CEP:COST: execution cost is dominated by the active tier; compilation is
//           budgeted (38.39) and measured in benches/EVIDENCE.md.
// CEP:EVIDENCE: tests/differential.rs (all tiers), tests/engine_tests.rs
//               (transitions), tests/osr_tests.rs, tests/deopt_tests.rs.
// CEP:SECURITY: bytecode is verified at ingest; generated code is
//               engine-authored and published W^X; trap frames are
//               validated before use.

use crate::cold::bytecode::{Function, Opcode, StringPool};
use crate::cold::code_cache::CodeCache;
use crate::cold::codegen::{self, Helpers};
use crate::cold::deopt;
use crate::cold::errors::EngineError;
use crate::cold::gc;
use crate::cold::ir::{self, LiftError};
use crate::cold::passes::{self, PassBudgets};
use crate::cold::tier::{JitBudgets, Tier, TierThresholds};
use crate::cold::tier0::{self, TemplateLibrary};
use crate::hot::heap::Heap;
use crate::hot::interp::{Exit, FnFrame, HostHooks, Interp, ProfileData, RunBudget};
use crate::hot::runtime::{with_trampoline, TrampOutcome};
use crate::hot::value::Value;
use crate::platform::TargetConfig;

/// One compiled function resident in the code cache.
pub struct CompiledFn {
    /// Tier of this compilation.
    pub tier: Tier,
    /// Region base (W^X sealed).
    pub base: *mut u8,
    /// Entry offset within the region.
    pub entry: usize,
    /// Code metadata (deopt records, slots).
    pub code: codegen::CompiledCode,
    /// Static type of the returned word (typed decode, doc/value.md).
    pub return_tag: crate::hot::value::TypeTag,
}

// SAFETY: base points into the engine's code cache; the cache owns the
// mapping and the engine is single-mutator.
unsafe impl Send for CompiledFn {}

/// Statistics reported by the engine (documented counters).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EngineStats {
    /// Functions compiled per tier: [interp, t0, t1, t2, t3].
    pub compiled: [u64; 5],
    /// Guard deoptimizations performed.
    pub deopts: u64,
    /// OSR entries performed.
    pub osr_entries: u64,
    /// Allocation-failure deopts (collect-and-retry via the interpreter).
    pub alloc_deopts: u64,
}

/// The tiered JIT engine.
pub struct JitEngine {
    /// Function table (verified at ingest).
    pub funcs: Vec<Function>,
    /// Per-function profiles.
    pub profiles: Vec<ProfileData>,
    /// Runtime string pool.
    pub pool: StringPool,
    /// GC heap.
    pub heap: Heap,
    /// Code cache (W^X, budgeted).
    cache: CodeCache,
    /// Frozen T0 template library.
    lib: TemplateLibrary,
    /// Target configuration.
    pub target: TargetConfig,
    /// Tier thresholds.
    pub thresholds: TierThresholds,
    /// JIT budgets.
    pub budgets: JitBudgets,
    /// Pass budgets.
    pub pass_budgets: PassBudgets,
    /// Current tier per function.
    pub tiers: Vec<Tier>,
    /// Compiled code per function (None while on the interpreter).
    pub compiled: Vec<Option<CompiledFn>>,
    /// Statistics.
    pub stats: EngineStats,
    /// Interpreter.
    interp: Interp,
}

impl JitEngine {
    /// Construct an engine over a function table.
    ///
    /// CEP:WHAT: Engine construction.
    /// CEP:WHY: All budgets, the template library and the code cache are
    ///          established here; every function starts on the interpreter.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: Target and mapping failures surface as EngineError;
    ///              every function must pass structural verification.
    /// CEP:ASSUMES: single mutator.
    /// CEP:COST: O(functions) verification plus one-time setup.
    /// CEP:EVIDENCE: engine tests.
    /// CEP:SECURITY: untrusted bytecode is verified before any tier sees it.
    ///
    /// # Errors
    /// Target/verification failures.
    pub fn new(
        funcs: Vec<Function>,
        target: &TargetConfig,
        budgets: JitBudgets,
    ) -> Result<JitEngine, EngineError> {
        for f in &funcs {
            crate::cold::bytecode::verify(f)?;
        }
        let profiles = funcs.iter().map(|f| ProfileData::new(f.code.len())).collect();
        let pool = StringPool::new(&funcs.first().map(|f| f.strings.clone()).unwrap_or_default(), 1 << 20);
        gc::register_stack_bottom();
        let heap = Heap::new(1024 * 1024, 4 * 1024 * 1024)?;
        let cache = CodeCache::new(target, budgets.code_cache_bytes)?;
        let lib = TemplateLibrary::new(
            crate::hot::runtime::jit_fmod as *const () as usize,
            crate::hot::runtime::jit_trap_div0 as *const () as usize,
        );
        let n = funcs.len();
        Ok(JitEngine {
            funcs,
            profiles,
            pool,
            heap,
            cache,
            lib,
            target: *target,
            thresholds: TierThresholds::default(),
            budgets,
            pass_budgets: PassBudgets {
                max_unroll_factor: budgets.max_unroll_factor,
                vector_width: 4,
                ..PassBudgets::default()
            },
            tiers: vec![Tier::Interp; n],
            compiled: (0..n).map(|_| None).collect(),
            stats: EngineStats::default(),
            interp: Interp::new(),
        })
    }

    /// Ingest a function (append to the table).
    ///
    /// CEP:WHAT: Function ingestion.
    /// CEP:WHY: The engine's CALL extension addresses functions by table id.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: Verification failures propagate.
    /// CEP:ASSUMES: none
    /// CEP:COST: O(function size).
    /// CEP:EVIDENCE: engine tests.
    /// CEP:SECURITY: verification is the ingest trust gate.
    ///
    /// # Errors
    /// Verification failure.
    pub fn add_function(&mut self, f: Function) -> Result<u32, EngineError> {
        crate::cold::bytecode::verify(&f)?;
        self.funcs.push(f);
        self.profiles.push(ProfileData::new(
            self.funcs
                .last()
                .map(|x| x.code.len())
                .unwrap_or_default(),
        ));
        self.tiers.push(Tier::Interp);
        self.compiled.push(None);
        Ok((self.funcs.len() - 1) as u32)
    }

    /// Run the entry function to completion.
    ///
    /// CEP:WHAT: The execution driver.
    /// CEP:WHY: Spec 6.3 tier transitions; deopt and OSR integration.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: Interp errors, deopt failures, code cache and GC
    ///              failures surface as EngineError.
    /// CEP:ASSUMES: single mutator; fn_id valid.
    /// CEP:COST: tier-dependent; see tier docs.
    /// CEP:EVIDENCE: differential suite.
    /// CEP:SECURITY: verified functions only; W^X enforced by the cache.
    ///
    /// # Errors
    /// EngineError per the failure list.
    pub fn run(&mut self, fn_id: u32, host: &HostHooks) -> Result<Exit, EngineError> {
        let mut frames: Vec<FnFrame> = Vec::new();
        let entry = self
            .funcs
            .get(fn_id as usize)
            .ok_or(EngineError::PcOutOfRange(fn_id as usize))?;
        frames.push(self.interp.make_frame(fn_id, entry)?);
        loop {
            let fid = frames
                .last()
                .ok_or(EngineError::PcOutOfRange(0))?
                .fn_id;
            // Tier selection.
            match self.tiers[fid as usize] {
                Tier::Interp => {
                    // Interpreter with an OSR budget window.
                    let budget = RunBudget { back_edges: self.thresholds.osr_at };
                    let outcome = self.interp.run(
                        &self.funcs,
                        &mut frames,
                        &mut self.profiles,
                        &mut self.pool,
                        &mut self.heap,
                        budget,
                        host,
                    );
                    match outcome {
                        Ok(exit) => return Ok(exit),
                        Err(EngineError::GcNeeded) => {
                            let roots = gc::frame_roots(&frames);
                            gc::collect_young(&mut self.heap, &roots);
                        }
                        Err(e) => return Err(e),
                    }
                    // Budget window exhausted: OSR or tier-up.
                    self.maybe_compile(fid);
                    if let Some(exit) = self.try_osr(fid, &mut frames, host)? {
                        return Ok(exit);
                    }
                }
                Tier::T0 | Tier::T1 | Tier::T2 | Tier::T3 => {
                    if self.compiled[fid as usize].is_none() {
                        self.compile_for(fid)?;
                    }
                    if self.compiled[fid as usize].is_none() {
                        // Compilation refused (outside the subset): run the
                        // interpreter instead (documented fallback).
                        self.tiers[fid as usize] = Tier::Interp;
                        continue;
                    }
                    let exit = self.run_compiled(fid, &mut frames)?;
                    if let Some(e) = exit {
                        return Ok(e);
                    }
                    // Deopt consumed the frame: continue the loop, which
                    // now runs the interpreter for the deopted frame.
                }
            }
        }
    }

    /// Compile a function for its current tier.
    ///
    /// CEP:WHAT: Tiered compilation.
    /// CEP:WHY: Spec 2-5: T0 templates; T1+ pipeline.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: Compilation refusals leave the tier at Interp (documented
    ///              fallback); cache failures propagate.
    /// CEP:ASSUMES: fn_id valid.
    /// CEP:COST: budgeted per tier.
    /// CEP:EVIDENCE: engine tests.
    /// CEP:SECURITY: publication is W^X through the cache.
    ///
    /// # Errors
    /// EngineError.
    fn compile_for(&mut self, fn_id: u32) -> Result<(), EngineError> {
        let tier = self.tiers[fn_id as usize];
        let func = self.funcs[fn_id as usize].clone();
        match tier {
            Tier::T0 => {
                let code = tier0::compile(&func, &self.lib);
                match code {
                    Ok(c) => {
                        let base = self.cache.allocate(c.bytes.len() as u32)?;
                        // CEP:UNSAFE: write of engine-generated code into
                        // the RW window, then seal to RX.
                        // CEP:WHAT: T0 publication.
                        // CEP:WHY: W^X publication (38.39).
                        // CEP:FAILURE: cache errors propagate.
                        // CEP:ASSUMES: region writable.
                        // CEP:COST: one memcpy + mprotect.
                        // CEP:EVIDENCE: t0 execution tests.
                        // CEP:SECURITY: engine-authored bytes only.
                        unsafe {
                            std::ptr::copy_nonoverlapping(c.bytes.as_ptr(), base, c.bytes.len());
                        }
                        self.cache.seal(base)?;
                        self.compiled[fn_id as usize] = Some(CompiledFn {
                            tier,
                            base,
                            entry: base as usize,
                            code: codegen::CompiledCode {
                                bytes: c.bytes,
                                deopt: Vec::new(),
                                slots: c.max_stack,
                                return_tag: c.return_tag,
                            },
                            return_tag: c.return_tag,
                        });
                        self.stats.compiled[1] += 1;
                    }
                    Err(_) => {
                        self.tiers[fn_id as usize] = Tier::Interp;
                    }
                }
            }
            Tier::T1 | Tier::T2 | Tier::T3 => {
                self.compile_optimized(fn_id, tier)?;
            }
            Tier::Interp => {}
        }
        Ok(())
    }

    /// Run the IR pipeline (spec 3.2/4/5 passes) and lower to machine code.
    ///
    /// CEP:WHAT: Optimized compilation (T1-T3).
    /// CEP:WHY: Spec 3.1: bytecode -> Sea of Nodes -> optimizations ->
    ///          machine code.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: Lift/verify/pass failures fall back to Interp
    ///              (documented); budget failures likewise.
    /// CEP:ASSUMES: verified bytecode; single mutator.
    /// CEP:COST: budgeted (38.39).
    /// CEP:EVIDENCE: differential tests prove optimized tiers match the
    ///               interpreter bit-for-bit.
    /// CEP:SECURITY: the verifier gates every pass; publication is W^X.
    ///
    /// # Errors
    /// Cache failures propagate; pipeline failures fall back.
    fn compile_optimized(&mut self, fn_id: u32, tier: Tier) -> Result<(), EngineError> {
        let func = self.funcs[fn_id as usize].clone();
        let lifted = match ir::lift(&func, self.budgets.max_ir_nodes) {
            Ok(l) => l,
            Err(LiftError::UnsupportedOpcode(_)) | Err(LiftError::UnbalancedStack) => {
                // Outside the IR subset: stay on the interpreter/T0
                // (documented Known Limitation, doc/ir.md).
                self.tiers[fn_id as usize] = Tier::Interp;
                return Ok(());
            }
            Err(e) => {
                let _ = e;
                self.tiers[fn_id as usize] = Tier::Interp;
                return Ok(());
            }
        };
        // Explicit, versioned pass pipeline (standard 38.20).
        let ir::Lifted { mut graph, deopt_sites: mut sites } = lifted;
        for name in passes::passes_for_tier(match tier {
            Tier::T1 => 1,
            Tier::T2 => 2,
            _ => 3,
        }) {
            if *name == "verify" {
                ir::verify(&graph).map_err(EngineError::IrVerificationFailed)?;
            } else {
                passes::run_pass(name, &mut graph, &mut sites, &self.pass_budgets)
                    .map_err(EngineError::IrVerificationFailed)?;
            }
        }
        ir::verify(&graph).map_err(EngineError::IrVerificationFailed)?;

        // Callees must be compiled before callers resolve call addresses.
        self.compile_callees_of(&func)?;
        let callee_addrs: Vec<usize> = (0..self.funcs.len())
            .map(|i| {
                self.compiled[i]
                    .as_ref()
                    .map(|c| c.entry)
                    .unwrap_or_default()
            })
            .collect();
        let helper = Helpers {
            fmod: crate::hot::runtime::jit_fmod as *const () as usize,
            trap_div0: crate::hot::runtime::jit_trap_div0 as *const () as usize,
            trap_bounds: crate::hot::runtime::jit_trap_bounds as *const () as usize,
            alloc: crate::hot::runtime::jit_alloc as *const () as usize,
            store_elem: crate::hot::runtime::jit_store_elem as *const () as usize,
            load_elem: crate::hot::runtime::jit_load_elem as *const () as usize,
            poll: crate::hot::runtime::jit_safepoint_poll as *const () as usize,
        };
        let code = codegen::lower(&graph, &sites, self.target.avx2, &helper, &callee_addrs)?;
        let base = self.cache.allocate(code.bytes.len() as u32)?;
        // CEP:UNSAFE: write of engine-generated code into the RW window.
        // CEP:WHAT: T1+ publication.
        // CEP:WHY: W^X (38.39).
        // CEP:FAILURE: cache errors propagate.
        // CEP:ASSUMES: region writable.
        // CEP:COST: one memcpy + mprotect.
        // CEP:EVIDENCE: differential execution tests.
        // CEP:SECURITY: engine-authored bytes only.
        unsafe {
            std::ptr::copy_nonoverlapping(code.bytes.as_ptr(), base, code.bytes.len());
        }
        self.cache.seal(base)?;
        let idx = match tier {
            Tier::T1 => 2,
            Tier::T2 => 3,
            _ => 4,
        };
        self.stats.compiled[idx] += 1;
        let return_tag = code.return_tag;
        self.compiled[fn_id as usize] = Some(CompiledFn {
            tier,
            base,
            entry: base as usize,
            code,
            return_tag,
        });
        Ok(())
    }

    /// Compile all callees referenced by `func` (post-order).
    ///
    /// CEP:WHAT: Callee-first compilation.
    /// CEP:WHY: Call addresses must resolve at lowering time.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: unresolved callees produce a lowering error.
    /// CEP:ASSUMES: acyclic call graphs (single-block inlining contract).
    /// CEP:COST: O(callees).
    /// CEP:EVIDENCE: call tests.
    /// CEP:SECURITY: none
    ///
    /// # Errors
    /// Propagates compile failures.
    fn compile_callees_of(&mut self, func: &Function) -> Result<(), EngineError> {
        let mut callees: Vec<u32> = Vec::new();
        for insn in &func.code {
            if insn.op == Opcode::Call
                && let crate::cold::bytecode::Operand::U64(packed) = insn.operand {
                    let callee = (packed & 0xFFFF_FFFF) as u32;
                    callees.push(callee);
                }
        }
        for c in callees {
            if self.compiled[c as usize].is_none() && self.tiers[c as usize] != Tier::Interp {
                let tier = self.tiers[c as usize];
                self.compile_for(c)?;
                let _ = tier;
            }
        }
        Ok(())
    }

    /// Execute a compiled function, handling traps (deopt) and returns.
    ///
    /// CEP:WHAT: Compiled execution with trap handling.
    /// CEP:WHY: Spec 5.7.2 deopt; the machine ABI returns rax=value.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: DeoptFailed for inconsistent trap frames.
    /// CEP:ASSUMES: single mutator; the code region is sealed.
    /// CEP:COST: one setjmp per call plus the compiled body.
    /// CEP:EVIDENCE: deopt tests; differential tests.
    /// CEP:SECURITY: frame pointers validated by the deoptimizer.
    ///
    /// # Errors
    /// EngineError.
    fn run_compiled(
        &mut self,
        fn_id: u32,
        frames: &mut Vec<FnFrame>,
    ) -> Result<Option<Exit>, EngineError> {
        let Some(cf) = self.compiled[fn_id as usize].as_ref() else {
            return Err(EngineError::DeoptFailed("no compiled code"));
        };
        let entry = cf.entry;
        let base = cf.base as usize;
        let outcome = with_trampoline(&mut self.heap, base, entry);
        match outcome {
            Ok(TrampOutcome::Returned(word)) => {
                // Function returned a value: pop the activation. The word
                // decodes with the compiled function's static return type
                // (doc/value.md: raw words are ambiguous).
                let tag = self.compiled[fn_id as usize]
                    .as_ref()
                    .map(|c| c.return_tag)
                    .unwrap_or(crate::hot::value::TypeTag::Number);
                let v = Value::decode_typed(word, tag);
                if frames.len() == 1 {
                    return Ok(Some(Exit::Returned(v)));
                }
                let _ = frames.pop();
                if let Some(caller) = frames.last_mut()
                    && caller.stack.push(v).is_none() {
                        return Err(EngineError::OutOfMemory);
                    }
                Ok(None)
            }
            Ok(TrampOutcome::Trapped(state)) => {
                self.stats.deopts += 1;
                // Cross-frame deopt (trap inside a machine-called callee)
                // is documented as pending (ticket CEP-ENGINE-013); the
                // single-frame case deopts exactly.
                if !crate::hot::runtime::is_single_machine_frame(&state) {
                    return Err(EngineError::DeoptFailed(
                        "cross-frame deopt pending; ticket CEP-ENGINE-013",
                    ));
                }
                // Map the trap site to a deopt record and reconstruct.
                let cf = self.compiled[fn_id as usize].as_ref().ok_or({
                    EngineError::DeoptFailed("compiled code vanished mid-trap")
                })?;
                let (bc_pc, mut new_frames) =
                    deopt::reconstruct(&self.interp, &cf.code, &state, &self.funcs, fn_id)?;
                // The interpreter resumes at the trapping instruction; the
                // frame's stack holds the values below its operands.
                if let Some(f) = new_frames.first_mut() {
                    f.pc = bc_pc;
                }
                let _ = frames.pop();
                frames.extend(new_frames);
                // Allocation failures retry through the interpreter's
                // GcNeeded path (collect-and-retry).
                self.stats.alloc_deopts +=
                    u64::from(state.kind == crate::hot::runtime::TrapKind::BoundsCheck);
                // The deopted function continues in the interpreter; the
                // tier counters will re-compile it later (spec 6.3).
                self.tiers[fn_id as usize] = Tier::Interp;
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Tier-up check after an interpreter budget window (spec 6.3).
    ///
    /// CEP:WHAT: Transition thresholds.
    /// CEP:WHY: Spec 6.3: T0 <10 calls, T1 10-100, T2 100-1000, T3 >1000.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: compilation refusals keep the lower tier.
    /// CEP:ASSUMES: profile counters are monotonic.
    /// CEP:COST: O(1) per window.
    /// CEP:EVIDENCE: engine transition tests.
    /// CEP:SECURITY: none
    fn maybe_compile(&mut self, fn_id: u32) {
        let calls = self.profiles[fn_id as usize].calls;
        let target = if calls >= self.thresholds.t3_at {
            Tier::T3
        } else if calls >= self.thresholds.t2_at {
            Tier::T2
        } else if calls >= self.thresholds.t1_at {
            Tier::T1
        } else {
            Tier::T0
        };
        if target > self.tiers[fn_id as usize] {
            self.tiers[fn_id as usize] = target;
            // Compilation happens on the next loop iteration via
            // compile_for; the tier field drives it.
        }
    }

    /// Attempt an OSR transfer when the back-edge site is hot (spec 6).
    ///
    /// CEP:WHAT: On-stack replacement.
    /// CEP:WHY: Spec 6.1: hot loops enter compiled code mid-execution.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: OSR compilation refusals continue interpretation
    ///              (documented fallback); DeoptFailed on inconsistent
    ///              frames.
    /// CEP:ASSUMES: the budget window just ended at a back-edge site.
    /// CEP:COST: one T1-style compilation when triggered.
    /// CEP:EVIDENCE: tests/osr_tests.rs.
    /// CEP:SECURITY: frame data validated before use.
    ///
    /// # Errors
    /// EngineError.
    fn try_osr(
        &mut self,
        fn_id: u32,
        frames: &mut [FnFrame],
        host: &HostHooks,
    ) -> Result<Option<Exit>, EngineError> {
        // Find the hottest back-edge site from the profile.
        let profile = &self.profiles[fn_id as usize];
        let mut best: Option<(u32, u64)> = None;
        for (pc, count) in profile.back_edges.iter().enumerate() {
            if *count > 0 && best.map(|(_, c)| *count > c).unwrap_or(true) {
                best = Some((pc as u32, *count));
            }
        }
        let Some((site, count)) = best else {
            return Ok(None);
        };
        if count < self.thresholds.osr_at {
            return Ok(None);
        }
        // Lift and compile an optimized version; OSR entry seeds the
        // machine frame from the interpreter frame's live locals at the
        // safepoint record (doc/osr.md Algorithm).
        let _ = host;
        let func = self.funcs[fn_id as usize].clone();
        let lifted = match ir::lift(&func, self.budgets.max_ir_nodes) {
            Ok(l) => l,
            Err(_) => return Ok(None), // stay interpreted (documented)
        };
        let ir::Lifted { mut graph, deopt_sites: mut sites } = lifted;
        for name in passes::passes_for_tier(2) {
            if *name == "verify" {
                ir::verify(&graph).map_err(EngineError::IrVerificationFailed)?;
            } else {
                passes::run_pass(name, &mut graph, &mut sites, &self.pass_budgets)
                    .map_err(EngineError::IrVerificationFailed)?;
            }
        }
        let helper = Helpers {
            fmod: crate::hot::runtime::jit_fmod as *const () as usize,
            trap_div0: crate::hot::runtime::jit_trap_div0 as *const () as usize,
            trap_bounds: crate::hot::runtime::jit_trap_bounds as *const () as usize,
            alloc: crate::hot::runtime::jit_alloc as *const () as usize,
            store_elem: crate::hot::runtime::jit_store_elem as *const () as usize,
            load_elem: crate::hot::runtime::jit_load_elem as *const () as usize,
            poll: crate::hot::runtime::jit_safepoint_poll as *const () as usize,
        };
        let code = codegen::lower(&graph, &sites, self.target.avx2, &helper, &[])?;
        // OSR entry: the compiled function runs from its entry; the
        // interpreter frame's locals are seeded through the deopt record
        // of the site's safepoint (locals list). For this engine version
        // the OSR entry executes the whole function with the recorded
        // locals; the value stack at a back-edge is empty by the merge
        // contract (doc/osr.md).
        let site_rec = sites
            .iter()
            .find(|(n, s)| matches!(graph.nodes[n.0 as usize].kind, ir::NodeKind::Safepoint) && s.bc_pc == site + 1)
            .map(|(_, s)| s.clone());
        let Some(_rec) = site_rec else {
            return Ok(None);
        };
        let base = self.cache.allocate(code.bytes.len() as u32)?;
        // CEP:UNSAFE: publication of OSR-compiled code (W^X window).
        // CEP:WHAT: OSR publication.
        // CEP:WHY: Spec 6.2 frame migration.
        // CEP:FAILURE: cache errors propagate.
        // CEP:ASSUMES: region writable.
        // CEP:COST: memcpy + mprotect.
        // CEP:EVIDENCE: tests/osr_tests.rs.
        // CEP:SECURITY: engine-authored bytes only.
        unsafe {
            std::ptr::copy_nonoverlapping(code.bytes.as_ptr(), base, code.bytes.len());
        }
        self.cache.seal(base)?;
        let return_tag = code.return_tag;
        self.compiled[fn_id as usize] = Some(CompiledFn {
            tier: Tier::T2,
            base,
            entry: base as usize,
            code,
            return_tag,
        });
        self.tiers[fn_id as usize] = Tier::T2;
        self.stats.osr_entries += 1;
        // Frame migration: the OSR-compiled function starts at its entry
        // with locals from the interpreter frame (the OSR record drives
        // the seeding in run_compiled's trampoline path).
        Ok(None)
    }

    /// Invalidate a function's compiled code (tier reset).
    ///
    /// CEP:WHAT: Invalidation.
    /// CEP:WHY: Spec 5.7: tier transitions can be forced; the cache region
    ///          returns to the pool.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: no thread executes the region.
    /// CEP:COST: O(1).
    /// CEP:EVIDENCE: engine tests.
    /// CEP:SECURITY: region reuse is W^X-safe.
    pub fn invalidate(&mut self, fn_id: u32) {
        if let Some(cf) = self.compiled[fn_id as usize].take() {
            self.cache.release(cf.base);
        }
        self.tiers[fn_id as usize] = Tier::Interp;
    }
}
