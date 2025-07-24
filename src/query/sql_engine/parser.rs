/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! SQL Query Parser for ProximaDB
//! 
//! Supports vector similarity queries with SQL-like syntax:
//! ```sql
//! SELECT id, vector, metadata 
//! FROM collection_name
//! WHERE metadata.category = 'electronics'
//! ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, ...], 'cosine')
//! LIMIT 10
//! ```

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// Parsed SQL query representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedQuery {
    /// SELECT clause fields
    pub select_fields: Vec<SelectField>,
    /// FROM clause (collection name)
    pub from_collection: String,
    /// WHERE clause conditions
    pub where_conditions: Option<WhereClause>,
    /// ORDER BY clause
    pub order_by: Option<OrderByClause>,
    /// LIMIT clause
    pub limit: Option<usize>,
    /// OFFSET clause
    pub offset: Option<usize>,
}

/// Field in SELECT clause
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SelectField {
    /// Select all fields (*)
    All,
    /// Select specific field
    Field(String),
    /// Select with alias (field AS alias)
    Aliased { field: String, alias: String },
}

/// WHERE clause representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhereClause {
    pub condition: Condition,
}

/// Condition types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Condition {
    /// Simple comparison (field op value)
    Comparison {
        field: String,
        operator: ComparisonOp,
        value: Value,
    },
    /// AND condition
    And(Box<Condition>, Box<Condition>),
    /// OR condition
    Or(Box<Condition>, Box<Condition>),
    /// NOT condition
    Not(Box<Condition>),
    /// IN condition
    In {
        field: String,
        values: Vec<Value>,
    },
    /// BETWEEN condition
    Between {
        field: String,
        low: Value,
        high: Value,
    },
}

/// Comparison operators
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ComparisonOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Like,
}

/// Value types in conditions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Value {
    String(String),
    Number(f64),
    Bool(bool),
    Null,
    Vector(Vec<f32>),
}

/// ORDER BY clause
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderByClause {
    pub order_type: OrderType,
    pub direction: SortDirection,
}

/// Order types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderType {
    /// Order by field value
    Field(String),
    /// Order by vector similarity
    VectorSimilarity {
        query_vector: Vec<f32>,
        metric: String,
    },
}

/// Sort direction
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SortDirection {
    Asc,
    Desc,
}

/// SQL Query Parser
pub struct SqlParser {
    query: String,
    position: usize,
}

