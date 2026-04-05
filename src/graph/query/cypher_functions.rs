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

//! # Cypher Function Library
//!
//! Provides built-in functions for Cypher queries.
//! Supports string, math, date, and aggregation functions.
//!
//! ## Function Categories
//!
//! ### String Functions
//! - `toUpper()` - Convert to uppercase
//! - `toLower()` - Convert to lowercase
//! - `trim()` - Remove leading/trailing whitespace
//! - `substring()` - Extract substring
//! - `replace()` - Replace occurrences
//! - `size()` - String length
//! - `concat()` - String concatenation
//!
//! ### Math Functions
//! - `abs()` - Absolute value
//! - `ceil()` - Round up
//! - `floor()` - Round down
//! - `round()` - Round to nearest
//! - `sqrt()` - Square root
//! - `pow()` - Power function
//! - `log()` - Natural logarithm
//! - `log10()` - Base-10 logarithm
//! - `exp()` - Exponential
//! - `sin()`, `cos()`, `tan()` - Trigonometric functions
//! - `asin()`, `acos()`, `atan()` - Inverse trigonometric functions
//!
//! ### Date Functions
//! - `date()` - Current date/time
//! - `timestamp()` - Unix timestamp
//! - `datetime()` - Create datetime from components
//! - `duration()` - Time difference
//!
//! ## Usage
//!
//! ```cypher
//! // String functions
//! MATCH (p:Person) RETURN toUpper(p.name), size(p.name)
//!
//! // Math functions
//! MATCH (p:Product) RETURN round(p.price * 1.1), abs(p.balance)
//!
//! // Date functions
//! MATCH (o:Order) RETURN o.created_at, duration(o.created_at, timestamp())
//! ```

use std::collections::HashMap;
use anyhow::{Result, bail, anyhow};

use super::cypher_ast::{Expression, CypherValue};

/// Context for function evaluation
pub struct FunctionContext {
    /// Variable bindings (e.g., from MATCH clause)
    pub variables: HashMap<String, CypherValue>,
}

impl FunctionContext {
    /// Create a new function context
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
        }
    }

    /// Set a variable value
    pub fn set_variable(&mut self, name: String, value: CypherValue) {
        self.variables.insert(name, value);
    }

    /// Get a variable value
    pub fn get_variable(&self, name: &str) -> Option<&CypherValue> {
        self.variables.get(name)
    }
}

impl Default for FunctionContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Cypher function definition with metadata
#[derive(Debug, Clone)]
pub struct CypherFunction {
    /// Function name (case-insensitive)
    pub name: String,
    /// Number of required arguments
    pub min_args: usize,
    /// Number of optional arguments (0 if all required)
    pub max_args: usize,
    /// Whether function is variadic (accepts unlimited arguments)
    pub variadic: bool,
    /// Function implementation
    pub handler: fn(&[Expression], &FunctionContext) -> Result<CypherValue>,
}

impl CypherFunction {
    /// Create a new Cypher function
    pub fn new(
        name: &str,
        min_args: usize,
        max_args: usize,
        variadic: bool,
        handler: fn(&[Expression], &FunctionContext) -> Result<CypherValue>,
    ) -> Self {
        Self {
            name: name.to_uppercase(),
            min_args,
            max_args,
            variadic,
            handler,
        }
    }
}

/// Registry of built-in Cypher functions
pub struct CypherFunctionRegistry {
    functions: HashMap<String, CypherFunction>,
}

impl CypherFunctionRegistry {
    /// Create a new function registry
    pub fn new() -> Self {
        let mut registry = Self {
            functions: HashMap::new(),
        };

        // Register all built-in functions
        registry.register_string_functions();
        registry.register_math_functions();
        registry.register_date_functions();
        registry.register_aggregation_functions();

        registry
    }

    /// Register a function in the registry
    pub fn register(&mut self, function: CypherFunction) {
        self.functions.insert(function.name.clone(), function);
    }

    /// Get a function by name (case-insensitive)
    pub fn get(&self, name: &str) -> Option<&CypherFunction> {
        self.functions.get(&name.to_uppercase())
    }

    /// List all registered function names
    pub fn list_functions(&self) -> Vec<String> {
        self.functions.keys().cloned().collect()
    }

