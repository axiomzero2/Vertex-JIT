// CEP:FILE: engine/src/cold/compile.rs
// CEP:WHAT: Tiered compilation: T0 template publication, the T1-T3 IR
//           pipeline (lift, T3 inlining commit gates, passes, lowering,
//           W^X publication), callee-first post-order compilation with
//           the static call-graph cycle check, and the call-convention
//           arity table.
// CEP:WHY: Spec 2-5 define the compilation tiers and spec 5.2.1 the
//          inlining commit gates; the engine drives them from the run
//          loop (engine.rs). The compilation machinery is one cohesive
//          unit: every path ends in a W^X publication or a documented
//          refusal (tier back to Interp, interpreter exact).
// CEP:CLASS: CEP-1
// CEP:STATUS: complete
// CEP:FAILURE: Compilation refusals leave the tier at Interp (documented
//              fallback); cache failures propagate; a cyclic call graph
//              refuses before any work (doc/inline.md).
// CEP:ASSUMES: verified bytecode; single mutator; Linux x86-64.
// CEP:COST: budgeted per tier (38.39); the cycle check is O(reachable
//           instructions) per compilation.
// CEP:EVIDENCE: tests/differential.rs (all tiers), tests/tier23_tests.rs,
//               tests/call_inline_tests.rs (gates, cycles, inlining).
// CEP:SECURITY: generated code is engine-authored and published W^X;
//               the verifier gates every pass.

use crate::cold::bytecode::{self, Function, Opcode};
use crate::cold::codegen::{self, Helpers};
use crate::cold::engine::{CompiledFn, JitEngine};
use crate::cold::errors::EngineError;
use crate::cold::inline;
use crate::cold::ir::{self, LiftError};
use crate::cold::passes;
use crate::cold::tier::{Tier};
use crate::cold::tier0;

