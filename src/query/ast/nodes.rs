//! Internal AST nodes for query representation.

#[derive(Debug, Clone)]
pub enum Query {
    Select(Select),
    /// WITH ctes AS (...) SELECT ...
    With {
        ctes: Vec<Cte>,
        query: Box<Query>,
    },
    /// Set operations like UNION/INTERSECT/EXCEPT
    Set {
        left: Box<Query>,
        op: SetOp,
        all: bool,
        right: Box<Query>,
    },
}

#[derive(Debug, Clone)]
pub struct Select {
    pub projection: Vec<ProjectionItem>,
    pub from: Vec<TableRef>,
    pub joins: Vec<Join>,
    pub selection: Option<Expr>,
    pub group_by: Vec<Expr>,
    pub having: Option<Expr>,
    pub order_by: Vec<OrderByExpr>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ProjectionItem {
    pub expr: Expr,
    pub alias: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Cte {
    pub name: String,
    pub query: Box<Query>,
}

#[derive(Debug, Clone)]
pub enum SetOp {
    Union,
    Intersect,
    Except,
}

#[derive(Debug, Clone)]
pub struct TableRef {
    pub name: Option<String>,
    pub subquery: Option<Box<Query>>,
    pub alias: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Join {
    pub join_type: JoinType,
    pub right_table: TableRef,
    pub on_condition: Option<Expr>,
}

#[derive(Debug, Clone)]
pub enum JoinType {
    Inner,
    LeftOuter,
    RightOuter,
    FullOuter,
    Cross,
}

#[derive(Debug, Clone)]
pub struct OrderByExpr {
    pub expr: Expr,
    pub asc: bool,
}

#[derive(Debug, Clone)]
pub enum Expr {
    // Generic
    Identifier(String),
    Literal(Literal),
    Param(String),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    FuncCall {
        name: String,
        args: Vec<Expr>,
    },
    // Aggregates
    AggCall {
        name: String,
        args: Vec<Expr>,
    },
    // Table functions (SIMILAR, FOLLOW, ASSEMBLE) lowered as function calls
    // SKS-specific functions (structured for planner)
    SksSimilar {
        field: String,
        query: Box<Expr>,
        metric: Option<String>,
        threshold: Option<f64>,
    },
    SksFollow {
        start: Box<Expr>,
        edge: String,
        max_depth: u32,
    },
}

#[derive(Debug, Clone)]
pub enum Literal {
    String(String),
    Number(f64),
    Bool(bool),
    Null,
}

#[derive(Debug, Clone)]
pub enum UnaryOp {
    Not,
    Neg,
}

#[derive(Debug, Clone)]
pub enum BinaryOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Like,
    Add,
    Sub,
    Mul,
    Div,
}