    /// Register all string functions
    fn register_string_functions(&mut self) {
        // toUpper(string) -> string
        self.register(CypherFunction::new("toUpper", 1, 1, false, cypher_to_upper));
        self.register(CypherFunction::new("toLower", 1, 1, false, cypher_to_lower));
        self.register(CypherFunction::new("trim", 1, 1, false, cypher_trim));
        self.register(CypherFunction::new("ltrim", 1, 1, false, cypher_ltrim));
        self.register(CypherFunction::new("rtrim", 1, 1, false, cypher_rtrim));
        self.register(CypherFunction::new("substring", 3, 3, false, cypher_substring));
        self.register(CypherFunction::new("replace", 3, 3, false, cypher_replace));
        self.register(CypherFunction::new("size", 1, 1, false, cypher_size));
        self.register(CypherFunction::new("length", 1, 1, false, cypher_size)); // Alias for size
        self.register(CypherFunction::new("concat", 2, 99, true, cypher_concat));
        self.register(CypherFunction::new("left", 2, 2, false, cypher_left));
        self.register(CypherFunction::new("right", 2, 2, false, cypher_right));
        self.register(CypherFunction::new("reverse", 1, 1, false, cypher_reverse));
        self.register(CypherFunction::new("toString", 1, 1, false, cypher_to_string));
    }

    /// Register all math functions
    fn register_math_functions(&mut self) {
        self.register(CypherFunction::new("abs", 1, 1, false, cypher_abs));
        self.register(CypherFunction::new("ceil", 1, 1, false, cypher_ceil));
        self.register(CypherFunction::new("floor", 1, 1, false, cypher_floor));
        self.register(CypherFunction::new("round", 1, 2, false, cypher_round));
        self.register(CypherFunction::new("sign", 1, 1, false, cypher_sign));
        self.register(CypherFunction::new("sqrt", 1, 1, false, cypher_sqrt));
        self.register(CypherFunction::new("cbrt", 1, 1, false, cypher_cbrt));
        self.register(CypherFunction::new("pow", 2, 2, false, cypher_pow));
        self.register(CypherFunction::new("exp", 1, 1, false, cypher_exp));
        self.register(CypherFunction::new("log", 1, 1, false, cypher_log));
        self.register(CypherFunction::new("log10", 1, 1, false, cypher_log10));
        self.register(CypherFunction::new("sin", 1, 1, false, cypher_sin));
        self.register(CypherFunction::new("cos", 1, 1, false, cypher_cos));
        self.register(CypherFunction::new("tan", 1, 1, false, cypher_tan));
        self.register(CypherFunction::new("asin", 1, 1, false, cypher_asin));
        self.register(CypherFunction::new("acos", 1, 1, false, cypher_acos));
        self.register(CypherFunction::new("atan", 1, 1, false, cypher_atan));
        self.register(CypherFunction::new("atan2", 2, 2, false, cypher_atan2));
        self.register(CypherFunction::new("degrees", 1, 1, false, cypher_degrees));
        self.register(CypherFunction::new("radians", 1, 1, false, cypher_radians));
        self.register(CypherFunction::new("pi", 0, 0, false, cypher_pi));
        self.register(CypherFunction::new("rand", 0, 0, false, cypher_rand));
    }

    /// Register all date/time functions
    fn register_date_functions(&mut self) {
        self.register(CypherFunction::new("date", 0, 1, false, cypher_date));
        self.register(CypherFunction::new("datetime", 1, 3, false, cypher_datetime));
        self.register(CypherFunction::new("timestamp", 0, 0, false, cypher_timestamp));
        self.register(CypherFunction::new("duration", 2, 2, false, cypher_duration));
        self.register(CypherFunction::new("toString", 1, 1, false, cypher_to_string));
    }

    /// Register aggregation functions
    fn register_aggregation_functions(&mut self) {
        // These are handled specially in RETURN clause, but we register them here
        self.register(CypherFunction::new("count", 0, 1, true, cypher_count));
        self.register(CypherFunction::new("sum", 1, 1, true, cypher_sum));
        self.register(CypherFunction::new("avg", 1, 1, true, cypher_avg));
        self.register(CypherFunction::new("min", 1, 1, true, cypher_min));
        self.register(CypherFunction::new("max", 1, 1, true, cypher_max));
        self.register(CypherFunction::new("collect", 1, 1, true, cypher_collect));
    }
}

impl Default for CypherFunctionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// STRING FUNCTIONS
// =============================================================================

fn cypher_to_upper(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::String(String::new()));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::String(s) => Ok(CypherValue::String(s.to_uppercase())),
        _ => bail!("toUpper requires a string argument"),
    }
}

fn cypher_to_lower(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::String(String::new()));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::String(s) => Ok(CypherValue::String(s.to_lowercase())),
        _ => bail!("toLower requires a string argument"),
    }
}

fn cypher_trim(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::String(String::new()));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::String(s) => Ok(CypherValue::String(s.trim().to_string())),
        _ => bail!("trim requires a string argument"),
    }
}

fn cypher_ltrim(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::String(String::new()));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::String(s) => Ok(CypherValue::String(s.trim_start().to_string())),
        _ => bail!("ltrim requires a string argument"),
    }
}