impl JitEngine {
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
    pub(super) fn compile_for(&mut self, fn_id: u32) -> Result<(), EngineError> {
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
                            interp_fn: fn_id,
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
    ///               interpreter bit-for-bit; tests/call_inline_tests.rs.
    /// CEP:SECURITY: the verifier gates every pass; publication is W^X.
    ///
    /// # Errors
    /// Cache failures propagate; pipeline failures fall back.
    pub(super) fn compile_optimized(&mut self, fn_id: u32, tier: Tier) -> Result<(), EngineError> {
        let func = self.funcs[fn_id as usize].clone();
        // Cyclic call graphs have no post order (a callee must publish
        // before its caller resolves the call address): refuse before any
        // work; interpreted execution is exact (doc/inline.md).
        if self.call_graph_has_cycle(fn_id) {
            self.tiers[fn_id as usize] = Tier::Interp;
            return Ok(());
        }
        let arity = self.arities[fn_id as usize] as usize;
        let lifted = match ir::lift_with_arity(&func, arity, self.budgets.max_ir_nodes) {
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
        let mut graph = lifted.graph;
        let mut sites = lifted.deopt_sites;
        // A T3 inlined compilation splices the merged body in here and
        // registers its interpreter image after all guards pass (the
        // deopt resume then runs on bytecode whose pc and slot space
        // match the compiled code, doc/inline.md).
        let mut merged_body: Option<Function> = None;
        let mut inlined_now: u64 = 0;
        if tier == Tier::T3 {
            let outcome = inline::inline_calls(
                &func,
                fn_id,
                &self.funcs,
                &self.arities,
                &self.pass_budgets,
                self.budgets.max_ir_nodes,
            );
            if outcome.inlined_calls > 0 {
                let Some(merged) = outcome.merged else {
                    unreachable!("inlined_calls > 0 implies a merged body");
                };
                // Two commit gates (grace degradation, doc/inline.md):
                // the merged body must lift, and every call left in it
                // must target a compilable callee (tier above Interp) —
                // compile_callees_of publishes exactly those before the
                // call addresses resolve. A refused nested call to any
                // other function would otherwise lower as a call to an
                // unresolved address.
                if let Ok(mlifted) =
                    ir::lift_with_arity(&merged, arity, self.budgets.max_ir_nodes)
                {
                    let targets_compilable = merged.code.iter().all(|i| {
                        i.op != Opcode::Call
                            || match i.operand {
                                bytecode::Operand::U64(p) => {
                                    let c = (p & 0xFFFF_FFFF) as usize;
                                    self.tiers.get(c).is_some_and(|t| *t != Tier::Interp)
                                }
                                _ => false,
                            }
                    });
                    if targets_compilable {
                        merged_body = Some(merged);
                        inlined_now = outcome.inlined_calls;
                        graph = mlifted.graph;
                        sites = mlifted.deopt_sites;
                    }
                }
            }
        }
        for name in passes::passes_for_tier(match tier {
            Tier::T1 => 1,
            Tier::T2 => 2,
            _ => 3,
        }) {
            // Verify/pass failures mean the graph left the verifiable
            // subset (for example a called body with several returns
            // lifts to more than one Return node): fall back to the
            // interpreter exactly as lift failures do (the documented
            // contract; the interpreter is exact).
            if *name == "verify" {
                if ir::verify(&graph).is_err() {
                    self.tiers[fn_id as usize] = Tier::Interp;
                    return Ok(());
                }
            } else if passes::run_pass(name, &mut graph, &mut sites, &self.pass_budgets).is_err()
            {
                self.tiers[fn_id as usize] = Tier::Interp;
                return Ok(());
            }
        }
        if ir::verify(&graph).is_err() {
            self.tiers[fn_id as usize] = Tier::Interp;
            return Ok(());
        }

        // Callees must be compiled before callers resolve call addresses.
        self.compile_callees_of(&func)?;
        // Every call the lowered code makes must resolve to a published
        // address; an Interp-tier callee has none, so the compilation
        // refuses and the interpreter runs exactly (doc/inline.md).
        let unresolved = graph.nodes.iter().any(|n| match n.kind {
            ir::NodeKind::Call { callee } => {
                let c = callee as usize;
                self.compiled.get(c).is_none_or(|s| s.is_none())
                    && self.tiers.get(c) == Some(&Tier::Interp)
            }
            _ => false,
        });
        if unresolved {
            self.tiers[fn_id as usize] = Tier::Interp;
            return Ok(());
        }
        // The interpreter image for deopt resumes: fn_id itself, or the
        // merged body registered now that every guard passed (stats and
        // the function table only ever see published merges).
        let interp_fn = match merged_body {
            Some(m) => {
                let image_id = if let Some(existing) = self.merged_of[fn_id as usize] {
                    existing
                } else {
                    let id = self.register_image(m)?;
                    self.merged_of[fn_id as usize] = Some(id);
                    id
                };
                self.stats.inlined_calls += inlined_now;
                image_id
            }
            None => fn_id,
        };
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
            trap_div0: crate::hot::trap::jit_trap_div0 as *const () as usize,
            trap_bounds: crate::hot::trap::jit_trap_bounds as *const () as usize,
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
            interp_fn,
        });
        Ok(())
    }

