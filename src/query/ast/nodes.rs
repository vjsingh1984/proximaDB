//! Internal AST nodes for query representation.

/// Top-level query AST node
#[derive(Debug, Clone)]
pub enum Query {
    /// Simple SELECT query
    Select(Select),
    /// WITH ctes AS (...) SELECT ...
    With {
        /// Common table expressions
        ctes: Vec<Cte>,
        /// The main query body
        query: Box<Query>,
    },
    /// Set operations like UNION/INTERSECT/EXCEPT
    Set {
        /// Left-hand query
        left: Box<Query>,
        /// Set operation type
        op: SetOp,
        /// Whether to include duplicates (ALL)
        all: bool,
        /// Right-hand query
        right: Box<Query>,
    },
}

/// SELECT statement AST node
#[derive(Debug, Clone)]
pub struct Select {
    /// Columns or expressions in the SELECT list
    pub projection: Vec<ProjectionItem>,
    /// Tables in the FROM clause
    pub from: Vec<TableRef>,
    /// JOIN clauses
    pub joins: Vec<Join>,
    /// WHERE clause filter expression
    pub selection: Option<Expr>,
    /// GROUP BY expressions
    pub group_by: Vec<Expr>,
    /// HAVING clause filter expression
    pub having: Option<Expr>,
    /// ORDER BY expressions
    pub order_by: Vec<OrderByExpr>,
    /// LIMIT count
    pub limit: Option<u64>,
    /// OFFSET count
    pub offset: Option<u64>,
}

/// Single item in a SELECT projection list
#[derive(Debug, Clone)]
pub struct ProjectionItem {
    /// The expression to project
    pub expr: Expr,
    /// Optional alias (AS name)
    pub alias: Option<String>,
}

/// Common Table Expression (WITH clause entry)
#[derive(Debug, Clone)]
pub struct Cte {
    /// Name of the CTE
    pub name: String,
    /// Query that defines the CTE
    pub query: Box<Query>,
}

/// Set operation type for combining query results
#[derive(Debug, Clone)]
pub enum SetOp {
    /// Combine results from two queries
    Union,
    /// Return only rows present in both queries
    Intersect,
    /// Return rows from left query not in right query
    Except,
}

/// Reference to a table or subquery in a FROM clause
#[derive(Debug, Clone)]
pub struct TableRef {
    /// Table name, if referencing a named table
    pub name: Option<String>,
    /// Subquery, if referencing an inline query
    pub subquery: Option<Box<Query>>,
    /// Optional alias for the table reference
    pub alias: Option<String>,
}

/// JOIN clause in a query
#[derive(Debug, Clone)]
pub struct Join {
    /// Type of join (INNER, LEFT, RIGHT, FULL, CROSS)
    pub join_type: JoinType,
    /// Right-hand table in the join
    pub right_table: TableRef,
    /// ON condition for the join
    pub on_condition: Option<Expr>,
}

/// Type of SQL join
#[derive(Debug, Clone)]
pub enum JoinType {
    /// INNER JOIN
    Inner,
    /// LEFT OUTER JOIN
    LeftOuter,
    /// RIGHT OUTER JOIN
    RightOuter,
    /// FULL OUTER JOIN
    FullOuter,
    /// CROSS JOIN (cartesian product)
    Cross,
}

/// ORDER BY expression with sort direction
#[derive(Debug, Clone)]
pub struct OrderByExpr {
    /// Expression to sort by
    pub expr: Expr,
    /// True for ascending, false for descending
    pub asc: bool,
}