fn cypher_rtrim(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::String(String::new()));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::String(s) => Ok(CypherValue::String(s.trim_end().to_string())),
        _ => bail!("rtrim requires a string argument"),
    }
}

fn cypher_substring(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.len() < 3 {
        bail!("substring requires 3 arguments: string, start, length");
    }

    let string = match evaluate_expression(&args[0], _ctx)? {
        CypherValue::String(s) => s,
        _ => bail!("substring first argument must be a string"),
    };

    let start = match evaluate_expression(&args[1], _ctx)? {
        CypherValue::Integer(i) => i as usize,
        _ => bail!("substring start must be an integer"),
    };

    let length = match evaluate_expression(&args[2], _ctx)? {
        CypherValue::Integer(i) => i as usize,
        _ => bail!("substring length must be an integer"),
    };

    if start >= string.len() {
        return Ok(CypherValue::String(String::new()));
    }

    let end = (start + length).min(string.len());
    Ok(CypherValue::String(string[start..end].to_string()))
}

fn cypher_replace(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.len() < 3 {
        bail!("replace requires 3 arguments: string, search, replacement");
    }

    let string = match evaluate_expression(&args[0], _ctx)? {
        CypherValue::String(s) => s,
        _ => bail!("replace first argument must be a string"),
    };

    let search = match evaluate_expression(&args[1], _ctx)? {
        CypherValue::String(s) => s,
        _ => bail!("replace second argument must be a string"),
    };

    let replacement = match evaluate_expression(&args[2], _ctx)? {
        CypherValue::String(s) => s,
        _ => bail!("replace third argument must be a string"),
    };

    Ok(CypherValue::String(string.replace(&search, replacement.as_str())))
}

fn cypher_size(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Integer(0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::String(s) => Ok(CypherValue::Integer(s.len() as i64)),
        _ => bail!("size requires a string argument"),
    }
}

fn cypher_concat(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::String(String::new()));
    }

    let mut result = String::new();
    for arg in args {
        match evaluate_expression(arg, _ctx)? {
            CypherValue::String(s) => result.push_str(&s),
            CypherValue::Integer(i) => result.push_str(&i.to_string()),
            CypherValue::Float(f) => result.push_str(&f.to_string()),
            _ => result.push_str("[complex]"),
        }
    }
    Ok(CypherValue::String(result))
}

fn cypher_left(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.len() < 2 {
        bail!("left requires 2 arguments: string, length");
    }

    let string = match evaluate_expression(&args[0], _ctx)? {
        CypherValue::String(s) => s,
        _ => bail!("left first argument must be a string"),
    };

    let length = match evaluate_expression(&args[1], _ctx)? {
        CypherValue::Integer(i) => i as usize,
        _ => bail!("left second argument must be an integer"),
    };

    if length >= string.len() {
        return Ok(CypherValue::String(string.clone()));
    }

    Ok(CypherValue::String(string[..length].to_string()))
}

fn cypher_right(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.len() < 2 {
        bail!("right requires 2 arguments: string, length");
    }

    let string = match evaluate_expression(&args[0], _ctx)? {
        CypherValue::String(s) => s,
        _ => bail!("right first argument must be a string"),
    };

    let length = match evaluate_expression(&args[1], _ctx)? {
        CypherValue::Integer(i) => i as usize,
        _ => bail!("right second argument must be an integer"),
    };

    if length >= string.len() {
        return Ok(CypherValue::String(string.clone()));
    }

    let start = string.len().saturating_sub(length);
    Ok(CypherValue::String(string[start..].to_string()))
}

fn cypher_reverse(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::String(String::new()));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::String(s) => Ok(CypherValue::String(s.chars().rev().collect())),
        _ => bail!("reverse requires a string argument"),
    }
}

fn cypher_to_string(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::String(String::new()));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::String(s) => Ok(CypherValue::String(s)),
        CypherValue::Integer(i) => Ok(CypherValue::String(i.to_string())),
        CypherValue::Float(f) => Ok(CypherValue::String(f.to_string())),
        CypherValue::Boolean(b) => Ok(CypherValue::String(b.to_string())),
        CypherValue::Null => Ok(CypherValue::String("null".to_string())),
        _ => Ok(CypherValue::String("[complex]".to_string())),
    }
}

// =============================================================================
// MATH FUNCTIONS
// =============================================================================

fn cypher_abs(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(0.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => Ok(CypherValue::Integer(i.abs())),
        CypherValue::Float(f) => Ok(CypherValue::Float(f.abs())),
        _ => bail!("abs requires a numeric argument"),
    }
}

fn cypher_ceil(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(0.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Float(f) => Ok(CypherValue::Float(f.ceil())),
        CypherValue::Integer(i) => Ok(CypherValue::Float(i as f64)),
        _ => bail!("ceil requires a numeric argument"),
    }
}