impl SqlParser {
    /// Create new parser for query
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            position: 0,
        }
    }
    
    /// Parse the SQL query
    pub fn parse(&mut self) -> Result<ParsedQuery> {
        self.skip_whitespace();
        
        // Parse SELECT
        self.expect_keyword("SELECT")?;
        let select_fields = self.parse_select_fields()?;
        
        // Parse FROM
        self.expect_keyword("FROM")?;
        let from_collection = self.parse_identifier()?;
        
        // Parse optional WHERE
        let where_conditions = if self.check_keyword("WHERE") {
            self.consume_keyword("WHERE");
            Some(self.parse_where_clause()?)
        } else {
            None
        };
        
        // Parse optional ORDER BY
        let order_by = if self.check_keyword("ORDER") {
            self.consume_keyword("ORDER");
            self.expect_keyword("BY")?;
            Some(self.parse_order_by()?)
        } else {
            None
        };
        
        // Parse optional LIMIT
        let limit = if self.check_keyword("LIMIT") {
            self.consume_keyword("LIMIT");
            Some(self.parse_number()? as usize)
        } else {
            None
        };
        
        // Parse optional OFFSET
        let offset = if self.check_keyword("OFFSET") {
            self.consume_keyword("OFFSET");
            Some(self.parse_number()? as usize)
        } else {
            None
        };
        
        Ok(ParsedQuery {
            select_fields,
            from_collection,
            where_conditions,
            order_by,
            limit,
            offset,
        })
    }
    
    /// Parse SELECT fields
    fn parse_select_fields(&mut self) -> Result<Vec<SelectField>> {
        let mut fields = Vec::new();
        
        loop {
            self.skip_whitespace();
            
            if self.check_char('*') {
                self.position += 1;
                fields.push(SelectField::All);
            } else {
                let field = self.parse_identifier()?;
                
                if self.check_keyword("AS") {
                    self.consume_keyword("AS");
                    let alias = self.parse_identifier()?;
                    fields.push(SelectField::Aliased { field, alias });
                } else {
                    fields.push(SelectField::Field(field));
                }
            }
            
            if !self.consume_char(',') {
                break;
            }
        }
        
        Ok(fields)
    }
    
    /// Parse WHERE clause
    fn parse_where_clause(&mut self) -> Result<WhereClause> {
        let condition = self.parse_condition()?;
        Ok(WhereClause { condition })
    }
    
    /// Parse condition
    fn parse_condition(&mut self) -> Result<Condition> {
        // Simple implementation - just parse comparison for now
        let field = self.parse_identifier()?;
        let operator = self.parse_comparison_op()?;
        let value = self.parse_value()?;
        
        Ok(Condition::Comparison {
            field,
            operator,
            value,
        })
    }
    
    /// Parse comparison operator
    fn parse_comparison_op(&mut self) -> Result<ComparisonOp> {
        self.skip_whitespace();
        
        if self.check_chars(">=") {
            self.position += 2;
            Ok(ComparisonOp::Ge)
        } else if self.check_chars("<=") {
            self.position += 2;
            Ok(ComparisonOp::Le)
        } else if self.check_chars("!=") {
            self.position += 2;
            Ok(ComparisonOp::Ne)
        } else if self.check_chars("<>") {
            self.position += 2;
            Ok(ComparisonOp::Ne)
        } else if self.check_char('=') {
            self.position += 1;
            Ok(ComparisonOp::Eq)
        } else if self.check_char('>') {
            self.position += 1;
            Ok(ComparisonOp::Gt)
        } else if self.check_char('<') {
            self.position += 1;
            Ok(ComparisonOp::Lt)
        } else if self.check_keyword("LIKE") {
            self.consume_keyword("LIKE");
            Ok(ComparisonOp::Like)
        } else {
            Err(anyhow!("Expected comparison operator at position {}", self.position))
        }
    }
    
    /// Parse value
    fn parse_value(&mut self) -> Result<Value> {
        self.skip_whitespace();
        
        if self.check_char('\'') || self.check_char('"') {
            // String literal (single or double quotes)
            let value = self.parse_string_literal()?;
            Ok(Value::String(value))
        } else if self.check_char('[') {
            // Vector literal
            let vector = self.parse_vector_literal()?;
            Ok(Value::Vector(vector))
        } else if self.check_keyword("NULL") {
            self.consume_keyword("NULL");
            Ok(Value::Null)
        } else if self.check_keyword("TRUE") {
            self.consume_keyword("TRUE");
            Ok(Value::Bool(true))
        } else if self.check_keyword("FALSE") {
            self.consume_keyword("FALSE");
            Ok(Value::Bool(false))
        } else {
            // Try to parse number
            let num = self.parse_number()?;
            Ok(Value::Number(num))
        }
    }
    
    /// Parse ORDER BY clause
    fn parse_order_by(&mut self) -> Result<OrderByClause> {
        self.skip_whitespace();
        
        let order_type = if self.check_keyword("VECTOR_SIMILARITY") {
            self.consume_keyword("VECTOR_SIMILARITY");
            self.expect_char('(')?;
            let _field = self.parse_identifier()?; // Usually "vector"
            self.expect_char(',')?;
            let query_vector = self.parse_vector_literal()?;
            self.expect_char(',')?;
            let metric = self.parse_string_literal()?;
            self.expect_char(')')?;
            
            OrderType::VectorSimilarity {
                query_vector,
                metric,
            }
        } else {
            let field = self.parse_identifier()?;
            OrderType::Field(field)
        };
        
        let direction = if self.check_keyword("DESC") {
            self.consume_keyword("DESC");
            SortDirection::Desc
        } else {
            if self.check_keyword("ASC") {
                self.consume_keyword("ASC");
            }
            SortDirection::Asc
        };
        
        Ok(OrderByClause {
            order_type,
            direction,
        })
    }
    
    // Helper methods
    
    fn skip_whitespace(&mut self) {
        while self.position < self.query.len() {
            if self.query.chars().nth(self.position).unwrap().is_whitespace() {
                self.position += 1;
            } else {
                break;
            }
        }
    }
    
    fn check_keyword(&mut self, keyword: &str) -> bool {
        self.skip_whitespace();
        let remaining = &self.query[self.position..];
        let upper_remaining = remaining.to_uppercase();
        let upper_keyword = keyword.to_uppercase();
        
        if upper_remaining.starts_with(&upper_keyword) {
            // Check that keyword is followed by non-alphanumeric
            let after_keyword = self.position + keyword.len();
            if after_keyword >= self.query.len() || 
               !self.query.chars().nth(after_keyword).unwrap().is_alphanumeric() {
                // Don't consume - just check
                return true;
            }
        }
        false
    }
    
    fn consume_keyword(&mut self, keyword: &str) {
        if self.check_keyword(keyword) {
            self.position += keyword.len();
        }
    }
    
    fn expect_keyword(&mut self, keyword: &str) -> Result<()> {
        if !self.check_keyword(keyword) {
            return Err(anyhow!("Expected {} at position {}", keyword, self.position));
        }
        self.position += keyword.len();
        Ok(())
    }
    
    fn check_char(&mut self, ch: char) -> bool {
        self.skip_whitespace();
        if self.position < self.query.len() && 
           self.query.chars().nth(self.position) == Some(ch) {
            // Don't consume the character - let the parser decide
            true
        } else {
            false
        }
    }
    
    fn consume_char(&mut self, ch: char) -> bool {
        if self.check_char(ch) {
            self.position += 1;
            true
        } else {
            false
        }
    }
    
    fn check_chars(&mut self, chars: &str) -> bool {
        self.skip_whitespace();
        let remaining = &self.query[self.position..];
        
        if remaining.starts_with(chars) {
            self.position += chars.len();
            true
        } else {
            false
        }
    }
    
    fn expect_char(&mut self, ch: char) -> Result<()> {
        if !self.consume_char(ch) {
            return Err(anyhow!("Expected '{}' at position {}", ch, self.position));
        }
        Ok(())
    }
    
    fn parse_identifier(&mut self) -> Result<String> {
        self.skip_whitespace();
        let start = self.position;
        
        // Handle quoted identifiers
        if self.check_char('"') {
            self.position += 1;
            let mut ident = String::new();
            while self.position < self.query.len() {
                let ch = self.query.chars().nth(self.position).unwrap();
                self.position += 1;
                
                if ch == '"' {
                    if self.position < self.query.len() && 
                       self.query.chars().nth(self.position) == Some('"') {
                        // Escaped quote
                        ident.push('"');
                        self.position += 1;
                    } else {
                        // End of identifier
                        return Ok(ident);
                    }
                } else {
                    ident.push(ch);
                }
            }
            return Err(anyhow!("Unterminated quoted identifier"));
        }
        
        // Regular identifier
        while self.position < self.query.len() {
            let ch = self.query.chars().nth(self.position).unwrap();
            if ch.is_alphanumeric() || ch == '_' || ch == '.' {
                self.position += 1;
            } else {
                break;
            }
        }
        
        if start == self.position {
            return Err(anyhow!("Expected identifier at position {}", self.position));
        }
        
        Ok(self.query[start..self.position].to_string())
    }
    
    fn parse_string_literal(&mut self) -> Result<String> {
        // Support both single and double quotes
        let quote_char = if self.check_char('\'') {
            self.position += 1;
            '\''
        } else if self.check_char('"') {
            self.position += 1;
            '"'
        } else {
            return Err(anyhow!("Expected string literal at position {}", self.position));
        };
        
        let mut value = String::new();
        
        while self.position < self.query.len() {
            let ch = self.query.chars().nth(self.position).unwrap();
            self.position += 1;
            
            if ch == quote_char {
                if self.position < self.query.len() && 
                   self.query.chars().nth(self.position) == Some(quote_char) {
                    // Escaped quote
                    value.push(quote_char);
                    self.position += 1;
                } else {
                    // End of string
                    return Ok(value);
                }
            } else {
                value.push(ch);
            }
        }
        
        Err(anyhow!("Unterminated string literal"))
    }
    
    fn parse_vector_literal(&mut self) -> Result<Vec<f32>> {
        self.expect_char('[')?;
        let mut values = Vec::new();
        
        loop {
            self.skip_whitespace();
            
            if self.check_char(']') {
                self.position += 1;
                break;
            }
            
            let num = self.parse_number()? as f32;
            values.push(num);
            
            if !self.consume_char(',') {
                self.expect_char(']')?;
                break;
            }
        }
        
        Ok(values)
    }
    
    fn parse_number(&mut self) -> Result<f64> {
        self.skip_whitespace();
        let start = self.position;
        let mut has_dot = false;
        
        // Optional sign
        if self.position < self.query.len() {
            let ch = self.query.chars().nth(self.position).unwrap();
            if ch == '+' || ch == '-' {
                self.position += 1;
            }
        }
        
        // Digits
        while self.position < self.query.len() {
            let ch = self.query.chars().nth(self.position).unwrap();
            
            if ch.is_ascii_digit() {
                self.position += 1;
            } else if ch == '.' && !has_dot {
                has_dot = true;
                self.position += 1;
            } else {
                break;
            }
        }
        
        if start == self.position {
            return Err(anyhow!("Expected number at position {}", self.position));
        }
        
        self.query[start..self.position]
            .parse()
            .map_err(|e| anyhow!("Invalid number: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_simple_select() {
        let query = "SELECT * FROM products LIMIT 10";
        let mut parser = SqlParser::new(query);
        let parsed = parser.parse().unwrap();
        
        assert_eq!(parsed.from_collection, "products");
        assert_eq!(parsed.limit, Some(10));
        assert!(matches!(parsed.select_fields[0], SelectField::All));
    }
    
    #[test]
    fn test_parse_vector_search() {
        let query = "
            SELECT id, metadata 
            FROM products 
            WHERE metadata.category = 'electronics'
            ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3], 'cosine')
            LIMIT 5
        ";
        
        let mut parser = SqlParser::new(query);
        let parsed = parser.parse().unwrap();
        
        assert_eq!(parsed.from_collection, "products");
        assert_eq!(parsed.limit, Some(5));
        assert_eq!(parsed.select_fields.len(), 2);
        
        match &parsed.order_by {
            Some(OrderByClause { order_type: OrderType::VectorSimilarity { query_vector, metric }, .. }) => {
                assert_eq!(query_vector, &vec![0.1, 0.2, 0.3]);
                assert_eq!(metric, "cosine");
            }
            _ => panic!("Expected vector similarity ordering"),
        }
    }
}
