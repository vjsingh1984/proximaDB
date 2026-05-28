//! AST → bytecode lowering.
//!
//! Splits AST `Call(name, args)` into two paths:
//!
//! - **Built-in function** (max, min, log, exp, pow, sqrt, sigmoid, relu,
//!   tanh, clamp, abs): emit the corresponding [`Op`] after lowering args.
//! - **Feature reference** (`bm25`, `closeness`, `attribute`, …): look the
//!   blueprint up in the provided [`BlueprintFactory`], extract literal
//!   args, instantiate a `Box<dyn FeatureExecutor>` and push it into
//!   `sub_features`; emit `Op::PushSubFeature(idx)`.
//!
//! `if(cond, then, else)` lowers to jump-based short-circuit so the
//! unused branch never executes its sub-features. This is critical when
//! the then/else arms reference expensive features (R-5 ONNX models).

use crate::ast::{Ast, BinOp};
use crate::bytecode::{Code, Op, SubFeatureIdx};
use proximadb_rank_core::{
    BlueprintFactory, FeatureExecutor, PhaseConfig, QueryContext, RankError, RankResult,
};

const MAX_OPS: usize = 1024;

const BUILTIN_FNS: &[(&str, BuiltinKind)] = &[
    ("max", BuiltinKind::Max),
    ("min", BuiltinKind::Min),
    ("log", BuiltinKind::Log),
    ("exp", BuiltinKind::Exp),
    ("pow", BuiltinKind::Pow),
    ("sqrt", BuiltinKind::Sqrt),
    ("sigmoid", BuiltinKind::Sigmoid),
    ("relu", BuiltinKind::Relu),
    ("tanh", BuiltinKind::Tanh),
    ("clamp", BuiltinKind::Clamp),
    ("abs", BuiltinKind::Abs),
];

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum BuiltinKind {
    Min,
    Max,
    Log,
    Exp,
    Pow,
    Sqrt,
    Sigmoid,
    Relu,
    Tanh,
    Clamp,
    Abs,
}

fn lookup_builtin(name: &str) -> Option<BuiltinKind> {
    BUILTIN_FNS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, k)| *k)
}

/// Lower an AST to bytecode plus the owned sub-feature executors.
///
/// `factory` resolves non-builtin `Call(name, ...)` nodes. `qctx` is
/// forwarded to each blueprint's `build_executor` so query-scoped
/// configuration (tenant, query vector, deadline) flows through.
pub fn lower(
    ast: &Ast,
    factory: &BlueprintFactory,
    qctx: &QueryContext,
) -> RankResult<(Code, Vec<Box<dyn FeatureExecutor>>)> {
    let mut ctx = LowerCtx {
        ops: Vec::new(),
        sub_features: Vec::new(),
        factory,
        qctx,
    };
    let stack = ctx.lower(ast, 0)?;
    if ctx.ops.len() > MAX_OPS {
        return Err(RankError::ExpressionParse(format!(
            "expression compiled to {} ops, exceeds cap of {}",
            ctx.ops.len(),
            MAX_OPS
        )));
    }
    Ok((
        Code {
            ops: ctx.ops,
            max_stack: stack,
        },
        ctx.sub_features,
    ))
}

struct LowerCtx<'a> {
    ops: Vec<Op>,
    sub_features: Vec<Box<dyn FeatureExecutor>>,
    factory: &'a BlueprintFactory,
    qctx: &'a QueryContext,
}