fn cypher_floor(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(0.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Float(f) => Ok(CypherValue::Float(f.floor())),
        CypherValue::Integer(i) => Ok(CypherValue::Float(i as f64)),
        _ => bail!("floor requires a numeric argument"),
    }
}

fn cypher_round(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Integer(0));
    }

    let value = match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => i as f64,
        CypherValue::Float(f) => f,
        _ => bail!("round requires a numeric argument"),
    };

    let precision = if args.len() > 1 {
        match evaluate_expression(&args[1], _ctx)? {
            CypherValue::Integer(i) => i as u32,
            _ => 0,
        }
    } else {
        0
    };

    let multiplier = 10_f64.powi(precision as i32);
    Ok(CypherValue::Float((value * multiplier).round() / multiplier))
}

fn cypher_sign(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Integer(0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => Ok(CypherValue::Integer(i.signum())),
        CypherValue::Float(f) => Ok(CypherValue::Integer(f.signum() as i64)),
        _ => bail!("sign requires a numeric argument"),
    }
}

fn cypher_sqrt(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(0.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => Ok(CypherValue::Float((i as f64).sqrt())),
        CypherValue::Float(f) => {
            if f < 0.0 {
                bail!("sqrt of negative number")
            }
            Ok(CypherValue::Float(f.sqrt()))
        },
        _ => bail!("sqrt requires a numeric argument"),
    }
}

fn cypher_cbrt(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(0.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => Ok(CypherValue::Float((i as f64).cbrt())),
        CypherValue::Float(f) => Ok(CypherValue::Float(f.cbrt())),
        _ => bail!("cbrt requires a numeric argument"),
    }
}

fn cypher_pow(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.len() < 2 {
        bail!("pow requires 2 arguments: base, exponent");
    }

    let base = match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => i as f64,
        CypherValue::Float(f) => f,
        _ => bail!("pow first argument must be numeric"),
    };

    let exp = match evaluate_expression(&args[1], _ctx)? {
        CypherValue::Integer(i) => i as f64,
        CypherValue::Float(f) => f,
        _ => bail!("pow second argument must be numeric"),
    };

    Ok(CypherValue::Float(base.powf(exp)))
}

fn cypher_exp(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(1.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => Ok(CypherValue::Float((i as f64).exp())),
        CypherValue::Float(f) => Ok(CypherValue::Float(f.exp())),
        _ => bail!("exp requires a numeric argument"),
    }
}

fn cypher_log(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(0.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => {
            if i <= 0 {
                bail!("log of non-positive number")
            }
            Ok(CypherValue::Float((i as f64).ln()))
        }
        CypherValue::Float(f) => {
            if f <= 0.0 {
                bail!("log of non-positive number")
            }
            Ok(CypherValue::Float(f.ln()))
        }
        _ => bail!("log requires a numeric argument"),
    }
}

fn cypher_log10(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(0.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => {
            if i <= 0 {
                bail!("log10 of non-positive number")
            }
            Ok(CypherValue::Float((i as f64).log10()))
        }
        CypherValue::Float(f) => {
            if f <= 0.0 {
                bail!("log10 of non-positive number")
            }
            Ok(CypherValue::Float(f.log10()))
        }
        _ => bail!("log10 requires a numeric argument"),
    }
}

fn cypher_sin(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(0.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => Ok(CypherValue::Float((i as f64).sin())),
        CypherValue::Float(f) => Ok(CypherValue::Float(f.sin())),
        _ => bail!("sin requires a numeric argument"),
    }
}

fn cypher_cos(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(1.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => Ok(CypherValue::Float((i as f64).cos())),
        CypherValue::Float(f) => Ok(CypherValue::Float(f.cos())),
        _ => bail!("cos requires a numeric argument"),
    }
}

fn cypher_tan(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(0.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => Ok(CypherValue::Float((i as f64).tan())),
        CypherValue::Float(f) => Ok(CypherValue::Float(f.tan())),
        _ => bail!("tan requires a numeric argument"),
    }
}

fn cypher_asin(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(0.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => {
            if (i as f64).abs() > 1.0 {
                bail!("asin argument out of range [-1, 1]")
            }
            Ok(CypherValue::Float((i as f64).asin()))
        }
        CypherValue::Float(f) => {
            if f.abs() > 1.0 {
                bail!("asin argument out of range [-1, 1]")
            }
            Ok(CypherValue::Float(f.asin()))
        }
        _ => bail!("asin requires a numeric argument"),
    }
}

fn cypher_acos(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(std::f64::consts::FRAC_PI_2));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => {
            if (i as f64).abs() > 1.0 {
                bail!("acos argument out of range [-1, 1]")
            }
            Ok(CypherValue::Float((i as f64).acos()))
        }
        CypherValue::Float(f) => {
            if f.abs() > 1.0 {
                bail!("acos argument out of range [-1, 1]")
            }
            Ok(CypherValue::Float(f.acos()))
        }
        _ => bail!("acos requires a numeric argument"),
    }
}

