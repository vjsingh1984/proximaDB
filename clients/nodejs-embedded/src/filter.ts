/**
 * ProximaDB TypeScript SDK - Filter Builder
 *
 * Provides a fluent API for building metadata filters on vector searches.
 *
 * Copyright 2025 ProximaDB Contributors
 * Licensed under the Apache License, Version 2.0
 */

import {
  Filter,
  FilterCondition,
  FilterGroup,
  FilterNode,
  FilterOp,
  LogicalOp,
  JsonValue,
} from "./types";

/**
 * Check if a node is a FilterCondition
 */
function isFilterCondition(node: FilterNode): node is FilterCondition {
  return "field" in node && "operation" in node;
}

/**
 * Check if a node is a FilterGroup
 */
function isFilterGroup(node: FilterNode): node is FilterGroup {
  return "operator" in node && "conditions" in node;
}

/**
 * Convert a filter condition to expression string
 */
function conditionToExpression(condition: FilterCondition): string {
  const { field, operation, value } = condition;

  if (value === undefined) {
    const opStr = operation === "exists" ? "EXISTS" : "IS NULL";
    return field + " " + opStr;
  }

  let valStr: string;
  if (typeof value === "string") {
    valStr = "'" + value + "'";
  } else if (Array.isArray(value)) {
    const items = value.map((x) =>
      typeof x === "string" ? "'" + x + "'" : String(x)
    );
    valStr = "[" + items.join(", ") + "]";
  } else {
    valStr = String(value);
  }

  const opMap: Record<string, string> = {
    equals: "=",
    not_equals: "!=",
    gt: ">",
    gte: ">=",
    lt: "<",
    lte: "<=",
    in: "IN",
    not_in: "NOT IN",
    contains: "CONTAINS",
    starts_with: "STARTS WITH",
    ends_with: "ENDS WITH",
  };

  const opStr = opMap[operation] ?? operation;
  return field + " " + opStr + " " + valStr;
}

/**
 * Convert a filter group to expression string
 */
function groupToExpression(group: FilterGroup): string {
  const exprs: string[] = group.conditions
    .map((node) => {
      if (isFilterCondition(node)) {
        return conditionToExpression(node);
      } else if (isFilterGroup(node)) {
        return "(" + groupToExpression(node) + ")";
      }
      return "";
    })
    .filter((s) => s.length > 0);

  const sep = group.operator === "and" ? " AND " : " OR ";
  return exprs.join(sep);
}

/**
 * Builder for constructing complex filter expressions
 */
export class FilterBuilder {
  private conditions: FilterNode[] = [];
  private currentOperator: LogicalOp = LogicalOp.And;
  private pendingOperator: LogicalOp | null = null;

  static new(): FilterBuilder {
    return new FilterBuilder();
  }

  eq(field: string, value: JsonValue): FilterBuilder {
    this.addCondition({ field, operation: FilterOp.Eq, value });
    return this;
  }

  ne(field: string, value: JsonValue): FilterBuilder {
    this.addCondition({ field, operation: FilterOp.Ne, value });
    return this;
  }

  gt(field: string, value: JsonValue): FilterBuilder {
    this.addCondition({ field, operation: FilterOp.Gt, value });
    return this;
  }

  gte(field: string, value: JsonValue): FilterBuilder {
    this.addCondition({ field, operation: FilterOp.Gte, value });
    return this;
  }

  lt(field: string, value: JsonValue): FilterBuilder {
    this.addCondition({ field, operation: FilterOp.Lt, value });
    return this;
  }

  lte(field: string, value: JsonValue): FilterBuilder {
    this.addCondition({ field, operation: FilterOp.Lte, value });
    return this;
  }

  inList(field: string, values: JsonValue[]): FilterBuilder {
    this.addCondition({ field, operation: FilterOp.In, value: values });
    return this;
  }

  notIn(field: string, values: JsonValue[]): FilterBuilder {
    this.addCondition({ field, operation: FilterOp.NotIn, value: values });
    return this;
  }

  contains(field: string, value: JsonValue): FilterBuilder {
    this.addCondition({ field, operation: FilterOp.Contains, value });
    return this;
  }

  startsWith(field: string, prefixVal: string): FilterBuilder {
    this.addCondition({ field, operation: FilterOp.StartsWith, value: prefixVal });
    return this;
  }

  endsWith(field: string, suffixVal: string): FilterBuilder {
    this.addCondition({ field, operation: FilterOp.EndsWith, value: suffixVal });
    return this;
  }

  prefix(field: string, prefixVal: string): FilterBuilder {
    return this.startsWith(field, prefixVal);
  }

  suffix(field: string, suffixVal: string): FilterBuilder {
    return this.endsWith(field, suffixVal);
  }

  exists(field: string): FilterBuilder {
    this.addCondition({ field, operation: FilterOp.Exists });
    return this;
  }

  isNull(field: string): FilterBuilder {
    this.addCondition({ field, operation: FilterOp.IsNull });
    return this;
  }

  range(field: string, min: JsonValue, max: JsonValue): FilterBuilder {
    this.addCondition({ field, operation: FilterOp.Gte, value: min });
    this.addCondition({ field, operation: FilterOp.Lte, value: max });
    return this;
  }

  and(): FilterBuilder {
    this.pendingOperator = LogicalOp.And;
    return this;
  }

  or(): FilterBuilder {
    this.pendingOperator = LogicalOp.Or;
    return this;
  }

  group(builderFn: (fb: FilterBuilder) => FilterBuilder): FilterBuilder {
    const inner = builderFn(new FilterBuilder());
    const filter = inner.build();
    this.conditions.push({
      operator: filter.operator,
      conditions: filter.conditions,
    });
    return this;
  }

  private addCondition(condition: FilterCondition): void {
    if (this.pendingOperator !== null) {
      if (
        this.pendingOperator === LogicalOp.Or &&
        this.currentOperator === LogicalOp.And
      ) {
        this.currentOperator = LogicalOp.Or;
      }
      this.pendingOperator = null;
    }
    this.conditions.push(condition);
  }

  build(): Filter {
    return {
      operator: this.currentOperator,
      conditions: [...this.conditions],
    };
  }

  toExpression(): string {
    const filter = this.build();
    return groupToExpression({
      operator: filter.operator,
      conditions: filter.conditions,
    });
  }

  toJson(): string {
    return JSON.stringify(this.build());
  }
}

export function eq(field: string, value: JsonValue): Filter {
  return FilterBuilder.new().eq(field, value).build();
}

export function ne(field: string, value: JsonValue): Filter {
  return FilterBuilder.new().ne(field, value).build();
}

export function gt(field: string, value: JsonValue): Filter {
  return FilterBuilder.new().gt(field, value).build();
}

export function lt(field: string, value: JsonValue): Filter {
  return FilterBuilder.new().lt(field, value).build();
}

export function inList(field: string, values: JsonValue[]): Filter {
  return FilterBuilder.new().inList(field, values).build();
}

export function range(field: string, min: JsonValue, max: JsonValue): Filter {
  return FilterBuilder.new().range(field, min, max).build();
}

export function andFilters(filters: Filter[]): Filter {
  const conditions: FilterNode[] = filters.flatMap((f) => f.conditions);
  return { operator: LogicalOp.And, conditions };
}

export function orFilters(filters: Filter[]): Filter {
  const conditions: FilterNode[] = filters.map((f) => ({
    operator: f.operator,
    conditions: f.conditions,
  }));
  return { operator: LogicalOp.Or, conditions };
}

export function filterToExpression(filter: Filter): string {
  return groupToExpression({
    operator: filter.operator,
    conditions: filter.conditions,
  });
}