impl LowerCtx<'_> {
    /// Returns the max operand-stack depth needed to evaluate `ast`,
    /// assuming the current_stack depth is `current`.
    fn lower(&mut self, ast: &Ast, current: u16) -> RankResult<u16> {
        match ast {
            Ast::Num(n) => {
                self.emit(Op::PushConst(*n as f32));
                Ok(current + 1)
            }
            Ast::Str(_) => Err(RankError::ExpressionType(
                "string literal cannot appear as a top-level expression value".into(),
            )),
            Ast::Neg(inner) => {
                let s = self.lower(inner, current)?;
                self.emit(Op::Neg);
                Ok(s)
            }
            Ast::Bin(op, l, r) => self.lower_bin(*op, l, r, current),
            Ast::If(cond, then_b, else_b) => self.lower_if(cond, then_b, else_b, current),
            Ast::Call(name, args) => self.lower_call(name, args, current),
        }
    }

    fn lower_bin(&mut self, op: BinOp, l: &Ast, r: &Ast, current: u16) -> RankResult<u16> {
        let s1 = self.lower(l, current)?;
        let s2 = self.lower(r, current + 1)?;
        self.emit(match op {
            BinOp::Add => Op::Add,
            BinOp::Sub => Op::Sub,
            BinOp::Mul => Op::Mul,
            BinOp::Div => Op::Div,
            BinOp::Pow => Op::Pow,
        });
        Ok(s1.max(s2))
    }

    fn lower_if(
        &mut self,
        cond: &Ast,
        then_b: &Ast,
        else_b: &Ast,
        current: u16,
    ) -> RankResult<u16> {
        // Layout:
        //   <cond ops>
        //   JumpIfZero target_else
        //   <then ops>
        //   Jump target_end
        //   <else ops>
        //
        // We back-patch the two jump targets after we know the offsets.
        let s_cond = self.lower(cond, current)?;
        // After cond runs, the cond value is on the stack and the
        // JumpIfZero pops it. So stack depth at the branch point is `current`.
        let jiz_pc = self.emit_placeholder(Op::JumpIfZero(0));
        let s_then = self.lower(then_b, current)?;
        let jmp_pc = self.emit_placeholder(Op::Jump(0));
        let else_target = self.ops.len();
        let s_else = self.lower(else_b, current)?;
        let end_target = self.ops.len();
        self.patch(jiz_pc, Op::JumpIfZero(checked_target(else_target)?));
        self.patch(jmp_pc, Op::Jump(checked_target(end_target)?));
        Ok(s_cond.max(s_then).max(s_else))
    }

    fn lower_call(&mut self, name: &str, args: &[Ast], current: u16) -> RankResult<u16> {
        if let Some(kind) = lookup_builtin(name) {
            return self.lower_builtin(kind, name, args, current);
        }
        // Feature reference path.
        let bp = self.factory.require(name)?;
        let phase_cfg = literal_args_for_feature(name, args)?;
        let exec = bp.build_executor(&phase_cfg, self.qctx)?;
        if self.sub_features.len() >= u16::MAX as usize {
            return Err(RankError::ExpressionParse(
                "too many sub-feature references in one expression".into(),
            ));
        }
        let idx = self.sub_features.len() as SubFeatureIdx;
        self.sub_features.push(exec);
        self.emit(Op::PushSubFeature(idx));
        Ok(current + 1)
    }

    fn lower_builtin(
        &mut self,
        kind: BuiltinKind,
        name: &str,
        args: &[Ast],
        current: u16,
    ) -> RankResult<u16> {
        let (expected, op) = match kind {
            BuiltinKind::Min => (2, Op::Min),
            BuiltinKind::Max => (2, Op::Max),
            BuiltinKind::Log => (1, Op::Log),
            BuiltinKind::Exp => (1, Op::Exp),
            BuiltinKind::Pow => (2, Op::Pow),
            BuiltinKind::Sqrt => (1, Op::Sqrt),
            BuiltinKind::Sigmoid => (1, Op::Sigmoid),
            BuiltinKind::Relu => (1, Op::Relu),
            BuiltinKind::Tanh => (1, Op::Tanh),
            BuiltinKind::Clamp => (3, Op::Clamp),
            BuiltinKind::Abs => (1, Op::Abs),
        };
        if args.len() != expected {
            return Err(RankError::ExpressionParse(format!(
                "{name}(...) takes exactly {expected} arguments, got {}",
                args.len()
            )));
        }
        let mut max_stack = 0;
        for (i, a) in args.iter().enumerate() {
            let s = self.lower(a, current + i as u16)?;
            max_stack = max_stack.max(s);
        }
        self.emit(op);
        Ok(max_stack)
    }

    fn emit(&mut self, op: Op) {
        self.ops.push(op);
    }

    fn emit_placeholder(&mut self, op: Op) -> usize {
        let pc = self.ops.len();
        self.ops.push(op);
        pc
    }

    fn patch(&mut self, pc: usize, op: Op) {
        self.ops[pc] = op;
    }
}