/// Expression AST node representing any value-producing expression
#[derive(Debug, Clone)]
pub enum Expr {
    /// Column or variable reference by name
    Identifier(String),
    /// Literal constant value
    Literal(Literal),
    /// Named parameter placeholder
    Param(String),
    /// Unary operation (NOT, negation)
    Unary {
        /// Unary operator
        op: UnaryOp,
        /// Operand expression
        expr: Box<Expr>,
    },
    /// Binary operation (comparison, arithmetic, logical)
    Binary {
        /// Left operand
        left: Box<Expr>,
        /// Binary operator
        op: BinaryOp,
        /// Right operand
        right: Box<Expr>,
    },
    /// Scalar function call
    FuncCall {
        /// Function name
        name: String,
        /// Function arguments
        args: Vec<Expr>,
    },
    /// CASE WHEN ... THEN ... ELSE ... END expression
    Case {
        /// Optional operand for simple CASE
        operand: Option<Box<Expr>>,
        /// List of (condition, result) pairs
        conditions: Vec<(Expr, Expr)>,
        /// Optional ELSE expression
        else_expr: Option<Box<Expr>>,
    },
    /// Scalar subquery expression
    Subquery(Box<Query>),
    /// Aggregate function call (SUM, COUNT, AVG, etc.)
    AggCall {
        /// Aggregate function name
        name: String,
        /// Aggregate function arguments
        args: Vec<Expr>,
    },
    /// SKS SIMILAR function for vector similarity search
    SksSimilar {
        /// Vector field to search
        field: String,
        /// Query vector expression
        query: Box<Expr>,
        /// Distance metric (e.g., "cosine", "l2")
        metric: Option<String>,
        /// Similarity threshold
        threshold: Option<f64>,
    },
    /// SKS FOLLOW function for graph traversal
    SksFollow {
        /// Starting node expression
        start: Box<Expr>,
        /// Edge type to traverse
        edge: String,
        /// Maximum traversal depth
        max_depth: u32,
    },
    /// SKS ASSEMBLE function for context assembly
    SksAssemble {
        /// Items to assemble into context
        context_items: Vec<Expr>,
        /// Assembly strategy name
        strategy: Option<String>,
        /// Maximum context size
        max_size: Option<u32>,
    },
    /// Array literal for vector expressions
    Array {
        /// Array elements
        elem: Vec<Expr>,
        /// Whether the array has named elements
        named: bool,
    },
    /// EXISTS subquery expression
    Exists {
        /// Subquery to check for existence
        subquery: Box<Query>,
        /// Whether this is NOT EXISTS
        negated: bool,
    },
    /// BETWEEN expression (expr BETWEEN low AND high)
    Between {
        /// Expression to test
        expr: Box<Expr>,
        /// Lower bound
        low: Box<Expr>,
        /// Upper bound
        high: Box<Expr>,
        /// Whether this is NOT BETWEEN
        negated: bool,
    },
    /// IS NULL / IS NOT NULL expression
    IsNull {
        /// Expression to test for null
        expr: Box<Expr>,
        /// Whether this is IS NOT NULL
        negated: bool,
    },
    /// IN list expression (expr IN (val1, val2, ...))
    InList {
        /// Expression to test
        expr: Box<Expr>,
        /// List of values to test against
        list: Vec<Expr>,
        /// Whether this is NOT IN
        negated: bool,
    },
    // Geospatial expressions
    /// GEO_DISTANCE(point1_lat, point1_lon, point2_lat, point2_lon) - returns distance in km
    GeoDistance {
        /// Latitude of the first point
        lat1: Box<Expr>,
        /// Longitude of the first point
        lon1: Box<Expr>,
        /// Latitude of the second point
        lat2: Box<Expr>,
        /// Longitude of the second point
        lon2: Box<Expr>,
    },
    /// GEO_WITHIN_DISTANCE(lat, lon, center_lat, center_lon, radius, unit) - check if within radius
    GeoWithinDistance {
        /// Latitude of the point to test
        lat: Box<Expr>,
        /// Longitude of the point to test
        lon: Box<Expr>,
        /// Latitude of the center point
        center_lat: Box<Expr>,
        /// Longitude of the center point
        center_lon: Box<Expr>,
        /// Radius distance
        radius: Box<Expr>,
        /// Distance unit (km, mi, m)
        unit: Option<String>,
    },
    /// GEO_WITHIN_BOX(lat, lon, sw_lat, sw_lon, ne_lat, ne_lon) - check if within bounding box
    GeoWithinBox {
        /// Latitude of the point to test
        lat: Box<Expr>,
        /// Longitude of the point to test
        lon: Box<Expr>,
        /// Southwest corner latitude
        sw_lat: Box<Expr>,
        /// Southwest corner longitude
        sw_lon: Box<Expr>,
        /// Northeast corner latitude
        ne_lat: Box<Expr>,
        /// Northeast corner longitude
        ne_lon: Box<Expr>,
    },
    /// GEO_POINT(lat, lon) - create a point for geo operations
    GeoPoint {
        /// Latitude value
        lat: Box<Expr>,
        /// Longitude value
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
    /// Frame defined by row count
    Rows,
    /// Frame defined by value range
    Range,
    /// Frame defined by peer groups
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

/// Literal constant value in an expression
#[derive(Debug, Clone)]
pub enum Literal {
    /// String literal
    String(String),
    /// Numeric literal (integer or float)
    Number(f64),
    /// Boolean literal
    Bool(bool),
    /// NULL literal
    Null,
}

/// Unary operator type
#[derive(Debug, Clone)]
pub enum UnaryOp {
    /// Logical NOT
    Not,
    /// Arithmetic negation
    Neg,
}

/// Binary operator type
#[derive(Debug, Clone)]
pub enum BinaryOp {
    /// Equal (=)
    Eq,
    /// Not equal (<> or !=)
    Ne,
    /// Less than (<)
    Lt,
    /// Less than or equal (<=)
    Le,
    /// Greater than (>)
    Gt,
    /// Greater than or equal (>=)
    Ge,
    /// Logical AND
    And,
    /// Logical OR
    Or,
    /// LIKE pattern match
    Like,
    /// NOT LIKE pattern match
    NotLike,
    /// IN set membership
    In,
    /// NOT IN set membership
    NotIn,
    /// Addition (+)
    Add,
    /// Subtraction (-)
    Sub,
    /// Multiplication (*)
    Mul,
    /// Division (/)
    Div,
    /// Modulo (%)
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
