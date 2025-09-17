//! Golden tests for SQL → AST mapping. Marked ignored until parser is wired.

use std::fs;

#[test]
#[ignore]
fn golden_select_vector_order_by() {
    let sql = "SELECT id FROM products ORDER BY COSINE_DISTANCE(embedding, [0.1,0.2]) LIMIT 5";
    // TODO: Use SqlFrontendParser to parse and compare to golden JSON
    // let ast = SqlFrontendParser::new().parse(sql).unwrap();
    // let json = serde_json::to_string_pretty(&ast).unwrap();
    // let golden = fs::read_to_string("tests/golden/sql/select_vector_order_by.ast.json").unwrap();
    // assert_eq!(json, golden);
    let _ = sql;
    let _: fn<P: AsRef<std::path::Path>>(P) -> Result<String, std::io::Error> = fs::read_to_string; // silence warnings
}
