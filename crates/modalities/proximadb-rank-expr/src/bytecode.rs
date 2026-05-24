//! Compact bytecode the VM interprets.
//!
//! Each [`Op`] either pushes a value (constant or sub-feature output),
//! performs a unary/binary numeric operation, or jumps for the `if`
//! short-circuit. The hot loop in [`crate::vm::execute`] dispatches via
//! a single `match` so the branch predictor stays warm.

/// Index into [`crate::executor::ExprExecutor::sub_features`] — local to
/// one expression. `u16` is plenty: an expression that references more
/// than 64k sub-features would have hit the AST node cap first.
pub type SubFeatureIdx = u16;

/// Index into the bytecode `Vec<Op>`; absolute target for jumps. `u16`
/// matches the per-expression op cap (1024) with room to spare.
pub type PcTarget = u16;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    /// Push a numeric literal.
    PushConst(f32),
    /// Force a sub-feature and push its scalar output.
    PushSubFeature(SubFeatureIdx),

    // ---- arithmetic ----
    Add,
    Sub,
    Mul,
    Div,
    Neg,
    Pow,

    // ---- binary fns ----
    Min,
    Max,
    /// pops [val, lo, hi] (top = hi)
    Clamp,

    // ---- unary fns ----
    Abs,
    Sqrt,
    Log,
    Exp,
    Sigmoid,
    Relu,
    Tanh,

    // ---- control flow (used by `if`) ----
    /// pops cond; if `cond == 0.0`, set `pc = target`. else fall through.
    JumpIfZero(PcTarget),
    /// unconditional: `pc = target`.
    Jump(PcTarget),
}

/// Compiled expression: an op sequence plus the worst-case operand stack
/// depth the VM needs. Lowering computes the depth so callers can size
/// the SmallVec stack.
#[derive(Debug, Clone)]
pub struct Code {
    pub ops: Vec<Op>,
    pub max_stack: u16,
}

impl Code {
    pub fn len(&self) -> usize {
        self.ops.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_size_is_small() {
        // Layout-guard: a Vec<Op> for a 1024-op program should stay under
        // 16KB on common targets. f32 + 4-byte discriminant + padding
        // typically lands at 8 bytes.
        assert!(
            std::mem::size_of::<Op>() <= 12,
            "Op is {} bytes — bytecode density regressed",
            std::mem::size_of::<Op>()
        );
    }

    #[test]
    fn code_len_round_trip() {
        let c = Code {
            ops: vec![Op::PushConst(1.0), Op::PushConst(2.0), Op::Add],
            max_stack: 2,
        };
        assert_eq!(c.len(), 3);
        assert!(!c.is_empty());
    }
}
