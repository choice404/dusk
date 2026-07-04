//! The async task frame layout, the single source of truth both the poll
//! emission and the call site read so their byte offsets can never diverge.
//!
//! The C task header (48 bytes, owned by `cool_task`) sits BEFORE the frame; the
//! poll's `%frame` points at the frame base. The fixed prefix is state i64 @0,
//! pending data ptr @8, pending gen i64 @16, result @24 (max(sizeof(ret),1)
//! rounded to 8), then parameters in declaration order (each aligned), then
//! emission-order slots (each aligned). `frame_size` rounds the end up to 16.
//! Every alignment is at most 8 today; a wider one panics rather than silently
//! misaligning a frame GEP.

/// The state word, always at frame offset 0. The poll's entry switch reads it.
pub const STATE_OFF: u64 = 0;
/// The pending future's data pointer, written before a suspension (B3).
pub const PEND_D_OFF: u64 = 8;
/// The pending future's generation, written before a suspension (B3).
pub const PEND_G_OFF: u64 = 16;
/// The result bytes the async return path stores and `cool_task_return` copies.
pub const RESULT_OFF: u64 = 24;

/// Rounds `n` up to a multiple of `align`.
pub fn align_up(n: u64, align: u64) -> u64 {
    if align <= 1 {
        n
    } else {
        n.div_ceil(align) * align
    }
}

/// One frame slot minted while a poll body is lowered: the SSA name its entry
/// block GEP defines, the LLVM type it stores, the size and align that place it,
/// and the byte offset assigned in [`FrameCtx::finalize`].
pub struct Slot {
    pub name: String,
    pub llty: String,
    pub size: u64,
    pub align: u64,
    pub off: u64,
}

/// The fixed prefix offsets. Both the poll and the call site read `param_offs`,
/// so an argument the caller stores lands exactly where the poll reads it.
pub struct Prefix {
    pub result_off: u64,
    pub param_offs: Vec<u64>,
    pub first_free: u64,
}

/// Computes the shared prefix from the return type's `(size, align)` and each
/// parameter's `(size, align)` in declaration order. This one function, called
/// with the same signature inputs from both sides, is what keeps the poll's
/// parameter GEPs and the call site's argument stores at identical offsets.
pub fn frame_prefix(ret: (u64, u64), params: &[(u64, u64)]) -> Prefix {
    assert!(ret.1 <= 8, "an async return type wants alignment {} > 8", ret.1);
    let result_region = align_up(ret.0.max(1), 8);
    let mut off = RESULT_OFF + result_region;
    let mut param_offs = Vec::with_capacity(params.len());
    for &(size, align) in params {
        assert!(align <= 8, "an async parameter wants alignment {align} > 8");
        off = align_up(off, align);
        param_offs.push(off);
        off += size;
    }
    Prefix {
        result_off: RESULT_OFF,
        param_offs,
        first_free: off,
    }
}

/// The per-poll frame context: the prefix GEP names (minted from the poll's SSA
/// counter), the parameter GEPs with their offsets, and the emission-order
/// slots. `ret_size` is `sizeof(ret)` (0 for void), the byte count the async
/// return path hands `cool_task_return`.
pub struct FrameCtx {
    pub slots: Vec<Slot>,
    pub pend_d: String,
    pub pend_g: String,
    pub res: String,
    pub params: Vec<(String, u64)>,
    pub result_off: u64,
    pub ret_size: u64,
    pub first_free: u64,
    /// The number of await sites lowered so far; the next await is this plus one.
    /// Await state indices count from 1, leaving 0 for the initial `start` entry.
    pub await_count: u32,
    /// The (state index, resume label) pairs, one per await, added to the entry
    /// switch alongside state 0 -> start.
    pub resume_cases: Vec<(u64, String)>,
}

impl FrameCtx {
    /// Assigns emission-order slot offsets (each aligned) and returns the total
    /// frame size rounded up to 16. The prefix and parameters already carry
    /// their offsets from [`frame_prefix`].
    pub fn finalize(&mut self) -> u64 {
        let mut off = self.first_free;
        for slot in &mut self.slots {
            off = align_up(off, slot.align);
            slot.off = off;
            off += slot.size;
        }
        align_up(off, 16).max(16)
    }

