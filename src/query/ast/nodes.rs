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
        unit: Option<String>, // km, mi, m
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
    /// Window function call: func(...) OVER (PARTITION BY ... ORDER BY ... frame)
    WindowCall {
        /// Function name (ROW_NUMBER, RANK, DENSE_RANK, LAG, LEAD, SUM, COUNT, etc.)
        func: String,
        /// Function arguments
        args: Vec<Expr>,
        /// PARTITION BY expressions
        partition_by: Vec<Expr>,
        /// ORDER BY expressions within the window
        order_by: Vec<OrderByExpr>,
        /// Window frame specification
        frame: Option<WindowFrame>,
    },
}

/// Window frame specification
#[derive(Debug, Clone)]
pub struct WindowFrame {
    /// Frame unit (ROWS, RANGE, GROUPS)
    pub unit: WindowFrameUnit,
    /// Frame start bound
    pub start: WindowFrameBound,
    /// Frame end bound (if BETWEEN specified)
    pub end: Option<WindowFrameBound>,
}

/// Window frame unit
#[derive(Debug, Clone)]
pub enum WindowFrameUnit {
    Rows,
    Range,
    Groups,
}

/// Window frame bound
#[derive(Debug, Clone)]
pub enum WindowFrameBound {
    /// CURRENT ROW
    CurrentRow,
    /// N PRECEDING (None means UNBOUNDED PRECEDING)
    Preceding(Option<Box<Expr>>),
    /// N FOLLOWING (None means UNBOUNDED FOLLOWING)
    Following(Option<Box<Expr>>),
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

#[cfg(test)]
mod window_tests {
    use super::*;

    #[test]
    fn test_window_call_ast_construction() {
        let expr = Expr::WindowCall {
            func: "ROW_NUMBER".to_string(),
            args: vec![],
            partition_by: vec![Expr::Identifier("category".to_string())],
            order_by: vec![OrderByExpr {
                expr: Expr::Identifier("price".to_string()),
                asc: false,
            }],
            frame: None,
        };
        match expr {
            Expr::WindowCall {
                func,
                partition_by,
                order_by,
                frame,
                ..
            } => {
                assert_eq!(func, "ROW_NUMBER");
                assert_eq!(partition_by.len(), 1);
                assert_eq!(order_by.len(), 1);
                assert!(!order_by[0].asc);
                assert!(frame.is_none());
            }
            _ => panic!("Expected WindowCall"),
        }
    }

    #[test]
    fn test_window_call_with_frame() {
        let expr = Expr::WindowCall {
            func: "SUM".to_string(),
            args: vec![Expr::Identifier("amount".to_string())],
            partition_by: vec![],
            order_by: vec![OrderByExpr {
                expr: Expr::Identifier("date".to_string()),
                asc: true,
            }],
            frame: Some(WindowFrame {
                unit: WindowFrameUnit::Rows,
                start: WindowFrameBound::Preceding(None),
                end: Some(WindowFrameBound::CurrentRow),
            }),
        };
        match expr {
            Expr::WindowCall {
                func, args, frame, ..
            } => {
                assert_eq!(func, "SUM");
                assert_eq!(args.len(), 1);
                let frame = frame.as_ref().expect("frame should be present");
                assert!(matches!(frame.unit, WindowFrameUnit::Rows));
                assert!(matches!(frame.start, WindowFrameBound::Preceding(None)));
                assert!(matches!(frame.end, Some(WindowFrameBound::CurrentRow)));
            }
            _ => panic!("Expected WindowCall"),
        }
    }

    #[test]
    fn test_window_frame_with_numeric_bound() {
        let frame = WindowFrame {
            unit: WindowFrameUnit::Range,
            start: WindowFrameBound::Preceding(Some(Box::new(Expr::Literal(Literal::Number(5.0))))),
            end: Some(WindowFrameBound::Following(Some(Box::new(Expr::Literal(
                Literal::Number(3.0),
            ))))),
        };
        assert!(matches!(frame.unit, WindowFrameUnit::Range));
        match &frame.start {
            WindowFrameBound::Preceding(Some(expr)) => {
                assert!(
                    matches!(expr.as_ref(), Expr::Literal(Literal::Number(n)) if (*n - 5.0).abs() < f64::EPSILON)
                );
            }
            _ => panic!("Expected Preceding with value"),
        }
    }

    #[test]
    fn test_window_call_aggregate_over() {
        // Test: COUNT(*) OVER (PARTITION BY dept ORDER BY hire_date)
        let expr = Expr::WindowCall {
            func: "COUNT".to_string(),
            args: vec![Expr::Identifier("*".to_string())],
            partition_by: vec![Expr::Identifier("dept".to_string())],
            order_by: vec![OrderByExpr {
                expr: Expr::Identifier("hire_date".to_string()),
                asc: true,
            }],
            frame: None,
        };
        match expr {
            Expr::WindowCall {
                func,
                args,
                partition_by,
                order_by,
                ..
            } => {
                assert_eq!(func, "COUNT");
                assert_eq!(args.len(), 1);
                assert_eq!(partition_by.len(), 1);
                assert_eq!(order_by.len(), 1);
            }
            _ => panic!("Expected WindowCall"),
        }
    }
}
