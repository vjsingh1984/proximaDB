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
    // CASE expression
    Case {
        operand: Option<Box<Expr>>,
        conditions: Vec<(Expr, Expr)>,
        else_expr: Option<Box<Expr>>,
    },
    // Subquery expression
    Subquery(Box<Query>),
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
    SksAssemble {
        context_items: Vec<Expr>,
        strategy: Option<String>,
        max_size: Option<u32>,
    },
    // Array literal for vector expressions
    Array {
        elem: Vec<Expr>,
        named: bool,
    },
    // EXISTS subquery expression
    Exists {
        subquery: Box<Query>,
        negated: bool,
    },
    // BETWEEN expression (expr BETWEEN low AND high)
    Between {
        expr: Box<Expr>,
        low: Box<Expr>,
        high: Box<Expr>,
        negated: bool,
    },
    // IS NULL / IS NOT NULL expression
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    // IN list expression (expr IN (val1, val2, ...))
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    // Geospatial expressions
    /// GEO_DISTANCE(point1_lat, point1_lon, point2_lat, point2_lon) - returns distance in km
    GeoDistance {
        lat1: Box<Expr>,
        lon1: Box<Expr>,
        lat2: Box<Expr>,
        lon2: Box<Expr>,
    },
    /// GEO_WITHIN_DISTANCE(lat, lon, center_lat, center_lon, radius, unit) - check if within radius
    GeoWithinDistance {
        lat: Box<Expr>,
        lon: Box<Expr>,
        center_lat: Box<Expr>,
        center_lon: Box<Expr>,
        radius: Box<Expr>,
        unit: Option<String>,  // km, mi, m
    },
    /// GEO_WITHIN_BOX(lat, lon, sw_lat, sw_lon, ne_lat, ne_lon) - check if within bounding box
    GeoWithinBox {
        lat: Box<Expr>,
        lon: Box<Expr>,
        sw_lat: Box<Expr>,
        sw_lon: Box<Expr>,
        ne_lat: Box<Expr>,
        ne_lon: Box<Expr>,
    },
    /// GEO_POINT(lat, lon) - create a point for geo operations
    GeoPoint {
        lat: Box<Expr>,
        lon: Box<Expr>,
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
    NotLike,
    In,
    NotIn,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}
