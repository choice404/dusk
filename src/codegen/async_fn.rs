//! Async function lowering: one poll function over a heap frame per async func,
//! plus a framesize constant the call site loads. The body is generated first,
//! starting at `start:`; afterward the slot offsets are assigned and the entry
//! block is synthesized and prepended, so every frame slot pointer is born in
//! the entry block and dominates every resume edge. Awaits (B3) add resume
//! labels; B2 emits the state-0-only switch.

use crate::codegen::frame::{self, FrameCtx};
use crate::codegen::llvm::Module;
use crate::codegen::lower::{CTy, Ctx, Fb};
use crate::parser::ast::Func;

/// Emits `define void @async.<name>.poll(ptr %frame)` and its
/// `@async.<name>.framesize` constant. Parameters are not re-stored from
/// registers: the call site wrote them into the frame, and each param name maps
/// to its entry-block frame GEP.
pub(crate) fn gen_async_func(m: &mut Module, ctx: &Ctx, f: &Func) -> String {
    let (ret, params) = {
        let info = ctx
            .async_fns
            .get(&f.name)
            .expect("an async func is registered in async_fns");
        (info.ret.clone(), info.params.clone())
    };
    let ret_sa = ctx.size_align(&ret);
    let param_sa: Vec<(u64, u64)> = params.iter().map(|p| ctx.size_align(p)).collect();
    let prefix = frame::frame_prefix(ret_sa, &param_sa);
    // sizeof(ret), the byte count the async return path copies; a void return
    // copies nothing.
    let ret_size = if matches!(ret, CTy::Void) {
        0
    } else {
        ret_sa.0
    };

    let mut fb = Fb::new(m, ctx, ret);
    // Mint the prefix GEP names from the poll's SSA counter. The entry block
    // defines them, so they dominate the whole body and every resume edge.
    let pend_d = fb.fresh();
    let pend_g = fb.fresh();
    let res = fb.fresh();
    let mut param_geps = Vec::with_capacity(prefix.param_offs.len());
    for (i, off) in prefix.param_offs.iter().enumerate() {
        let name = fb.fresh();
        param_geps.push((name.clone(), *off));
        fb.locals
            .insert(f.params[i].name.clone(), (params[i].clone(), name));
    }
    fb.frame = Some(FrameCtx {
        slots: Vec::new(),
        pend_d,
        pend_g,
        res,
        params: param_geps,
        result_off: prefix.result_off,
        ret_size,
        first_free: prefix.first_free,
        await_count: 0,
        resume_cases: Vec::new(),
    });

    // The body runs at `start:`; state 0 dispatches here. Falling off the end
    // completes the task with no value, the bare-return shape.
    fb.place_label("start");
    fb.gen_block(&f.body.stmts);
    if !fb.terminated {
        fb.gen_async_return(None);
    }

    let frame_size = fb.frame.as_mut().expect("async frame is set").finalize();
    let state_tmp = fb.fresh();
    // State 0 enters `start`; each await registered a resume case during body
    // emission. The switch dispatches every live state, faulting on any other.
    let mut cases = vec![(0u64, "start".to_string())];
    cases.extend(
        fb.frame
            .as_ref()
            .expect("async frame is set")
            .resume_cases
            .iter()
            .cloned(),
    );
    let entry = fb
        .frame
        .as_ref()
        .expect("async frame is set")
        .entry_text(&state_tmp, &cases);
    let body = std::mem::take(&mut fb.body);
    fb.m.global(format!(
        "@async.{}.framesize = constant i64 {frame_size}",
        f.name
    ));
    format!(
        "define void @async.{}.poll(ptr %frame) {{\n{entry}{body}}}",
        f.name
    )
}