fn checked_target(idx: usize) -> RankResult<u16> {
    u16::try_from(idx).map_err(|_| {
        RankError::ExpressionParse(format!("jump target {idx} exceeds u16 bytecode size"))
    })
}

/// Extract literal-only arguments suitable for a [`Blueprint::build_executor`]
/// call. Feature references can only accept `Num(...)` or `Str(...)` AST
/// nodes — nested expressions are rejected so the blueprint's literal_args
/// surface stays type-stable.
fn literal_args_for_feature(name: &str, args: &[Ast]) -> RankResult<PhaseConfig> {
    let mut out = Vec::with_capacity(args.len());
    for (i, a) in args.iter().enumerate() {
        match a {
            Ast::Str(s) => out.push(s.clone()),
            Ast::Num(n) => out.push(format_number(*n)),
            // A bare identifier was parsed as Call(name, []) — allow that
            // shorthand for backwards compatibility with `closeness(embedding)`-
            // style references where the field is unquoted.
            Ast::Call(ident, sub_args) if sub_args.is_empty() => out.push(ident.clone()),
            other => {
                return Err(RankError::ExpressionType(format!(
                    "feature {name}(...) arg {i}: expected string or number literal, got non-trivial expression ({other:?})"
                )));
            }
        }
    }
    Ok(PhaseConfig { literal_args: out })
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use proximadb_rank_core::{Blueprint, DocHandle, FeatureLookup, OutputSpec, ScoreCtx};
    use std::sync::Arc;

    // Minimal stub blueprint for sub-feature lowering tests.
    struct ConstBp {
        name: &'static str,
        value: f32,
    }
    struct ConstEx(f32);
    impl FeatureExecutor for ConstEx {
        fn execute(
            &mut self,
            _doc: DocHandle,
            _lookup: &mut dyn FeatureLookup,
            _ctx: &mut ScoreCtx<'_>,
        ) -> f32 {
            self.0
        }
    }
    impl Blueprint for ConstBp {
        fn name(&self) -> &str {
            self.name
        }
        fn declared_outputs(&self) -> &[OutputSpec] {
            const OUT: &[OutputSpec] = &[];
            OUT
        }
        fn build_executor(
            &self,
            _cfg: &PhaseConfig,
            _q: &QueryContext,
        ) -> RankResult<Box<dyn FeatureExecutor>> {
            Ok(Box::new(ConstEx(self.value)))
        }
    }

    fn factory_with(features: &[(&'static str, f32)]) -> BlueprintFactory {
        let f = BlueprintFactory::new();
        for (n, v) in features {
            f.register(Arc::new(ConstBp { name: n, value: *v }));
        }
        f
    }

    #[test]
    fn lower_constant_emits_push_const() {
        let ast = parse("42").unwrap();
        let f = BlueprintFactory::new();
        let q = QueryContext::default();
        let (code, subs) = lower(&ast, &f, &q).unwrap();
        assert_eq!(code.ops, vec![Op::PushConst(42.0)]);
        assert_eq!(code.max_stack, 1);
        assert!(subs.is_empty());
    }

    #[test]
    fn lower_simple_arithmetic() {
        let ast = parse("1 + 2 * 3").unwrap();
        let f = BlueprintFactory::new();
        let q = QueryContext::default();
        let (code, _) = lower(&ast, &f, &q).unwrap();
        // Expected: PushConst(1), PushConst(2), PushConst(3), Mul, Add
        assert_eq!(
            code.ops,
            vec![
                Op::PushConst(1.0),
                Op::PushConst(2.0),
                Op::PushConst(3.0),
                Op::Mul,
                Op::Add,
            ]
        );
        assert_eq!(code.max_stack, 3);
    }

    #[test]
    fn lower_unary_neg() {
        let ast = parse("-5").unwrap();
        let f = BlueprintFactory::new();
        let q = QueryContext::default();
        let (code, _) = lower(&ast, &f, &q).unwrap();
        assert_eq!(code.ops, vec![Op::PushConst(5.0), Op::Neg]);
    }

    #[test]
    fn lower_with_feature_ref() {
        let ast = parse("bm25(\"title\") * 0.4").unwrap();
        let f = factory_with(&[("bm25", 9.9)]);
        let q = QueryContext::default();
        let (code, subs) = lower(&ast, &f, &q).unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(
            code.ops,
            vec![Op::PushSubFeature(0), Op::PushConst(0.4), Op::Mul]
        );
    }

    #[test]
    fn lower_unknown_feature_errors() {
        let ast = parse("frobnicate(\"x\")").unwrap();
        let f = BlueprintFactory::new();
        let q = QueryContext::default();
        match lower(&ast, &f, &q) {
            Err(RankError::UnknownFeature(name)) => assert_eq!(name, "frobnicate"),
            Err(_) => panic!("expected UnknownFeature, got a different error"),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn lower_unquoted_field_shorthand() {
        // `closeness(embedding)` parses as Call("closeness", [Call("embedding", [])]).
        // Lowering should treat the bare ident as a string literal arg.
        let ast = parse("closeness(embedding)").unwrap();
        let f = factory_with(&[("closeness", 0.5)]);
        let q = QueryContext::default();
        let (_code, subs) = lower(&ast, &f, &q).unwrap();
        assert_eq!(subs.len(), 1);
    }

    #[test]
    fn lower_builtin_arity_check() {
        let ast = parse("max(1)").unwrap();
        let f = BlueprintFactory::new();
        let q = QueryContext::default();
        match lower(&ast, &f, &q) {
            Err(RankError::ExpressionParse(msg)) => assert!(msg.contains("max")),
            Err(_) => panic!("expected ExpressionParse, got a different error"),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn lower_clamp_takes_3() {
        let ast = parse("clamp(1, 0, 2)").unwrap();
        let f = BlueprintFactory::new();
        let q = QueryContext::default();
        let (code, _) = lower(&ast, &f, &q).unwrap();
        assert!(matches!(code.ops.last(), Some(Op::Clamp)));
    }

    #[test]
    fn lower_if_emits_jumps() {
        let ast = parse("if(1, 2, 3)").unwrap();
        let f = BlueprintFactory::new();
        let q = QueryContext::default();
        let (code, _) = lower(&ast, &f, &q).unwrap();
        // PushConst(1), JumpIfZero(else_target), PushConst(2), Jump(end), PushConst(3)
        assert_eq!(code.ops[0], Op::PushConst(1.0));
        match code.ops[1] {
            Op::JumpIfZero(t) => assert_eq!(t as usize, 4), // skip ahead to PushConst(3)
            ref other => panic!("expected JumpIfZero, got {other:?}"),
        }
        assert_eq!(code.ops[2], Op::PushConst(2.0));
        match code.ops[3] {
            Op::Jump(t) => assert_eq!(t as usize, 5), // skip past PushConst(3)
            ref other => panic!("expected Jump, got {other:?}"),
        }
        assert_eq!(code.ops[4], Op::PushConst(3.0));
        assert_eq!(code.ops.len(), 5);
    }

    #[test]
    fn lower_feature_with_complex_arg_errors() {
        // bm25(1+2) should reject — feature args must be literals.
        let ast = parse("bm25(1+2)").unwrap();
        let f = factory_with(&[("bm25", 1.0)]);
        let q = QueryContext::default();
        match lower(&ast, &f, &q) {
            Err(RankError::ExpressionType(_)) => {}
            Err(_) => panic!("expected ExpressionType, got a different error"),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn lower_string_at_top_level_errors() {
        // A bare string can't be the value of an expression.
        let ast = Ast::Str("oops".into());
        let f = BlueprintFactory::new();
        let q = QueryContext::default();
        match lower(&ast, &f, &q) {
            Err(RankError::ExpressionType(_)) => {}
            Err(_) => panic!("expected ExpressionType, got a different RankError"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn format_number_uses_integer_form_when_possible() {
        assert_eq!(format_number(42.0), "42");
        assert_eq!(format_number(3.5), "3.5");
    }
}