fn cypher_atan(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(0.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => Ok(CypherValue::Float((i as f64).atan())),
        CypherValue::Float(f) => Ok(CypherValue::Float(f.atan())),
        _ => bail!("atan requires a numeric argument"),
    }
}

fn cypher_atan2(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.len() < 2 {
        bail!("atan2 requires 2 arguments: y, x");
    }

    let y = match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => i as f64,
        CypherValue::Float(f) => f,
        _ => bail!("atan2 first argument must be numeric"),
    };

    let x = match evaluate_expression(&args[1], _ctx)? {
        CypherValue::Integer(i) => i as f64,
        CypherValue::Float(f) => f,
        _ => bail!("atan2 second argument must be numeric"),
    };

    Ok(CypherValue::Float(y.atan2(x)))
}

fn cypher_degrees(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(0.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => Ok(CypherValue::Float((i as f64).to_degrees())),
        CypherValue::Float(f) => Ok(CypherValue::Float(f.to_degrees())),
        _ => bail!("degrees requires a numeric argument"),
    }
}

fn cypher_radians(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(0.0));
    }

    match evaluate_expression(&args[0], _ctx)? {
        CypherValue::Integer(i) => Ok(CypherValue::Float((i as f64).to_radians())),
        CypherValue::Float(f) => Ok(CypherValue::Float(f.to_radians())),
        _ => bail!("radians requires a numeric argument"),
    }
}

fn cypher_pi(_args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    Ok(CypherValue::Float(std::f64::consts::PI))
}

fn cypher_rand(_args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow!("Failed to get time: {}", e))?
        .as_nanos();

    Ok(CypherValue::Float((nanos % 1000) as f64 / 1000.0))
}

// =============================================================================
// DATE/TIME FUNCTIONS
// =============================================================================

fn cypher_date(_args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    // Return current date as ISO string
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow!("Failed to get time: {}", e))?
        .as_secs();

    // Format as ISO date string (simplified)
    let days_since_epoch = now / 86400;
    let date = chrono::DateTime::from_timestamp(days_since_epoch as i64, 0)
        .ok_or_else(|| anyhow!("Failed to create date"))?
        .format("%Y-%m-%d");

    Ok(CypherValue::String(date.to_string()))
}

fn cypher_datetime(args: &[Expression], ctx: &FunctionContext) -> Result<CypherValue> {
    // datetime(year, month, day, hour, minute, second)
    if args.len() < 3 {
        bail!("datetime requires at least 3 arguments: year, month, day");
    }

    let year = match evaluate_expression(&args[0], ctx)? {
        CypherValue::Integer(i) => i as i32,
        _ => bail!("datetime year must be an integer"),
    };

    let month = match evaluate_expression(&args[1], ctx)? {
        CypherValue::Integer(i) => i as u32,
        _ => bail!("datetime month must be an integer"),
    };

    let day = match evaluate_expression(&args[2], ctx)? {
        CypherValue::Integer(i) => i as u32,
        _ => bail!("datetime day must be an integer"),
    };

    let hour = if args.len() > 3 {
        match evaluate_expression(&args[3], ctx)? {
            CypherValue::Integer(i) => Some(i as u32),
            _ => None,
        }
    } else {
        None
    };

    let minute = if args.len() > 4 {
        match evaluate_expression(&args[4], ctx)? {
            CypherValue::Integer(i) => Some(i as u32),
            _ => None,
        }
    } else {
        None
    };

    let second = if args.len() > 5 {
        match evaluate_expression(&args[5], ctx)? {
            CypherValue::Integer(i) => Some(i as u32),
            _ => None,
        }
    } else {
        None
    };

    let datetime = if let (Some(h), Some(m), Some(s)) = (hour, minute, second) {
        chrono::NaiveDate::from_ymd_opt(year, month, day)
            .ok_or_else(|| anyhow!("Invalid date"))?
            .and_hms_opt(h, m, s)
            .ok_or_else(|| anyhow!("Invalid time"))?
    } else {
        chrono::NaiveDate::from_ymd_opt(year, month, day)
            .ok_or_else(|| anyhow!("Invalid date"))?
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow!("Invalid time"))?
    };

    Ok(CypherValue::String(datetime.format("%Y-%m-%dT%H:%M:%S").to_string()))
}

fn cypher_timestamp(_args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow!("Failed to get time: {}", e))?
        .as_secs();

    Ok(CypherValue::Integer(timestamp as i64))
}