    /// Detect a cycle in the static call graph reachable from `root`.
    ///
    /// CEP:WHAT: Call-graph cycle detection.
    /// CEP:WHY: Post-order compilation publishes a callee before its
    ///          caller; a cycle (self-recursion or mutual) has no post
    ///          order, and compiling one would recurse forever or lower
    ///          an unresolved address. Functions on a cycle compile
    ///          never: they stay on the interpreter, which is exact
    ///          (doc/inline.md Known Limitations).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none (pure query).
    /// CEP:ASSUMES: Call operands pack (argc<<32)|callee.
    /// CEP:COST: O(reachable instructions) per compilation.
    /// CEP:EVIDENCE: tests/call_inline_tests.rs recursion test.
    /// CEP:SECURITY: none
    pub(super) fn call_graph_has_cycle(&self, root: u32) -> bool {
        /// One depth-first visit: a back edge to a node on the current
        /// path is a cycle; finished nodes cannot close one.
        ///
        /// CEP:WHAT: Cycle DFS.
        /// CEP:WHY: The post-order compilation contract needs a static
        ///          cycle answer before any compilation work.
        /// CEP:STATUS: complete
        /// CEP:FAILURE: none (pure query).
        /// CEP:ASSUMES: Call operands pack (argc<<32)|callee.
        /// CEP:COST: O(edges) across the whole search.
        /// CEP:EVIDENCE: tests/call_inline_tests.rs recursion test.
        /// CEP:SECURITY: none
        fn dfs(
            funcs: &[Function],
            id: u32,
            visiting: &mut std::collections::BTreeSet<u32>,
            done: &mut std::collections::BTreeSet<u32>,
        ) -> bool {
            if done.contains(&id) {
                return false;
            }
            if !visiting.insert(id) {
                return true; // back edge to a node on the current path
            }
            let mut cyc = false;
            if let Some(f) = funcs.get(id as usize) {
                for insn in &f.code {
                    if insn.op == Opcode::Call
                        && let bytecode::Operand::U64(p) = insn.operand
                        && dfs(funcs, (p & 0xFFFF_FFFF) as u32, visiting, done)
                    {
                        cyc = true;
                        break;
                    }
                }
            }
            visiting.remove(&id);
            done.insert(id);
            cyc
        }
        let mut visiting = std::collections::BTreeSet::new();
        let mut done = std::collections::BTreeSet::new();
        dfs(&self.funcs, root, &mut visiting, &mut done)
    }

    /// Compile all callees referenced by `func` (post-order).
    ///
    /// CEP:WHAT: Callee-first compilation.
    /// CEP:WHY: Call addresses must resolve at lowering time.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: unresolved callees produce a lowering error.
    /// CEP:ASSUMES: acyclic call graphs (the static cycle check ran).
    /// CEP:COST: O(callees).
    /// CEP:EVIDENCE: tests/call_inline_tests.rs machine-call tests.
    /// CEP:SECURITY: none
    ///
    /// # Errors
    /// Propagates compile failures.
    pub(super) fn compile_callees_of(&mut self, func: &Function) -> Result<(), EngineError> {
        let mut callees: Vec<u32> = Vec::new();
        for insn in &func.code {
            if insn.op == Opcode::Call
                && let bytecode::Operand::U64(packed) = insn.operand
            {
                let callee = (packed & 0xFFFF_FFFF) as u32;
                callees.push(callee);
            }
        }
        for c in callees {
            if self.compiled[c as usize].is_none() && self.tiers[c as usize] != Tier::Interp {
                self.compile_for(c)?;
            }
        }
        Ok(())
    }
}

/// Derive the call-convention arity table: per function, the maximum argc
/// over all Call sites targeting it (doc/bytecode.md).
///
/// CEP:WHAT: Arity derivation.
/// CEP:WHY: The bytecode model carries no arity field; the lift builder
///          needs the parameter count to emit Param nodes (doc/ir.md).
/// CEP:STATUS: complete
/// CEP:FAILURE: none (uncalled functions get arity 0).
/// CEP:ASSUMES: Call operands pack (argc<<32)|callee.
/// CEP:COST: O(total instructions).
/// CEP:EVIDENCE: tests/call_inline_tests.rs arity tests.
/// CEP:SECURITY: none
pub(super) fn compute_arities(funcs: &[Function]) -> Vec<u32> {
    let mut arities = vec![0u32; funcs.len()];
    for f in funcs {
        for insn in &f.code {
            if insn.op == Opcode::Call
                && let bytecode::Operand::U64(packed) = insn.operand
            {
                let argc = (packed >> 32) as u32;
                let callee = (packed & 0xFFFF_FFFF) as usize;
                if let Some(a) = arities.get_mut(callee) {
                    *a = (*a).max(argc);
                }
            }
        }
    }
    arities
}
