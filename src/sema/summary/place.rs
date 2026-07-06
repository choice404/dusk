//! Store routing and place-root analysis for the escape-summary walk: how a
//! stored value reaches its destination binding, and how a place expression roots
//! to a binding through fields, indexes, and dereferences. Split from `walk`,
//! which holds the statement and expression transfer, to keep each file focused.

use crate::parser::ast::*;

use super::walk::{AbsVal, FnState};
use super::ParamSet;

impl FnState<'_> {
    /// Records a view flowing into a store place through a call's store edge.
    /// The source's whole abstract value travels; the routing is `record_store`'s.
    pub(super) fn flow_into_place(&mut self, place: &Expr, src: AbsVal) {
        self.record_store(place, src, true);
    }

    /// Routes a stored value to its destination. The destination's caller
    /// reachability is decided by the root binding's own abstract value, not by
    /// its name: a parameter reaches the caller through its seeded bits, and a
    /// local that aliases a parameter's view (`origins`) or addresses a caller
    /// object (`reads`) reaches it the same way, so a borrow laundered through a
    /// call or a destructure routes identically to the parameter itself. Every
    /// caller-reaching store carries the source's `origins` and `reads` as
    /// `flows_into` edges; a direct store of a frame view additionally records a
    /// frame-store site, which the checker rejects outright (the view dies with
    /// this frame no matter what the caller does). A store rooted at a plain
    /// local raises that local's state for its own later return or store.
    pub(super) fn record_store(&mut self, place: &Expr, src: AbsVal, via_call: bool) {
        let Some(root) = dest_root(place) else {
            return;
        };
        let mut targets = ParamSet::default();
        if let Some(&j) = self.param_index.get(&root) {
            if self.dest.contains(&j) {
                targets = targets.union(ParamSet::single(j));
            }
        }
        if let Some(av) = self.env.get(&root) {
            targets = targets.union(av.origins).union(av.reads);
        }
        if !targets.is_empty() {
            for j in targets.iter() {
                for k in src.origins.union(src.reads).iter() {
                    self.flows_into.push((k, j));
                }
                if src.frame && !via_call {
                    self.frame_stores.push((place.span, j));
                }
            }
        }
        // A store rooted at a name that is neither a parameter nor a local of the
        // walked body targets a captured binding: free here, resolved in the
        // enclosing scope. Only the synthetic lambda transfer sees such a name (a
        // top-level function stores only through its parameters and locals), so
        // the edge is a lambda capture-flow: the parameter positions the stored
        // view came from, keyed against the captured name. The checker raises that
        // binding's escape flag at a call whose argument in that position is a
        // frame view, so a later egress of the binding is caught.
        if !self.param_index.contains_key(&root) && !self.locals.contains(&root) {
            for k in src.origins.union(src.reads).iter() {
                self.capture_flows.push((k, root.clone()));
            }
        }
        if !self.param_index.contains_key(&root) {
            self.raise_env(&root, src);
        }
    }

    pub(super) fn raise_env(&mut self, name: &str, v: AbsVal) {
        let cur = self.env.get(name).copied().unwrap_or_default();
        self.env.insert(name.to_string(), cur.join(v));
    }
}

/// Closes a function's sink set backward over its store edges. If argument `i`'s
/// view flows into argument `j`'s place (a `flows_into (i, j)` edge) and `j` is
/// sunk into a channel, then `i` reaches the same thread boundary the receiver
/// outlives, so `i` sinks too. This is a fixed-point backward reachability in the
/// `flows_into` graph seeded from the sink set: an added `i` may in turn pull in a
/// `k` with an edge `(k, i)`, so the pass iterates until the set stops growing.
///
/// It is what fuses the store hop and the send hop when a single body does both.
/// `evil(ch, c, s) { (*c).rows = s; chan_send(ch, c) }` records `flows_into (2, 1)`
/// and `sinks {1}`; without the closure, `s` (the frame view the caller supplies)
/// would cross the channel unchecked, because the call-site sink check reads the
/// caller state *before* the callee's store edge raises a flag, so the pointer in
/// position 1 still looks clean there. Closing lifts `sinks` to `{1, 2}`, so the
/// frame view in the source position `2` is caught at the call. The result only
/// grows the set, so it stays monotone and the enclosing fixpoint still converges.
pub(super) fn close_sinks_over_flows(seed: ParamSet, flows: &[(u8, u8)]) -> ParamSet {
    let mut sinks = seed;
    loop {
        let mut next = sinks;
        for &(i, j) in flows {
            if sinks.contains(j) {
                next = next.union(ParamSet::single(i));
            }
        }
        if next == sinks {
            return sinks;
        }
        sinks = next;
    }
}

/// The element type of a slice or array, else an inference hole.
pub(super) fn elem_of(t: &Type) -> Type {
    match t {
        Type::Slice(e) | Type::Array(e, _) => (**e).clone(),
        _ => Type::Infer,
    }
}

/// The base binding of a by-value projection chain, rooting through field
/// accesses and indexes but stopping at a pointer dereference, whose target is
/// the heap. Mirrors the intraprocedural `projection_root`.
pub(super) fn projection_root(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n.clone()),
        ExprKind::Field(base, _) | ExprKind::Index(base, _) => projection_root(base),
        _ => None,
    }
}

/// The pointer binding a by-value projection reads back through, when the chain
/// crosses a dereference: reading `(*p).f` or `(*p)[i]` reads whatever lives
/// behind `p`, so the projected value reads-through `p`. None when the chain has
/// no dereference (a plain local or parameter projection, handled by
/// `projection_root`), so the two roots stay disjoint.
pub(super) fn reads_through_root(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Field(base, _) | ExprKind::Index(base, _) => reads_through_root(base),
        ExprKind::Unary(UnOp::Deref, base) => dest_root(base),
        _ => None,
    }
}

/// The root binding of a store place, rooting through field accesses, indexes,
/// and pointer dereferences alike: a store through a parameter pointer reaches
/// the caller's heap object, so the dereference is followed here (unlike the
/// return-escape `projection_root`).
fn dest_root(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n.clone()),
        ExprKind::Field(base, _) | ExprKind::Index(base, _) => dest_root(base),
        ExprKind::Unary(UnOp::Deref, base) => dest_root(base),
        _ => None,
    }
}