fn cypher_duration(args: &[Expression], ctx: &FunctionContext) -> Result<CypherValue> {
    if args.len() < 2 {
        bail!("duration requires 2 arguments: start, end");
    }

    let start = evaluate_expression(&args[0], ctx)?;
    let end = evaluate_expression(&args[1], ctx)?;

    match (start, end) {
        (CypherValue::Integer(s), CypherValue::Integer(e)) => {
            Ok(CypherValue::Integer((e - s).max(0)))
        }
        (CypherValue::Float(s), CypherValue::Float(e)) => {
            Ok(CypherValue::Float((e - s).max(0.0)))
        }
        _ => bail!("duration requires timestamp arguments"),
    }
}

// =============================================================================
// AGGREGATION FUNCTIONS
// =============================================================================

fn cypher_count(args: &[Expression], _ctx: &FunctionContext) -> Result<CypherValue> {
    Ok(CypherValue::Integer(args.len() as i64))
}

fn cypher_sum(args: &[Expression], ctx: &FunctionContext) -> Result<CypherValue> {
    let mut sum = 0.0;
    for arg in args {
        match evaluate_expression(arg, ctx)? {
            CypherValue::Integer(i) => sum += i as f64,
            CypherValue::Float(f) => sum += f,
            _ => bail!("sum requires numeric arguments"),
        }
    }
    Ok(CypherValue::Float(sum))
}

fn cypher_avg(args: &[Expression], ctx: &FunctionContext) -> Result<CypherValue> {
    if args.is_empty() {
        return Ok(CypherValue::Float(0.0));
    }

    let mut sum = 0.0;
    let mut count = 0;
    for arg in args {
        match evaluate_expression(arg, ctx)? {
            CypherValue::Integer(i) => {
                sum += i as f64;
                count += 1;
            }
            CypherValue::Float(f) => {
                sum += f;
                count += 1;
            }
            _ => bail!("avg requires numeric arguments"),
        }
    }

    if count > 0 {
        Ok(CypherValue::Float(sum / count as f64))
    } else {
        Ok(CypherValue::Float(0.0))
    }
}

fn cypher_min(args: &[Expression], ctx: &FunctionContext) -> Result<CypherValue> {
    let mut min_int = None;
    let mut min_float = None;
    for arg in args {
        match evaluate_expression(arg, ctx)? {
            CypherValue::Integer(i) => {
                min_int = Some(min_int.map_or(i, |m: i64| m.min(i)));
            }
            CypherValue::Float(f) => {
                min_float = Some(min_float.map_or(f, |m: f64| m.min(f)));
            }
            _ => bail!("min requires numeric arguments"),
        }
    }

    if let Some(val) = min_int {
        Ok(CypherValue::Integer(val))
    } else if let Some(val) = min_float {
        Ok(CypherValue::Float(val))
    } else {
        Ok(CypherValue::Integer(0))
    }
}

fn cypher_max(args: &[Expression], ctx: &FunctionContext) -> Result<CypherValue> {
    let mut max_int = None;
    let mut max_float = None;
    for arg in args {
        match evaluate_expression(arg, ctx)? {
            CypherValue::Integer(i) => {
                max_int = Some(max_int.map_or(i, |m: i64| m.max(i)));
            }
            CypherValue::Float(f) => {
                max_float = Some(max_float.map_or(f, |m: f64| m.max(f)));
            }
            _ => bail!("max requires numeric arguments"),
        }
    }

    if let Some(val) = max_int {
        Ok(CypherValue::Integer(val))
    } else if let Some(val) = max_float {
        Ok(CypherValue::Float(val))
    } else {
        Ok(CypherValue::Integer(0))
    }
}

fn cypher_collect(args: &[Expression], ctx: &FunctionContext) -> Result<CypherValue> {
    let mut collected = Vec::new();
    for arg in args {
        match evaluate_expression(arg, ctx)? {
            CypherValue::String(s) => collected.push(CypherValue::String(s)),
            CypherValue::Integer(i) => collected.push(CypherValue::Integer(i)),
            CypherValue::Float(f) => collected.push(CypherValue::Float(f)),
            CypherValue::Boolean(b) => collected.push(CypherValue::Boolean(b)),
            CypherValue::Null => collected.push(CypherValue::Null),
            _ => bail!("collect requires simple type arguments"),
        }
    }
    Ok(CypherValue::List(collected))
}

// =============================================================================
// EXPRESSION EVALUATION
// =============================================================================

