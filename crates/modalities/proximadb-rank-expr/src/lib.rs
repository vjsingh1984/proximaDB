//! ProximaDB ranking expression DSL — parser + bytecode VM.
//!
//! Grammar (PEG, per spec §4.2.1):
//! ```text
//! expr      <- add
//! add       <- mul (('+' / '-') mul)*
//! mul       <- pow (('*' / '/') pow)*
//! unary     <- '-' unary / pow
//! pow       <- atom ('^' unary)?         # right-associative
//! atom      <- number / string / call / paren
//! paren     <- '(' expr ')'
//! call      <- ident ('(' (expr (',' expr)*)? ')')?
//! ident     <- [A-Za-z_][A-Za-z0-9_]*
//! number    <- digit+ ('.' digit+)? ([eE][-+]?digit+)?   # unary '-' supplies sign
//! string    <- '"' [^"]* '"' / '\'' [^']* '\''
//! ```
//!
//! Built-in functions (handled by [`vm`] / [`lowering`]):
//! `max`, `min`, `if`, `log`, `exp`, `pow`, `sqrt`, `sigmoid`, `relu`,
//! `tanh`, `clamp`, `abs`.
//!
//! All other `ident(args…)` calls resolve via [`proximadb_rank_core::BlueprintFactory`]
//! and become **owned sub-features** of the resulting [`executor::ExprExecutor`].
//! R-3 deliberately keeps sub-features local to the expression's evaluation
//! context — cross-expression memoization comes when the R-4 DAG resolver
//! lands.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` (R-3).

pub mod ast;
pub mod bytecode;
pub mod executor;
pub mod lowering;
pub mod parser;
pub mod vm;

pub use ast::{Ast, BinOp};
pub use bytecode::{Code, Op};
pub use executor::{ExprBlueprint, ExprExecutor};
pub use lowering::lower;
pub use parser::parse;
pub use vm::execute;