    /// Synthesizes the prepended entry block: one `getelementptr` per prefix
    /// pointer and slot (every slot pointer is born here, so it dominates every
    /// resume edge), then the state load and the resume switch, then the
    /// bad-state trap. `state_tmp` is a fresh SSA name from the poll's counter;
    /// `cases` maps each live state index to its label, state 0 to `start`.
    pub fn entry_text(&self, state_tmp: &str, cases: &[(u64, String)]) -> String {
        let mut s = String::from("entry:\n");
        push_gep(&mut s, &self.pend_d, PEND_D_OFF);
        push_gep(&mut s, &self.pend_g, PEND_G_OFF);
        push_gep(&mut s, &self.res, self.result_off);
        for (name, off) in &self.params {
            push_gep(&mut s, name, *off);
        }
        for slot in &self.slots {
            push_gep(&mut s, &slot.name, slot.off);
        }
        s.push_str(&format!("  {state_tmp} = load i64, ptr %frame\n"));
        let arms = cases
            .iter()
            .map(|(v, l)| format!("i64 {v}, label %{l}"))
            .collect::<Vec<_>>()
            .join(" ");
        s.push_str(&format!(
            "  switch i64 {state_tmp}, label %badstate [ {arms} ]\n"
        ));
        s.push_str("badstate:\n  call void @cool_task_state_fault()\n  unreachable\n");
        s
    }
}

fn push_gep(s: &mut String, name: &str, off: u64) {
    s.push_str(&format!("  {name} = getelementptr i8, ptr %frame, i64 {off}\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_places_result_at_24_and_pads_to_8() {
        // A void return (size 0) still reserves an 8-byte result region, so the
        // first parameter starts at 32.
        let p = frame_prefix((0, 1), &[(8, 8)]);
        assert_eq!(p.result_off, 24);
        assert_eq!(p.param_offs, vec![32]);
        assert_eq!(p.first_free, 40);
    }

    #[test]
    fn prefix_rounds_small_return_region_to_8() {
        // int32 return: 4 bytes rounded to an 8-byte region; params follow at 32.
        let p = frame_prefix((4, 4), &[(8, 8), (8, 8)]);
        assert_eq!(p.param_offs, vec![32, 40]);
        assert_eq!(p.first_free, 48);
    }

    #[test]
    fn prefix_aligns_each_param() {
        // A 1-byte then an 8-byte param: the second aligns up to 40.
        let p = frame_prefix((8, 8), &[(1, 1), (8, 8)]);
        assert_eq!(p.param_offs, vec![32, 40]);
        assert_eq!(p.first_free, 48);
    }

    #[test]
    fn finalize_rounds_frame_to_16() {
        let mut fc = FrameCtx {
            slots: vec![Slot {
                name: "%s0".into(),
                llty: "i64".into(),
                size: 8,
                align: 8,
                off: 0,
            }],
            pend_d: "%pd".into(),
            pend_g: "%pg".into(),
            res: "%r".into(),
            params: vec![],
            result_off: 24,
            ret_size: 8,
            first_free: 32,
            await_count: 0,
            resume_cases: vec![],
        };
        let size = fc.finalize();
        assert_eq!(fc.slots[0].off, 32);
        assert_eq!(size, 48); // 40 rounded up to 16
    }

    #[test]
    fn entry_text_geps_every_slot_and_switches_on_state() {
        // One int64 param at 40 (ending at 48), so the emission slot lands at 48.
        let mut fc = FrameCtx {
            slots: vec![Slot {
                name: "%s0".into(),
                llty: "i64".into(),
                size: 8,
                align: 8,
                off: 0,
            }],
            pend_d: "%pd".into(),
            pend_g: "%pg".into(),
            res: "%r".into(),
            params: vec![("%p0".into(), 40)],
            result_off: 24,
            ret_size: 8,
            first_free: 48,
            await_count: 0,
            resume_cases: vec![],
        };
        let size = fc.finalize();
        assert_eq!(fc.slots[0].off, 48);
        assert_eq!(size, 64); // 56 rounded up to 16
        let t = fc.entry_text("%st", &[(0, "start".into())]);
        assert!(t.contains("%r = getelementptr i8, ptr %frame, i64 24"));
        assert!(t.contains("%p0 = getelementptr i8, ptr %frame, i64 40"));
        assert!(t.contains("%s0 = getelementptr i8, ptr %frame, i64 48"));
        assert!(t.contains("%st = load i64, ptr %frame"));
        assert!(t.contains("switch i64 %st, label %badstate [ i64 0, label %start ]"));
        assert!(t.contains("badstate:\n  call void @cool_task_state_fault()\n  unreachable"));
    }
}