/// Evaluate a Cypher expression to a value
pub fn evaluate_expression(
    expr: &Expression,
    ctx: &FunctionContext,
) -> Result<CypherValue> {
    match expr {
        Expression::Literal(value) => Ok(value.clone()),

        Expression::Variable(name) => {
            ctx.get_variable(name)
                .cloned()
                .ok_or_else(|| anyhow!("Variable '{}' not found", name))
        }

        Expression::Property(box_expr, property) => {
            // For now, return a placeholder
            // A full implementation would navigate object properties
            let left_val = evaluate_expression(box_expr, ctx)?;
            let left_str = match &left_val {
                CypherValue::String(s) => s.clone(),
                _ => format!("{:?}", left_val),
            };
            Ok(CypherValue::String(format!("{}.{}", left_str, property)))
        }

        Expression::BinaryOp(left, op, right) => {
            let l = evaluate_expression(left, ctx)?;
            let r = evaluate_expression(right, ctx)?;
            evaluate_binary_op(&l, op, &r)
        }

        Expression::UnaryOp(unary_op, expr) => {
            let operand = evaluate_expression(expr, ctx)?;
            evaluate_unary_op(unary_op, &operand)
        }

        Expression::FunctionCall(name, args) => {
            evaluate_function_call(name, args, ctx)
        }

        Expression::Parameter(param) => {
            // For now, return a placeholder
            Ok(CypherValue::String(format!("${}", param)))
        }

        Expression::List(items) => {
            let mut evaluated = Vec::new();
            for item in items {
                evaluated.push(evaluate_expression(item, ctx)?);
            }
            Ok(CypherValue::List(evaluated))
        }

        Expression::Comparison(left, comp_op, right) => {
            let l = evaluate_expression(left, ctx)?;
            let r = evaluate_expression(right, ctx)?;
            Ok(CypherValue::Boolean(evaluate_comparison(&l, comp_op, &r)?))
        }

        // Reduce, ListComprehension, PatternComprehension are advanced Cypher
        // features not yet fully supported in the expression evaluator.
        _ => bail!("Unsupported expression type in evaluator"),
    }
}

/// Evaluate a binary operation
fn evaluate_binary_op(left: &CypherValue, op: &crate::graph::query::cypher_ast::BinaryOperator, right: &CypherValue) -> Result<CypherValue> {
    use crate::graph::query::cypher_ast::BinaryOperator;

    let l = to_numeric(left)?;
    let r = to_numeric(right)?;

    Ok(match op {
        BinaryOperator::Plus => CypherValue::Float(l + r),
        BinaryOperator::Minus => CypherValue::Float(l - r),
        BinaryOperator::Multiply => CypherValue::Float(l * r),
        BinaryOperator::Divide => {
            if r == 0.0 {
                bail!("Division by zero")
            }
            CypherValue::Float(l / r)
        }
        BinaryOperator::Modulo => {
            if r == 0.0 {
                bail!("Modulo by zero")
            }
            CypherValue::Float(l % r)
        }
        BinaryOperator::And => CypherValue::Boolean(
            truthy(left)? && truthy(right)?
        ),
        BinaryOperator::Or => CypherValue::Boolean(
            truthy(left)? || truthy(right)?
        ),
        BinaryOperator::Xor => CypherValue::Boolean(
            truthy(left)? ^ truthy(right)?
        ),
    })
}

/// Evaluate a unary operation
fn evaluate_unary_op(op: &crate::graph::query::cypher_ast::UnaryOperator, operand: &CypherValue) -> Result<CypherValue> {
    use crate::graph::query::cypher_ast::UnaryOperator;

    match op {
        UnaryOperator::Not => Ok(CypherValue::Boolean(!truthy(operand)?)),
        UnaryOperator::Negate => {
            let val = to_numeric(operand)?;
            Ok(CypherValue::Float(-val))
        }
    }
}

/// Evaluate a function call
fn evaluate_function_call(name: &str, args: &[Expression], ctx: &FunctionContext) -> Result<CypherValue> {
    let registry = CypherFunctionRegistry::new();

    let function = registry.get(name)
        .ok_or_else(|| anyhow!("Unknown function: {}", name))?;

    // Validate argument count
    if !function.variadic && (args.len() < function.min_args || args.len() > function.max_args) {
        bail!(
            "Function '{}' requires {}-{} arguments, got {}",
            name,
            function.min_args,
            function.max_args,
            args.len()
        );
    }

    // Call the function handler
    (function.handler)(args, ctx)
}

/// Evaluate a comparison operation
fn evaluate_comparison(left: &CypherValue, op: &crate::graph::query::cypher_ast::CompOp, right: &CypherValue) -> Result<bool> {
    use crate::graph::query::cypher_ast::CompOp;

    match op {
        CompOp::Eq => Ok(compare_values(left, right)? == 0),
        CompOp::Neq => Ok(compare_values(left, right)? != 0),
        CompOp::Lt => Ok(compare_values(left, right)? < 0),
        CompOp::Gt => Ok(compare_values(left, right)? > 0),
        CompOp::Lte => Ok(compare_values(left, right)? <= 0),
        CompOp::Gte => Ok(compare_values(left, right)? >= 0),
        CompOp::In => {
            // Simplified IN check
            if let CypherValue::List(items) = right {
                Ok(items.iter().any(|item| compare_values(left, item).unwrap_or(1) == 0))
            } else {
                Ok(false)
            }
        }
        _ => bail!("Comparison operator not yet implemented: {:?}", op),
    }
}

