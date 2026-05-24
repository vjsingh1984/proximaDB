//! Abstract syntax tree for ranking expressions.

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ast {
    /// Numeric literal. Always non-negative in the AST — unary `-` wraps
    /// the AST in `Neg(...)` and the lowering pass folds constants.
    Num(f64),
    /// String literal: only appears as a feature-call argument.
    Str(String),
    /// Unary negation.
    Neg(Box<Ast>),
    /// Binary arithmetic (+, -, *, /, ^).
    Bin(BinOp, Box<Ast>, Box<Ast>),
    /// `if(cond, then, else)` — special-cased so the VM can short-circuit
    /// to one branch.
    If(Box<Ast>, Box<Ast>, Box<Ast>),
    /// `name(args…)` — either a built-in function or a feature reference.
    /// Distinguished by [`lowering`]. Args may be any expression; for
    /// feature references they must reduce to literal numbers or strings.
    Call(String, Vec<Ast>),
}

impl Ast {
    /// Recursive depth — used by the parser/typecheck to enforce a
    /// hard cap (default 256) per the spec.
    pub fn depth(&self) -> usize {
        match self {
            Ast::Num(_) | Ast::Str(_) => 1,
            Ast::Neg(inner) => 1 + inner.depth(),
            Ast::Bin(_, l, r) => 1 + l.depth().max(r.depth()),
            Ast::If(c, t, e) => 1 + c.depth().max(t.depth()).max(e.depth()),
            Ast::Call(_, args) => 1 + args.iter().map(|a| a.depth()).max().unwrap_or(0),
        }
    }

    /// Total number of AST nodes — used to enforce the per-expression
    /// op-count cap (default 1024).
    pub fn node_count(&self) -> usize {
        match self {
            Ast::Num(_) | Ast::Str(_) => 1,
            Ast::Neg(inner) => 1 + inner.node_count(),
            Ast::Bin(_, l, r) => 1 + l.node_count() + r.node_count(),
            Ast::If(c, t, e) => 1 + c.node_count() + t.node_count() + e.node_count(),
            Ast::Call(_, args) => 1 + args.iter().map(|a| a.node_count()).sum::<usize>(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_of_leaf_is_one() {
        assert_eq!(Ast::Num(0.0).depth(), 1);
        assert_eq!(Ast::Str("x".into()).depth(), 1);
    }

    #[test]
    fn depth_of_nested_neg() {
        let a = Ast::Neg(Box::new(Ast::Neg(Box::new(Ast::Num(1.0)))));
        assert_eq!(a.depth(), 3);
    }

    #[test]
    fn depth_takes_max_of_branches() {
        let l = Ast::Num(1.0);
        let r = Ast::Bin(
            BinOp::Add,
            Box::new(Ast::Num(2.0)),
            Box::new(Ast::Bin(
                BinOp::Add,
                Box::new(Ast::Num(3.0)),
                Box::new(Ast::Num(4.0)),
            )),
        );
        let root = Ast::Bin(BinOp::Add, Box::new(l), Box::new(r));
        // depth(l)=1, depth(r)=3 (Bin + Bin + Num), so root=1+3=4
        assert_eq!(root.depth(), 4);
    }

    #[test]
    fn node_count_sums_subtrees() {
        let call = Ast::Call(
            "max".into(),
            vec![Ast::Num(1.0), Ast::Num(2.0), Ast::Num(3.0)],
        );
        assert_eq!(call.node_count(), 4); // Call + 3 Nums
    }
}