/// Compare two Cypher values, returning ordering (-1, 0, 1)
fn compare_values(left: &CypherValue, right: &CypherValue) -> Result<i32> {
    use std::cmp::Ordering;
    match (left, right) {
        (CypherValue::String(l), CypherValue::String(r)) => {
            Ok(match l.cmp(r) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            })
        }
        (CypherValue::Integer(l), CypherValue::Integer(r)) => {
            Ok(match l.cmp(r) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            })
        }
        (CypherValue::Integer(l), CypherValue::Float(r)) => {
            let l_f = *l as f64;
            let r_f = *r;
            Ok(match l_f.partial_cmp(&r_f).unwrap_or(Ordering::Equal) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            })
        }
        (CypherValue::Float(l), CypherValue::Integer(r)) => {
            let l_f = *l;
            let r_f = *r as f64;
            Ok(match l_f.partial_cmp(&r_f).unwrap_or(Ordering::Equal) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            })
        }
        (CypherValue::Float(l), CypherValue::Float(r)) => {
            Ok(match l.partial_cmp(r).unwrap_or(Ordering::Equal) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            })
        }
        (CypherValue::Boolean(l), CypherValue::Boolean(r)) => {
            Ok(match l.cmp(r) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            })
        }
        _ => Ok(0), // Null or complex types
    }
}

/// Convert a CypherValue to a numeric float
fn to_numeric(value: &CypherValue) -> Result<f64> {
    match value {
        CypherValue::Integer(i) => Ok(*i as f64),
        CypherValue::Float(f) => Ok(*f),
        CypherValue::Boolean(b) => Ok(if *b { 1.0 } else { 0.0 }),
        _ => bail!("Cannot convert to numeric: {:?}", value),
    }
}

/// Check if a value is truthy
fn truthy(value: &CypherValue) -> Result<bool> {
    match value {
        CypherValue::Boolean(b) => Ok(*b),
        CypherValue::Integer(i) => Ok(*i != 0),
        CypherValue::Float(f) => Ok(*f != 0.0),
        CypherValue::String(s) => Ok(!s.is_empty()),
        CypherValue::List(items) => Ok(!items.is_empty()),
        CypherValue::Null => Ok(false),
        _ => Ok(true), // Objects, Maps are truthy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_functions() {
        let ctx = FunctionContext::new();

        // Test toUpper
        let expr = Expression::FunctionCall("toUpper".to_string(), vec![
            Expression::Literal(CypherValue::String("hello".to_string()))
        ]);
        assert_eq!(
            evaluate_expression(&expr, &ctx).unwrap(),
            CypherValue::String("HELLO".to_string())
        );

        // Test concat
        let expr = Expression::FunctionCall("concat".to_string(), vec![
            Expression::Literal(CypherValue::String("Hello ".to_string())),
            Expression::Literal(CypherValue::String("World".to_string())),
        ]);
        assert_eq!(
            evaluate_expression(&expr, &ctx).unwrap(),
            CypherValue::String("Hello World".to_string())
        );
    }

    #[test]
    fn test_math_functions() {
        let ctx = FunctionContext::new();

        // Test abs
        let expr = Expression::FunctionCall("abs".to_string(), vec![
            Expression::Literal(CypherValue::Integer(-5))
        ]);
        assert_eq!(
            evaluate_expression(&expr, &ctx).unwrap(),
            CypherValue::Integer(5)
        );

        // Test round
        let expr = Expression::FunctionCall("round".to_string(), vec![
            Expression::Literal(CypherValue::Float(3.7)),
            Expression::Literal(CypherValue::Integer(1))
        ]);
        let result = evaluate_expression(&expr, &ctx).unwrap();
        match result {
            CypherValue::Float(f) => assert!((f - 3.7).abs() < 0.01),
            _ => panic!("Expected Float"),
        }
    }

    #[test]
    fn test_function_registry() {
        let registry = CypherFunctionRegistry::new();

        // Test that functions are registered
        assert!(registry.get("toUpper").is_some());
        assert!(registry.get("TOUPPER").is_some()); // Case-insensitive
        assert!(registry.get("unknown").is_none());

        // List all functions
        let functions = registry.list_functions();
        assert!(functions.contains(&"TOUPPER".to_string()));
        assert!(functions.contains(&"SQRT".to_string()));
        assert!(functions.contains(&"TIMESTAMP".to_string()));
    }
}
