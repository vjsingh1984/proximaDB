"""
Code-aware chunking strategy using Tree-sitter for AST parsing.

.. deprecated:: TD-CG2 (ADR-028)
    This in-SDK code chunker duplicates the tree-sitter symbol+relation chunker that
    now lives in the shared, neutral ``victor-codegraph`` package (Victor owns it; the
    ProximaDB SDK, Victor, and AnvaiOps all consume it). When ``victor_codegraph`` is
    installed (``pip install 'proximadb[codegraph]'``), ``CodeChunkingStrategy``
    **delegates** to it; otherwise it falls back to the legacy in-file implementation
    below. The legacy implementation is slated for deletion one minor release after
    ``victor-codegraph`` is published — prefer ``from victor_codegraph import chunk``.

This module provides AST-based code chunking that produces semantic code units
(functions, classes, methods) with full structural awareness and relationship extraction.

Unlike text-based chunking, this:
- Respects code structure (never splits a function mid-statement)
- Extracts symbols with fully qualified names
- Identifies relationships (calls, imports, inheritance)
- Preserves documentation and type annotations
"""

import hashlib
import os
import re
import warnings
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from enum import IntEnum
from typing import Any

from .base import ChunkingConfig, ChunkingStrategyInterface, TextChunk

# Optional delegation target (TD-CG2). Imported softly so the SDK keeps working without
# the `codegraph` extra; when present, it is the single source of truth for code chunking.
try:  # pragma: no cover - availability depends on the optional extra
    import victor_codegraph as _victor_codegraph
except Exception:  # ImportError, or a partial/native load failure
    _victor_codegraph = None


def _warn_code_chunker_deprecated() -> None:
    """Steer callers toward the shared ``victor-codegraph`` package (ADR-028 / TD-CG2)."""

    warnings.warn(
        "proximadb_sdk.chunking_strategies.code is deprecated (TD-CG2): the tree-sitter "
        "code chunker now lives in the shared 'victor-codegraph' package. Install "
        "`proximadb[codegraph]` and prefer `from victor_codegraph import chunk`. The "
        "legacy in-SDK implementation will be removed in a future minor release.",
        DeprecationWarning,
        stacklevel=3,
    )


class CodeSymbolType(IntEnum):
    """Types of code symbols that can be extracted"""

    FILE = 1
    MODULE = 2
    PACKAGE = 3
    CLASS = 4
    INTERFACE = 5
    TRAIT = 6
    STRUCT = 7
    ENUM = 8
    FUNCTION = 9
    METHOD = 10
    CONSTRUCTOR = 11
    PROPERTY = 12
    FIELD = 13
    CONSTANT = 14
    VARIABLE = 15
    PARAMETER = 16
    TYPE_ALIAS = 17
    MACRO = 18


class CodeRelationType(IntEnum):
    """Types of relationships between code symbols"""

    CALLS = 1
    CALLED_BY = 2
    EXTENDS = 3
    IMPLEMENTS = 4
    USES_TYPE = 5
    RETURNS_TYPE = 6
    IMPORTS = 7
    IMPORTED_BY = 8
    DEPENDS_ON = 9
    CONTAINS = 10
    CONTAINED_BY = 11
    DEFINES = 12
    REFERENCES = 13
    REFERENCED_BY = 14
    OVERRIDES = 15
    OVERRIDDEN_BY = 16
    TESTS = 17
    TESTED_BY = 18


@dataclass
class SourceLocation:
    """Source code location information"""

    file_path: str
    repository: str | None = None
    branch: str | None = None
    commit_hash: str | None = None
    start_line: int = 0
    start_column: int = 0
    end_line: int = 0
    end_column: int = 0
    byte_offset: int = 0
    byte_length: int = 0


@dataclass
class CodeSymbol:
    """Represents a code symbol (function, class, method, etc.)"""

    id: str
    symbol_type: CodeSymbolType
    fully_qualified_name: str
    simple_name: str
    location: SourceLocation
    source_code: str
    language: str
    documentation: str | None = None
    signature: str | None = None
    modifiers: list[str] = field(default_factory=list)
    scope_chain: list[str] = field(default_factory=list)
    parameters: list[dict[str, Any]] = field(default_factory=list)
    return_type: str | None = None
    complexity: dict[str, int] | None = None
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass
class CodeRelation:
    """Represents a relationship between two code symbols"""

    from_symbol_id: str
    to_symbol_id: str
    relation_type: CodeRelationType
    call_site: SourceLocation | None = None
    context: str | None = None
    confidence: float = 1.0


@dataclass
class ParsedCode:
    """Result of parsing a code file"""

    file_path: str
    language: str
    symbols: list[CodeSymbol]
    relations: list[CodeRelation]
    imports: list[str]
    content_hash: str


@dataclass
class CodeChunkingConfig(ChunkingConfig):
    """Extended configuration for code-aware chunking"""

    # Languages to parse (None = auto-detect from extension)
    languages: list[str] | None = None

    # Include private/internal symbols
    include_private: bool = True

    # Include test files/functions
    include_tests: bool = True

    # Extract call relationships
    extract_relations: bool = True

    # Maximum symbol depth to recurse into
    max_symbol_depth: int = 10

    # Include context (surrounding code)
    include_code_context: bool = True
    context_lines: int = 5


class LanguageParser(ABC):
    """Abstract base for language-specific parsers"""

    @property
    @abstractmethod
    def language(self) -> str:
        """Language name"""
        pass

    @property
    @abstractmethod
    def file_extensions(self) -> list[str]:
        """Supported file extensions"""
        pass

    @abstractmethod
    def parse(self, content: str, file_path: str) -> ParsedCode:
        """Parse content and extract symbols/relations"""
        pass


class PythonParser(LanguageParser):
    """
    Python-specific parser using Tree-sitter.

    Extracts functions, classes, methods with full context.
    """

    def __init__(self):
        self._parser = None
        self._language = None
        self._init_parser()

    def _init_parser(self):
        """Initialize tree-sitter parser for Python"""
        try:
            from tree_sitter_language_pack import get_language, get_parser

            self._parser = get_parser("python")
            self._language = get_language("python")
        except (ImportError, OSError) as e:
            # Grammar not installed / native module unavailable -> regex fallback.
            import logging

            logging.debug(f"Tree-sitter unavailable, using regex fallback: {e}")
            self._parser = None
            self._language = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for python; "
                "using regex fallback",
                exc_info=True,
            )
            self._parser = None
            self._language = None

    @property
    def language(self) -> str:
        return "python"

    @property
    def file_extensions(self) -> list[str]:
        return [".py", ".pyi"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        """Parse Python code and extract symbols/relations"""
        content_hash = hashlib.sha256(content.encode()).hexdigest()

        if self._parser is not None:
            return self._parse_with_treesitter(content, file_path, content_hash)
        else:
            return self._parse_with_regex(content, file_path, content_hash)

    def _parse_with_treesitter(
        self, content: str, file_path: str, content_hash: str
    ) -> ParsedCode:
        """Parse using tree-sitter AST"""
        tree = self._parser.parse(bytes(content, "utf8"))

        symbols = []
        relations = []
        imports = []

        # Extract imports
        imports = self._extract_imports_ts(tree.root_node, content)

        # Extract top-level functions and classes
        for child in tree.root_node.children:
            if child.type == "function_definition":
                symbol = self._extract_function_ts(child, content, file_path, [])
                if symbol:
                    symbols.append(symbol)
            elif child.type == "class_definition":
                class_symbol, method_symbols = self._extract_class_ts(
                    child, content, file_path
                )
                if class_symbol:
                    symbols.append(class_symbol)
                    symbols.extend(method_symbols)
            elif child.type == "decorated_definition":
                # Handle decorated functions/classes
                decorated = child.children[-1] if child.children else None
                if decorated and decorated.type == "function_definition":
                    symbol = self._extract_function_ts(
                        decorated,
                        content,
                        file_path,
                        [],
                        decorators=self._extract_decorators_ts(child, content),
                    )
                    if symbol:
                        symbols.append(symbol)
                elif decorated and decorated.type == "class_definition":
                    class_symbol, method_symbols = self._extract_class_ts(
                        decorated,
                        content,
                        file_path,
                        decorators=self._extract_decorators_ts(child, content),
                    )
                    if class_symbol:
                        symbols.append(class_symbol)
                        symbols.extend(method_symbols)

        # Extract call relations
        relations = self._extract_relations_ts(tree.root_node, content, symbols)

        return ParsedCode(
            file_path=file_path,
            language="python",
            symbols=symbols,
            relations=relations,
            imports=imports,
            content_hash=content_hash,
        )

    def _extract_imports_ts(self, node, content: str) -> list[str]:
        """Extract import statements"""
        imports = []
        for child in node.children:
            if child.type in ("import_statement", "import_from_statement"):
                imports.append(content[child.start_byte : child.end_byte])
        return imports

    def _extract_decorators_ts(self, node, content: str) -> list[str]:
        """Extract decorators from a decorated_definition"""
        decorators = []
        for child in node.children:
            if child.type == "decorator":
                decorators.append(content[child.start_byte : child.end_byte])
        return decorators

    def _extract_function_ts(
        self,
        node,
        content: str,
        file_path: str,
        scope_chain: list[str],
        decorators: list[str] | None = None,
    ) -> CodeSymbol | None:
        """Extract a function/method from AST node"""
        name_node = None
        params_node = None
        return_type_node = None
        body_node = None

        for child in node.children:
            if child.type == "identifier" and name_node is None:
                name_node = child
            elif child.type == "parameters":
                params_node = child
            elif child.type == "type":
                return_type_node = child
            elif child.type == "block":
                body_node = child

        if not name_node:
            return None

        name = content[name_node.start_byte : name_node.end_byte]
        source = content[node.start_byte : node.end_byte]

        # Determine symbol type
        symbol_type = CodeSymbolType.FUNCTION
        if scope_chain:
            symbol_type = CodeSymbolType.METHOD
            if name == "__init__":
                symbol_type = CodeSymbolType.CONSTRUCTOR

        # Extract parameters
        params = []
        if params_node:
            params = self._extract_parameters_ts(params_node, content)

        # Extract return type
        return_type = None
        if return_type_node:
            return_type = content[
                return_type_node.start_byte : return_type_node.end_byte
            ]

        # Extract docstring
        doc = self._extract_docstring_ts(body_node, content) if body_node else None

        # Build modifiers from decorators
        modifiers = decorators or []
        if name.startswith("_") and not name.startswith("__"):
            modifiers.append("private")
        if name.startswith("__") and name.endswith("__"):
            modifiers.append("dunder")

        # Build fully qualified name
        fqn_parts = [file_path.replace("/", ".").replace("\\", ".")]
        fqn_parts.extend(scope_chain)
        fqn_parts.append(name)
        fqn = "::".join(fqn_parts)

        # Generate ID
        symbol_id = self._generate_symbol_id(
            file_path, name, node.start_point[0], node.start_point[1]
        )

        return CodeSymbol(
            id=symbol_id,
            symbol_type=symbol_type,
            fully_qualified_name=fqn,
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                start_column=node.start_point[1],
                end_line=node.end_point[0] + 1,
                end_column=node.end_point[1],
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="python",
            documentation=doc,
            signature=self._build_signature(name, params, return_type),
            modifiers=modifiers,
            scope_chain=scope_chain,
            parameters=params,
            return_type=return_type,
            complexity=self._calculate_complexity_ts(node, content),
        )

    def _extract_class_ts(
        self, node, content: str, file_path: str, decorators: list[str] | None = None
    ) -> tuple[CodeSymbol | None, list[CodeSymbol]]:
        """Extract a class and its methods from AST node"""
        name_node = None
        bases_node = None
        body_node = None

        for child in node.children:
            if child.type == "identifier" and name_node is None:
                name_node = child
            elif child.type == "argument_list":
                bases_node = child
            elif child.type == "block":
                body_node = child

        if not name_node:
            return None, []

        name = content[name_node.start_byte : name_node.end_byte]
        source = content[node.start_byte : node.end_byte]

        # Extract base classes
        bases = []
        if bases_node:
            for child in bases_node.children:
                if child.type == "identifier":
                    bases.append(content[child.start_byte : child.end_byte])

        # Build modifiers
        modifiers = decorators or []
        if bases:
            modifiers.append(f"extends({','.join(bases)})")

        # Build fully qualified name
        fqn = f"{file_path.replace('/', '.').replace(os.sep, '.')}::{name}"

        # Generate ID
        symbol_id = self._generate_symbol_id(
            file_path, name, node.start_point[0], node.start_point[1]
        )

        # Extract docstring
        doc = self._extract_docstring_ts(body_node, content) if body_node else None

        class_symbol = CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.CLASS,
            fully_qualified_name=fqn,
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                start_column=node.start_point[1],
                end_line=node.end_point[0] + 1,
                end_column=node.end_point[1],
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="python",
            documentation=doc,
            modifiers=modifiers,
            scope_chain=[],
        )

        # Extract methods
        method_symbols = []
        if body_node:
            for child in body_node.children:
                if child.type == "function_definition":
                    method = self._extract_function_ts(
                        child, content, file_path, [name]
                    )
                    if method:
                        method_symbols.append(method)
                elif child.type == "decorated_definition":
                    decorated = child.children[-1] if child.children else None
                    if decorated and decorated.type == "function_definition":
                        method = self._extract_function_ts(
                            decorated,
                            content,
                            file_path,
                            [name],
                            decorators=self._extract_decorators_ts(child, content),
                        )
                        if method:
                            method_symbols.append(method)

        return class_symbol, method_symbols

    def _extract_parameters_ts(self, node, content: str) -> list[dict[str, Any]]:
        """Extract parameter information from parameters node"""
        params = []
        for child in node.children:
            if child.type in (
                "identifier",
                "typed_parameter",
                "default_parameter",
                "typed_default_parameter",
                "list_splat_pattern",
                "dictionary_splat_pattern",
            ):
                param = self._parse_parameter_ts(child, content)
                if param:
                    params.append(param)
        return params

    def _parse_parameter_ts(self, node, content: str) -> dict[str, Any] | None:
        """Parse a single parameter node"""
        if node.type == "identifier":
            name = content[node.start_byte : node.end_byte]
            if name in ("self", "cls"):
                return None  # Skip self/cls
            return {"name": name}
        elif node.type == "typed_parameter":
            name = None
            type_ann = None
            for child in node.children:
                if child.type == "identifier" and name is None:
                    name = content[child.start_byte : child.end_byte]
                elif child.type == "type":
                    type_ann = content[child.start_byte : child.end_byte]
            if name and name not in ("self", "cls"):
                return {"name": name, "type": type_ann}
        elif node.type in ("default_parameter", "typed_default_parameter"):
            name = None
            default = None
            type_ann = None
            for child in node.children:
                if child.type == "identifier" and name is None:
                    name = content[child.start_byte : child.end_byte]
                elif child.type == "type":
                    type_ann = content[child.start_byte : child.end_byte]
                elif child.type not in ("identifier", "type", "="):
                    default = content[child.start_byte : child.end_byte]
            if name and name not in ("self", "cls"):
                return {
                    "name": name,
                    "type": type_ann,
                    "default": default,
                    "is_optional": True,
                }
        elif node.type == "list_splat_pattern":
            for child in node.children:
                if child.type == "identifier":
                    name = content[child.start_byte : child.end_byte]
                    return {"name": f"*{name}", "is_variadic": True}
        elif node.type == "dictionary_splat_pattern":
            for child in node.children:
                if child.type == "identifier":
                    name = content[child.start_byte : child.end_byte]
                    return {"name": f"**{name}", "is_variadic": True}
        return None

    def _extract_docstring_ts(self, body_node, content: str) -> str | None:
        """Extract docstring from function/class body"""
        if not body_node or not body_node.children:
            return None

        first_stmt = body_node.children[0]
        if first_stmt.type == "expression_statement":
            expr = first_stmt.children[0] if first_stmt.children else None
            if expr and expr.type == "string":
                doc = content[expr.start_byte : expr.end_byte]
                # Remove quotes
                if doc.startswith('"""') or doc.startswith("'''"):
                    doc = doc[3:-3]
                elif doc.startswith('"') or doc.startswith("'"):
                    doc = doc[1:-1]
                return doc.strip()
        return None

    def _extract_relations_ts(
        self, node, content: str, symbols: list[CodeSymbol]
    ) -> list[CodeRelation]:
        """Extract call relationships from AST"""
        relations = []
        symbol_map = {s.simple_name: s for s in symbols}
        symbol_id_map = {s.id: s for s in symbols}

        # Find all call expressions
        def find_calls(n, containing_symbol: CodeSymbol | None = None):
            # Update containing symbol based on position
            for sym in symbols:
                if (
                    sym.location.byte_offset <= n.start_byte
                    and n.end_byte
                    <= sym.location.byte_offset + sym.location.byte_length
                ):
                    if sym.symbol_type in (
                        CodeSymbolType.FUNCTION,
                        CodeSymbolType.METHOD,
                        CodeSymbolType.CONSTRUCTOR,
                    ):
                        containing_symbol = sym
                        break

            if n.type == "call":
                callee_name = self._get_callee_name_ts(n, content)
                if callee_name and callee_name in symbol_map and containing_symbol:
                    relations.append(
                        CodeRelation(
                            from_symbol_id=containing_symbol.id,
                            to_symbol_id=symbol_map[callee_name].id,
                            relation_type=CodeRelationType.CALLS,
                            call_site=SourceLocation(
                                file_path=containing_symbol.location.file_path,
                                start_line=n.start_point[0] + 1,
                                start_column=n.start_point[1],
                            ),
                        )
                    )

            for child in n.children:
                find_calls(child, containing_symbol)

        find_calls(node)
        return relations

    def _get_callee_name_ts(self, node, content: str) -> str | None:
        """Get the name of the called function"""
        for child in node.children:
            if child.type == "identifier":
                return content[child.start_byte : child.end_byte]
            elif child.type == "attribute":
                # Get the attribute name (last identifier)
                for attr_child in reversed(child.children):
                    if attr_child.type == "identifier":
                        return content[attr_child.start_byte : attr_child.end_byte]
        return None

    def _calculate_complexity_ts(self, node, content: str) -> dict[str, int]:
        """Calculate complexity metrics for a function"""
        complexity = {
            "cyclomatic": 1,  # Base complexity
            "cognitive": 0,
            "lines": node.end_point[0] - node.start_point[0] + 1,
            "nesting_depth": 0,
        }

        def count_complexity(n, depth=0):
            if n.type in (
                "if_statement",
                "elif_clause",
                "for_statement",
                "while_statement",
                "try_statement",
                "except_clause",
                "with_statement",
                "match_statement",
            ):
                complexity["cyclomatic"] += 1
                complexity["cognitive"] += 1 + depth  # Nesting penalty

            if n.type in ("and", "or"):
                complexity["cyclomatic"] += 1

            complexity["nesting_depth"] = max(complexity["nesting_depth"], depth)

            for child in n.children:
                child_depth = (
                    depth + 1
                    if n.type
                    in (
                        "if_statement",
                        "for_statement",
                        "while_statement",
                        "with_statement",
                        "try_statement",
                    )
                    else depth
                )
                count_complexity(child, child_depth)

        count_complexity(node)
        return complexity

    def _build_signature(
        self, name: str, params: list[dict], return_type: str | None
    ) -> str:
        """Build function signature string"""
        param_strs = []
        for p in params:
            s = p["name"]
            if p.get("type"):
                s += f": {p['type']}"
            if p.get("default"):
                s += f" = {p['default']}"
            param_strs.append(s)

        sig = f"{name}({', '.join(param_strs)})"
        if return_type:
            sig += f" -> {return_type}"
        return sig

    def _generate_symbol_id(
        self, file_path: str, name: str, line: int, col: int
    ) -> str:
        """Generate unique symbol ID"""
        key = f"{file_path}:{name}:{line}:{col}"
        return hashlib.sha256(key.encode()).hexdigest()[:16]

    def _parse_with_regex(
        self, content: str, file_path: str, content_hash: str
    ) -> ParsedCode:
        """Fallback regex-based parsing when tree-sitter not available"""
        symbols = []
        relations = []
        imports = []

        lines = content.split("\n")

        # Extract imports
        import_pattern = re.compile(r"^(?:from\s+\S+\s+)?import\s+.+$")
        for i, line in enumerate(lines):
            if import_pattern.match(line.strip()):
                imports.append(line.strip())

        # Extract functions
        func_pattern = re.compile(
            r"^(\s*)(async\s+)?def\s+(\w+)\s*\(([^)]*)\)(?:\s*->\s*(\S+))?\s*:"
        )

        # Extract classes
        class_pattern = re.compile(r"^(\s*)class\s+(\w+)(?:\(([^)]*)\))?\s*:")

        current_class = None
        current_class_indent = -1

        for i, line in enumerate(lines):
            # Check class
            class_match = class_pattern.match(line)
            if class_match:
                indent = len(class_match.group(1))
                name = class_match.group(2)
                bases = class_match.group(3)

                current_class = name
                current_class_indent = indent

                # Find class end
                end_line = self._find_block_end_regex(lines, i, indent)
                source = "\n".join(lines[i : end_line + 1])

                symbol_id = self._generate_symbol_id(file_path, name, i, 0)
                symbols.append(
                    CodeSymbol(
                        id=symbol_id,
                        symbol_type=CodeSymbolType.CLASS,
                        fully_qualified_name=f"{file_path}::{name}",
                        simple_name=name,
                        location=SourceLocation(
                            file_path=file_path, start_line=i + 1, end_line=end_line + 1
                        ),
                        source_code=source,
                        language="python",
                        modifiers=[f"extends({bases})"] if bases else [],
                    )
                )
                continue

            # Check function
            func_match = func_pattern.match(line)
            if func_match:
                indent = len(func_match.group(1))
                is_async = bool(func_match.group(2))
                name = func_match.group(3)
                params_str = func_match.group(4)
                return_type = func_match.group(5)

                # Determine if method or function
                is_method = current_class and indent > current_class_indent

                # Find function end
                end_line = self._find_block_end_regex(lines, i, indent)
                source = "\n".join(lines[i : end_line + 1])

                # Parse parameters
                params = self._parse_params_regex(params_str)

                # Determine symbol type
                symbol_type = (
                    CodeSymbolType.METHOD if is_method else CodeSymbolType.FUNCTION
                )
                if name == "__init__":
                    symbol_type = CodeSymbolType.CONSTRUCTOR

                scope_chain = [current_class] if is_method else []
                fqn_parts = [file_path] + scope_chain + [name]

                symbol_id = self._generate_symbol_id(file_path, name, i, 0)

                modifiers = []
                if is_async:
                    modifiers.append("async")
                if name.startswith("_") and not name.startswith("__"):
                    modifiers.append("private")

                symbols.append(
                    CodeSymbol(
                        id=symbol_id,
                        symbol_type=symbol_type,
                        fully_qualified_name="::".join(fqn_parts),
                        simple_name=name,
                        location=SourceLocation(
                            file_path=file_path, start_line=i + 1, end_line=end_line + 1
                        ),
                        source_code=source,
                        language="python",
                        signature=self._build_signature(name, params, return_type),
                        modifiers=modifiers,
                        scope_chain=scope_chain,
                        parameters=params,
                        return_type=return_type,
                    )
                )

            # Reset class context if we go back to lower indent
            if (
                current_class
                and line.strip()
                and not line.startswith(" " * (current_class_indent + 1))
            ):
                if not class_pattern.match(line) and not func_pattern.match(line):
                    current_class = None
                    current_class_indent = -1

        return ParsedCode(
            file_path=file_path,
            language="python",
            symbols=symbols,
            relations=relations,
            imports=imports,
            content_hash=content_hash,
        )

    def _find_block_end_regex(
        self, lines: list[str], start: int, base_indent: int
    ) -> int:
        """Find the end of a block based on indentation"""
        for i in range(start + 1, len(lines)):
            line = lines[i]
            if not line.strip():  # Empty line
                continue
            current_indent = len(line) - len(line.lstrip())
            if current_indent <= base_indent and line.strip():
                return i - 1
        return len(lines) - 1

    def _parse_params_regex(self, params_str: str) -> list[dict[str, Any]]:
        """Parse parameters from string"""
        params = []
        if not params_str.strip():
            return params

        # Simple parameter parsing (doesn't handle all edge cases)
        for param in params_str.split(","):
            param = param.strip()
            if not param or param in ("self", "cls"):
                continue

            p = {"name": param}

            # Check for type annotation
            if ":" in param:
                parts = param.split(":")
                p["name"] = parts[0].strip()
                type_part = parts[1].strip()
                if "=" in type_part:
                    type_parts = type_part.split("=")
                    p["type"] = type_parts[0].strip()
                    p["default"] = type_parts[1].strip()
                    p["is_optional"] = True
                else:
                    p["type"] = type_part
            elif "=" in param:
                parts = param.split("=")
                p["name"] = parts[0].strip()
                p["default"] = parts[1].strip()
                p["is_optional"] = True

            if p["name"].startswith("*"):
                p["is_variadic"] = True

            params.append(p)

        return params


class JavaScriptParser(LanguageParser):
    """JavaScript/TypeScript parser using Tree-sitter"""

    def __init__(self, typescript: bool = False):
        self._typescript = typescript
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            lang = "typescript" if self._typescript else "javascript"
            self._parser = get_parser(lang)
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "typescript" if self._typescript else "javascript"

    @property
    def file_extensions(self) -> list[str]:
        if self._typescript:
            return [".ts", ".tsx"]
        return [".js", ".jsx", ".mjs", ".cjs"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        # Implementation similar to PythonParser but for JS/TS
        content_hash = hashlib.sha256(content.encode()).hexdigest()

        # Placeholder - would implement full JS/TS parsing
        return ParsedCode(
            file_path=file_path,
            language=self.language,
            symbols=[],
            relations=[],
            imports=[],
            content_hash=content_hash,
        )


# ============================================================================
# Additional Language Parsers (Tree-sitter based, pluggable architecture)
# ============================================================================


class RustParser(LanguageParser):
    """Rust parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("rust")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "rust"

    @property
    def file_extensions(self) -> list[str]:
        return [".rs"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()

        if self._parser is None:
            return self._parse_with_regex(content, file_path, content_hash)

        tree = self._parser.parse(bytes(content, "utf8"))
        symbols = []
        relations = []
        imports = []

        # Extract use statements
        for child in tree.root_node.children:
            if child.type == "use_declaration":
                imports.append(content[child.start_byte : child.end_byte])

        # Extract functions, structs, impls, enums, traits
        self._extract_rust_items(tree.root_node, content, file_path, [], symbols)
        relations = self._extract_rust_relations(tree.root_node, content, symbols)

        return ParsedCode(
            file_path=file_path,
            language="rust",
            symbols=symbols,
            relations=relations,
            imports=imports,
            content_hash=content_hash,
        )

    def _extract_rust_items(
        self,
        node,
        content: str,
        file_path: str,
        scope_chain: list[str],
        symbols: list[CodeSymbol],
    ):
        """Extract Rust items recursively"""
        for child in node.children:
            if child.type == "function_item":
                sym = self._extract_rust_function(
                    child, content, file_path, scope_chain
                )
                if sym:
                    symbols.append(sym)
            elif child.type == "struct_item":
                sym, fields = self._extract_rust_struct(child, content, file_path)
                if sym:
                    symbols.append(sym)
                    symbols.extend(fields)
            elif child.type == "enum_item":
                sym = self._extract_rust_enum(child, content, file_path)
                if sym:
                    symbols.append(sym)
            elif child.type == "trait_item":
                sym, methods = self._extract_rust_trait(child, content, file_path)
                if sym:
                    symbols.append(sym)
                    symbols.extend(methods)
            elif child.type == "impl_item":
                self._extract_rust_impl(child, content, file_path, symbols)
            elif child.type == "mod_item":
                # Handle nested modules
                mod_name = self._get_rust_name(child, content)
                if mod_name:
                    new_scope = scope_chain + [mod_name]
                    self._extract_rust_items(
                        child, content, file_path, new_scope, symbols
                    )

    def _extract_rust_function(
        self, node, content: str, file_path: str, scope_chain: list[str]
    ) -> CodeSymbol | None:
        """Extract Rust function/method"""
        name = self._get_rust_name(node, content)
        if not name:
            return None

        source = content[node.start_byte : node.end_byte]

        # Extract visibility and other modifiers
        modifiers = []
        for child in node.children:
            if child.type == "visibility_modifier":
                modifiers.append(content[child.start_byte : child.end_byte])
            elif child.type == "async":
                modifiers.append("async")
            elif child.type == "unsafe":
                modifiers.append("unsafe")

        # Extract parameters
        params = []
        for child in node.children:
            if child.type == "parameters":
                params = self._extract_rust_params(child, content)
                break

        # Extract return type
        return_type = None
        for child in node.children:
            if child.type == "return_type" or child.type == "type":
                # Look for the type node after ->
                for sub in child.children:
                    if sub.type not in ("->", "where_clause"):
                        return_type = content[sub.start_byte : sub.end_byte]
                        break
                if not return_type:
                    return_type = content[child.start_byte : child.end_byte]

        # Build FQN
        fqn = "::".join([file_path] + scope_chain + [name])
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        # Determine symbol type
        symbol_type = CodeSymbolType.METHOD if scope_chain else CodeSymbolType.FUNCTION

        return CodeSymbol(
            id=symbol_id,
            symbol_type=symbol_type,
            fully_qualified_name=fqn,
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                start_column=node.start_point[1],
                end_column=node.end_point[1],
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="rust",
            signature=self._build_rust_signature(name, params, return_type),
            modifiers=modifiers,
            scope_chain=scope_chain,
            parameters=params,
            return_type=return_type,
        )

    def _extract_rust_struct(
        self, node, content: str, file_path: str
    ) -> tuple[CodeSymbol | None, list[CodeSymbol]]:
        """Extract Rust struct and its fields"""
        name = self._get_rust_name(node, content)
        if not name:
            return None, []

        source = content[node.start_byte : node.end_byte]
        modifiers = []
        for child in node.children:
            if child.type == "visibility_modifier":
                modifiers.append(content[child.start_byte : child.end_byte])

        fqn = f"{file_path}::{name}"
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        struct_symbol = CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.STRUCT,
            fully_qualified_name=fqn,
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="rust",
            modifiers=modifiers,
        )

        # Extract fields
        fields = []
        for child in node.children:
            if child.type == "field_declaration_list":
                for field in child.children:
                    if field.type == "field_declaration":
                        field_sym = self._extract_rust_field(
                            field, content, file_path, name
                        )
                        if field_sym:
                            fields.append(field_sym)

        return struct_symbol, fields

    def _extract_rust_field(
        self, node, content: str, file_path: str, parent_name: str
    ) -> CodeSymbol | None:
        """Extract a struct field"""
        field_name = None
        field_type = None
        for child in node.children:
            if child.type == "field_identifier":
                field_name = content[child.start_byte : child.end_byte]
            elif child.type == "type_identifier" or child.type.endswith("_type"):
                field_type = content[child.start_byte : child.end_byte]

        if not field_name:
            return None

        fqn = f"{file_path}::{parent_name}::{field_name}"
        symbol_id = self._generate_id(file_path, field_name, node.start_point[0])

        return CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.FIELD,
            fully_qualified_name=fqn,
            simple_name=field_name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                byte_offset=node.start_byte,
            ),
            source_code=content[node.start_byte : node.end_byte],
            language="rust",
            return_type=field_type,
            scope_chain=[parent_name],
        )

    def _extract_rust_enum(
        self, node, content: str, file_path: str
    ) -> CodeSymbol | None:
        """Extract Rust enum"""
        name = self._get_rust_name(node, content)
        if not name:
            return None

        source = content[node.start_byte : node.end_byte]
        fqn = f"{file_path}::{name}"
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        return CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.ENUM,
            fully_qualified_name=fqn,
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="rust",
        )

    def _extract_rust_trait(
        self, node, content: str, file_path: str
    ) -> tuple[CodeSymbol | None, list[CodeSymbol]]:
        """Extract Rust trait and its methods"""
        name = self._get_rust_name(node, content)
        if not name:
            return None, []

        source = content[node.start_byte : node.end_byte]
        fqn = f"{file_path}::{name}"
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        trait_symbol = CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.TRAIT,
            fully_qualified_name=fqn,
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="rust",
        )

        methods = []
        for child in node.children:
            if child.type == "declaration_list":
                for decl in child.children:
                    if (
                        decl.type == "function_signature_item"
                        or decl.type == "function_item"
                    ):
                        method = self._extract_rust_function(
                            decl, content, file_path, [name]
                        )
                        if method:
                            methods.append(method)

        return trait_symbol, methods

    def _extract_rust_impl(
        self, node, content: str, file_path: str, symbols: list[CodeSymbol]
    ):
        """Extract impl block methods"""
        # Get the type being implemented
        impl_type = None
        trait_type = None
        for child in node.children:
            if child.type == "type_identifier":
                if impl_type is None:
                    impl_type = content[child.start_byte : child.end_byte]
                else:
                    trait_type = impl_type
                    impl_type = content[child.start_byte : child.end_byte]
            elif child.type == "generic_type":
                for sub in child.children:
                    if sub.type == "type_identifier":
                        impl_type = content[sub.start_byte : sub.end_byte]
                        break

        scope = [impl_type] if impl_type else []

        for child in node.children:
            if child.type == "declaration_list":
                for decl in child.children:
                    if decl.type == "function_item":
                        method = self._extract_rust_function(
                            decl, content, file_path, scope
                        )
                        if method:
                            symbols.append(method)

    def _extract_rust_params(self, node, content: str) -> list[dict[str, Any]]:
        """Extract function parameters"""
        params = []
        for child in node.children:
            if child.type == "parameter":
                param = {"name": "", "type": None}
                for sub in child.children:
                    if sub.type == "identifier":
                        param["name"] = content[sub.start_byte : sub.end_byte]
                    elif sub.type.endswith("_type") or sub.type == "type_identifier":
                        param["type"] = content[sub.start_byte : sub.end_byte]
                if param["name"] and param["name"] not in (
                    "self",
                    "&self",
                    "&mut self",
                ):
                    params.append(param)
            elif child.type == "self_parameter":
                pass  # Skip self
        return params

    def _extract_rust_relations(
        self, node, content: str, symbols: list[CodeSymbol]
    ) -> list[CodeRelation]:
        """Extract call relationships"""
        relations = []
        symbol_map = {s.simple_name: s for s in symbols}

        def find_calls(n, containing_symbol: CodeSymbol | None = None):
            for sym in symbols:
                if (
                    sym.location.byte_offset <= n.start_byte
                    and n.end_byte
                    <= sym.location.byte_offset + sym.location.byte_length
                ):
                    if sym.symbol_type in (
                        CodeSymbolType.FUNCTION,
                        CodeSymbolType.METHOD,
                    ):
                        containing_symbol = sym
                        break

            if n.type == "call_expression":
                callee_name = None
                for child in n.children:
                    if child.type == "identifier":
                        callee_name = content[child.start_byte : child.end_byte]
                    elif child.type == "field_expression":
                        # Get method name from field access
                        for sub in reversed(child.children):
                            if sub.type == "field_identifier":
                                callee_name = content[sub.start_byte : sub.end_byte]
                                break

                if callee_name and callee_name in symbol_map and containing_symbol:
                    relations.append(
                        CodeRelation(
                            from_symbol_id=containing_symbol.id,
                            to_symbol_id=symbol_map[callee_name].id,
                            relation_type=CodeRelationType.CALLS,
                        )
                    )

            for child in n.children:
                find_calls(child, containing_symbol)

        find_calls(node)
        return relations

    def _get_rust_name(self, node, content: str) -> str | None:
        """Get identifier name from node"""
        for child in node.children:
            if child.type in ("identifier", "type_identifier"):
                return content[child.start_byte : child.end_byte]
        return None

    def _build_rust_signature(
        self, name: str, params: list[dict], return_type: str | None
    ) -> str:
        """Build Rust function signature"""
        param_strs = [f"{p['name']}: {p.get('type', '?')}" for p in params]
        sig = f"fn {name}({', '.join(param_strs)})"
        if return_type:
            sig += f" -> {return_type}"
        return sig

    def _generate_id(self, file_path: str, name: str, line: int) -> str:
        key = f"{file_path}:{name}:{line}"
        return hashlib.sha256(key.encode()).hexdigest()[:16]

    def _parse_with_regex(
        self, content: str, file_path: str, content_hash: str
    ) -> ParsedCode:
        """Fallback regex parsing for Rust"""
        symbols = []
        imports = []
        lines = content.split("\n")

        # Extract use statements
        for line in lines:
            if line.strip().startswith("use "):
                imports.append(line.strip())

        # Simple function extraction
        fn_pattern = re.compile(r"^(\s*)(pub\s+)?(async\s+)?(unsafe\s+)?fn\s+(\w+)")
        for i, line in enumerate(lines):
            match = fn_pattern.match(line)
            if match:
                name = match.group(5)
                modifiers = []
                if match.group(2):
                    modifiers.append("pub")
                if match.group(3):
                    modifiers.append("async")
                if match.group(4):
                    modifiers.append("unsafe")

                symbol_id = self._generate_id(file_path, name, i)
                symbols.append(
                    CodeSymbol(
                        id=symbol_id,
                        symbol_type=CodeSymbolType.FUNCTION,
                        fully_qualified_name=f"{file_path}::{name}",
                        simple_name=name,
                        location=SourceLocation(file_path=file_path, start_line=i + 1),
                        source_code=line,
                        language="rust",
                        modifiers=modifiers,
                    )
                )

        return ParsedCode(
            file_path=file_path,
            language="rust",
            symbols=symbols,
            relations=[],
            imports=imports,
            content_hash=content_hash,
        )


class GoParser(LanguageParser):
    """Go parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("go")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "go"

    @property
    def file_extensions(self) -> list[str]:
        return [".go"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()

        if self._parser is None:
            return self._parse_with_regex(content, file_path, content_hash)

        tree = self._parser.parse(bytes(content, "utf8"))
        symbols = []
        relations = []
        imports = []

        # Extract imports
        for child in tree.root_node.children:
            if child.type == "import_declaration":
                imports.append(content[child.start_byte : child.end_byte])

        # Extract functions, types, methods
        for child in tree.root_node.children:
            if child.type == "function_declaration":
                sym = self._extract_go_function(child, content, file_path)
                if sym:
                    symbols.append(sym)
            elif child.type == "method_declaration":
                sym = self._extract_go_method(child, content, file_path)
                if sym:
                    symbols.append(sym)
            elif child.type == "type_declaration":
                syms = self._extract_go_type(child, content, file_path)
                symbols.extend(syms)

        return ParsedCode(
            file_path=file_path,
            language="go",
            symbols=symbols,
            relations=relations,
            imports=imports,
            content_hash=content_hash,
        )

    def _extract_go_function(
        self, node, content: str, file_path: str
    ) -> CodeSymbol | None:
        """Extract Go function"""
        name = None
        params = []
        return_type = None

        for child in node.children:
            if child.type == "identifier":
                name = content[child.start_byte : child.end_byte]
            elif child.type == "parameter_list":
                params = self._extract_go_params(child, content)
            elif child.type == "result" or child.type == "parameter_list":
                if child.type == "result":
                    return_type = content[child.start_byte : child.end_byte]

        if not name:
            return None

        source = content[node.start_byte : node.end_byte]
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        return CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.FUNCTION,
            fully_qualified_name=f"{file_path}::{name}",
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="go",
            parameters=params,
            return_type=return_type,
        )

    def _extract_go_method(
        self, node, content: str, file_path: str
    ) -> CodeSymbol | None:
        """Extract Go method (function with receiver)"""
        name = None
        receiver_type = None
        params = []

        for child in node.children:
            if child.type == "parameter_list" and receiver_type is None:
                # First param list is receiver
                for sub in child.children:
                    if sub.type == "parameter_declaration":
                        for t in sub.children:
                            if t.type == "type_identifier" or t.type == "pointer_type":
                                receiver_type = content[t.start_byte : t.end_byte]
            elif child.type == "field_identifier":
                name = content[child.start_byte : child.end_byte]
            elif child.type == "parameter_list" and receiver_type is not None:
                params = self._extract_go_params(child, content)

        if not name:
            return None

        source = content[node.start_byte : node.end_byte]
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        return CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.METHOD,
            fully_qualified_name=(
                f"{file_path}::{receiver_type}::{name}"
                if receiver_type
                else f"{file_path}::{name}"
            ),
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="go",
            scope_chain=[receiver_type] if receiver_type else [],
            parameters=params,
        )

    def _extract_go_type(self, node, content: str, file_path: str) -> list[CodeSymbol]:
        """Extract Go type declarations (struct, interface)"""
        symbols = []

        for child in node.children:
            if child.type == "type_spec":
                name = None
                type_kind = None
                for sub in child.children:
                    if sub.type == "type_identifier":
                        name = content[sub.start_byte : sub.end_byte]
                    elif sub.type == "struct_type":
                        type_kind = CodeSymbolType.STRUCT
                    elif sub.type == "interface_type":
                        type_kind = CodeSymbolType.INTERFACE

                if name and type_kind:
                    source = content[child.start_byte : child.end_byte]
                    symbol_id = self._generate_id(file_path, name, child.start_point[0])
                    symbols.append(
                        CodeSymbol(
                            id=symbol_id,
                            symbol_type=type_kind,
                            fully_qualified_name=f"{file_path}::{name}",
                            simple_name=name,
                            location=SourceLocation(
                                file_path=file_path,
                                start_line=child.start_point[0] + 1,
                                end_line=child.end_point[0] + 1,
                                byte_offset=child.start_byte,
                                byte_length=child.end_byte - child.start_byte,
                            ),
                            source_code=source,
                            language="go",
                        )
                    )

        return symbols

    def _extract_go_params(self, node, content: str) -> list[dict[str, Any]]:
        """Extract function parameters"""
        params = []
        for child in node.children:
            if child.type == "parameter_declaration":
                param = {"name": "", "type": None}
                for sub in child.children:
                    if sub.type == "identifier":
                        param["name"] = content[sub.start_byte : sub.end_byte]
                    elif sub.type in (
                        "type_identifier",
                        "pointer_type",
                        "slice_type",
                        "array_type",
                        "map_type",
                        "channel_type",
                    ):
                        param["type"] = content[sub.start_byte : sub.end_byte]
                if param["name"]:
                    params.append(param)
        return params

    def _generate_id(self, file_path: str, name: str, line: int) -> str:
        key = f"{file_path}:{name}:{line}"
        return hashlib.sha256(key.encode()).hexdigest()[:16]

    def _parse_with_regex(
        self, content: str, file_path: str, content_hash: str
    ) -> ParsedCode:
        """Fallback regex parsing"""
        symbols = []
        imports = []
        lines = content.split("\n")

        fn_pattern = re.compile(r"^func\s+(\(\s*\w+\s+\*?\w+\s*\))?\s*(\w+)\s*\(")

        for i, line in enumerate(lines):
            match = fn_pattern.match(line)
            if match:
                receiver = match.group(1)
                name = match.group(2)
                symbol_type = (
                    CodeSymbolType.METHOD if receiver else CodeSymbolType.FUNCTION
                )
                symbol_id = self._generate_id(file_path, name, i)

                symbols.append(
                    CodeSymbol(
                        id=symbol_id,
                        symbol_type=symbol_type,
                        fully_qualified_name=f"{file_path}::{name}",
                        simple_name=name,
                        location=SourceLocation(file_path=file_path, start_line=i + 1),
                        source_code=line,
                        language="go",
                    )
                )

        return ParsedCode(
            file_path=file_path,
            language="go",
            symbols=symbols,
            relations=[],
            imports=imports,
            content_hash=content_hash,
        )


class JavaParser(LanguageParser):
    """Java parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("java")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "java"

    @property
    def file_extensions(self) -> list[str]:
        return [".java"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()

        if self._parser is None:
            return self._parse_with_regex(content, file_path, content_hash)

        tree = self._parser.parse(bytes(content, "utf8"))
        symbols = []
        relations = []
        imports = []

        # Extract imports
        for child in tree.root_node.children:
            if child.type == "import_declaration":
                imports.append(content[child.start_byte : child.end_byte])

        # Extract classes, interfaces, methods
        self._extract_java_items(tree.root_node, content, file_path, [], symbols)

        return ParsedCode(
            file_path=file_path,
            language="java",
            symbols=symbols,
            relations=relations,
            imports=imports,
            content_hash=content_hash,
        )

    def _extract_java_items(
        self,
        node,
        content: str,
        file_path: str,
        scope_chain: list[str],
        symbols: list[CodeSymbol],
    ):
        """Extract Java declarations recursively"""
        for child in node.children:
            if child.type == "class_declaration":
                cls, methods = self._extract_java_class(
                    child, content, file_path, scope_chain
                )
                if cls:
                    symbols.append(cls)
                    symbols.extend(methods)
            elif child.type == "interface_declaration":
                iface, methods = self._extract_java_interface(
                    child, content, file_path, scope_chain
                )
                if iface:
                    symbols.append(iface)
                    symbols.extend(methods)
            elif child.type == "enum_declaration":
                enum = self._extract_java_enum(child, content, file_path, scope_chain)
                if enum:
                    symbols.append(enum)
            elif child.type == "method_declaration":
                method = self._extract_java_method(
                    child, content, file_path, scope_chain
                )
                if method:
                    symbols.append(method)
            elif child.type == "constructor_declaration":
                ctor = self._extract_java_constructor(
                    child, content, file_path, scope_chain
                )
                if ctor:
                    symbols.append(ctor)
            elif child.type == "program" or child.type == "class_body":
                self._extract_java_items(
                    child, content, file_path, scope_chain, symbols
                )

    def _extract_java_class(
        self, node, content: str, file_path: str, scope_chain: list[str]
    ) -> tuple[CodeSymbol | None, list[CodeSymbol]]:
        """Extract Java class"""
        name = None
        modifiers = []
        extends = None
        implements = []

        for child in node.children:
            if child.type == "identifier":
                name = content[child.start_byte : child.end_byte]
            elif child.type == "modifiers":
                for mod in child.children:
                    modifiers.append(content[mod.start_byte : mod.end_byte])
            elif child.type == "superclass":
                for sub in child.children:
                    if sub.type == "type_identifier":
                        extends = content[sub.start_byte : sub.end_byte]
            elif child.type == "super_interfaces":
                for sub in child.children:
                    if sub.type == "type_identifier":
                        implements.append(content[sub.start_byte : sub.end_byte])

        if not name:
            return None, []

        source = content[node.start_byte : node.end_byte]
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        if extends:
            modifiers.append(f"extends({extends})")
        if implements:
            modifiers.append(f"implements({','.join(implements)})")

        class_symbol = CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.CLASS,
            fully_qualified_name="::".join([file_path] + scope_chain + [name]),
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="java",
            modifiers=modifiers,
            scope_chain=scope_chain,
        )

        # Extract methods from class body
        methods = []
        new_scope = scope_chain + [name]
        for child in node.children:
            if child.type == "class_body":
                self._extract_java_items(child, content, file_path, new_scope, methods)

        return class_symbol, methods

    def _extract_java_interface(
        self, node, content: str, file_path: str, scope_chain: list[str]
    ) -> tuple[CodeSymbol | None, list[CodeSymbol]]:
        """Extract Java interface"""
        name = None
        modifiers = []

        for child in node.children:
            if child.type == "identifier":
                name = content[child.start_byte : child.end_byte]
            elif child.type == "modifiers":
                for mod in child.children:
                    modifiers.append(content[mod.start_byte : mod.end_byte])

        if not name:
            return None, []

        source = content[node.start_byte : node.end_byte]
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        iface_symbol = CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.INTERFACE,
            fully_qualified_name="::".join([file_path] + scope_chain + [name]),
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="java",
            modifiers=modifiers,
            scope_chain=scope_chain,
        )

        methods = []
        new_scope = scope_chain + [name]
        for child in node.children:
            if child.type == "interface_body":
                self._extract_java_items(child, content, file_path, new_scope, methods)

        return iface_symbol, methods

    def _extract_java_enum(
        self, node, content: str, file_path: str, scope_chain: list[str]
    ) -> CodeSymbol | None:
        """Extract Java enum"""
        name = None
        for child in node.children:
            if child.type == "identifier":
                name = content[child.start_byte : child.end_byte]
                break

        if not name:
            return None

        source = content[node.start_byte : node.end_byte]
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        return CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.ENUM,
            fully_qualified_name="::".join([file_path] + scope_chain + [name]),
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="java",
            scope_chain=scope_chain,
        )

    def _extract_java_method(
        self, node, content: str, file_path: str, scope_chain: list[str]
    ) -> CodeSymbol | None:
        """Extract Java method"""
        name = None
        modifiers = []
        return_type = None
        params = []

        for child in node.children:
            if child.type == "identifier":
                name = content[child.start_byte : child.end_byte]
            elif child.type == "modifiers":
                for mod in child.children:
                    modifiers.append(content[mod.start_byte : mod.end_byte])
            elif child.type in (
                "type_identifier",
                "void_type",
                "generic_type",
                "array_type",
                "primitive_type",
            ):
                if return_type is None:
                    return_type = content[child.start_byte : child.end_byte]
            elif child.type == "formal_parameters":
                params = self._extract_java_params(child, content)

        if not name:
            return None

        source = content[node.start_byte : node.end_byte]
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        return CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.METHOD,
            fully_qualified_name="::".join([file_path] + scope_chain + [name]),
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="java",
            modifiers=modifiers,
            scope_chain=scope_chain,
            parameters=params,
            return_type=return_type,
        )

    def _extract_java_constructor(
        self, node, content: str, file_path: str, scope_chain: list[str]
    ) -> CodeSymbol | None:
        """Extract Java constructor"""
        name = None
        modifiers = []
        params = []

        for child in node.children:
            if child.type == "identifier":
                name = content[child.start_byte : child.end_byte]
            elif child.type == "modifiers":
                for mod in child.children:
                    modifiers.append(content[mod.start_byte : mod.end_byte])
            elif child.type == "formal_parameters":
                params = self._extract_java_params(child, content)

        if not name:
            return None

        source = content[node.start_byte : node.end_byte]
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        return CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.CONSTRUCTOR,
            fully_qualified_name="::".join([file_path] + scope_chain + [name]),
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="java",
            modifiers=modifiers,
            scope_chain=scope_chain,
            parameters=params,
        )

    def _extract_java_params(self, node, content: str) -> list[dict[str, Any]]:
        """Extract method parameters"""
        params = []
        for child in node.children:
            if child.type == "formal_parameter":
                param = {"name": "", "type": None}
                for sub in child.children:
                    if sub.type == "identifier":
                        param["name"] = content[sub.start_byte : sub.end_byte]
                    elif sub.type in (
                        "type_identifier",
                        "generic_type",
                        "array_type",
                        "primitive_type",
                    ):
                        param["type"] = content[sub.start_byte : sub.end_byte]
                if param["name"]:
                    params.append(param)
        return params

    def _generate_id(self, file_path: str, name: str, line: int) -> str:
        key = f"{file_path}:{name}:{line}"
        return hashlib.sha256(key.encode()).hexdigest()[:16]

    def _parse_with_regex(
        self, content: str, file_path: str, content_hash: str
    ) -> ParsedCode:
        """Fallback regex parsing"""
        symbols = []
        imports = []
        lines = content.split("\n")

        for line in lines:
            if line.strip().startswith("import "):
                imports.append(line.strip())

        class_pattern = re.compile(
            r"^\s*(public\s+|private\s+|protected\s+)?(abstract\s+|final\s+)?"
            r"(class|interface|enum)\s+(\w+)"
        )
        method_pattern = re.compile(
            r"^\s*(public\s+|private\s+|protected\s+)?(static\s+)?"
            r"(\w+)\s+(\w+)\s*\("
        )

        for i, line in enumerate(lines):
            cls_match = class_pattern.match(line)
            if cls_match:
                kind = cls_match.group(3)
                name = cls_match.group(4)
                symbol_type = {
                    "class": CodeSymbolType.CLASS,
                    "interface": CodeSymbolType.INTERFACE,
                    "enum": CodeSymbolType.ENUM,
                }.get(kind, CodeSymbolType.CLASS)

                symbol_id = self._generate_id(file_path, name, i)
                symbols.append(
                    CodeSymbol(
                        id=symbol_id,
                        symbol_type=symbol_type,
                        fully_qualified_name=f"{file_path}::{name}",
                        simple_name=name,
                        location=SourceLocation(file_path=file_path, start_line=i + 1),
                        source_code=line,
                        language="java",
                    )
                )

        return ParsedCode(
            file_path=file_path,
            language="java",
            symbols=symbols,
            relations=[],
            imports=imports,
            content_hash=content_hash,
        )


class CppParser(LanguageParser):
    """C/C++ parser using Tree-sitter"""

    def __init__(self, c_mode: bool = False):
        self._c_mode = c_mode
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            lang = "c" if self._c_mode else "cpp"
            self._parser = get_parser(lang)
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "c" if self._c_mode else "cpp"

    @property
    def file_extensions(self) -> list[str]:
        if self._c_mode:
            return [".c", ".h"]
        return [".cpp", ".cc", ".cxx", ".hpp", ".hxx", ".h"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()

        if self._parser is None:
            return self._parse_with_regex(content, file_path, content_hash)

        tree = self._parser.parse(bytes(content, "utf8"))
        symbols = []
        relations = []
        imports = []

        # Extract includes
        for child in tree.root_node.children:
            if child.type == "preproc_include":
                imports.append(content[child.start_byte : child.end_byte])

        # Extract functions, classes, structs
        self._extract_cpp_items(tree.root_node, content, file_path, [], symbols)

        return ParsedCode(
            file_path=file_path,
            language=self.language,
            symbols=symbols,
            relations=relations,
            imports=imports,
            content_hash=content_hash,
        )

    def _extract_cpp_items(
        self,
        node,
        content: str,
        file_path: str,
        scope_chain: list[str],
        symbols: list[CodeSymbol],
    ):
        """Extract C/C++ items recursively"""
        for child in node.children:
            if child.type == "function_definition":
                sym = self._extract_cpp_function(child, content, file_path, scope_chain)
                if sym:
                    symbols.append(sym)
            elif child.type == "class_specifier":
                cls, members = self._extract_cpp_class(
                    child, content, file_path, scope_chain
                )
                if cls:
                    symbols.append(cls)
                    symbols.extend(members)
            elif child.type == "struct_specifier":
                struct, members = self._extract_cpp_struct(
                    child, content, file_path, scope_chain
                )
                if struct:
                    symbols.append(struct)
                    symbols.extend(members)
            elif child.type == "enum_specifier":
                enum = self._extract_cpp_enum(child, content, file_path)
                if enum:
                    symbols.append(enum)
            elif child.type == "namespace_definition":
                ns_name = self._get_cpp_name(child, content)
                if ns_name:
                    new_scope = scope_chain + [ns_name]
                    self._extract_cpp_items(
                        child, content, file_path, new_scope, symbols
                    )
            elif child.type == "declaration_list" or child.type == "translation_unit":
                self._extract_cpp_items(child, content, file_path, scope_chain, symbols)

    def _extract_cpp_function(
        self, node, content: str, file_path: str, scope_chain: list[str]
    ) -> CodeSymbol | None:
        """Extract C/C++ function"""
        name = None
        return_type = None
        params = []

        for child in node.children:
            if child.type == "function_declarator":
                for sub in child.children:
                    if sub.type == "identifier" or sub.type == "qualified_identifier":
                        name = content[sub.start_byte : sub.end_byte]
                    elif sub.type == "parameter_list":
                        params = self._extract_cpp_params(sub, content)
            elif child.type in (
                "primitive_type",
                "type_identifier",
                "qualified_identifier",
            ):
                if return_type is None:
                    return_type = content[child.start_byte : child.end_byte]

        if not name:
            return None

        source = content[node.start_byte : node.end_byte]
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        return CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.FUNCTION,
            fully_qualified_name="::".join([file_path] + scope_chain + [name]),
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language=self.language,
            scope_chain=scope_chain,
            parameters=params,
            return_type=return_type,
        )

    def _extract_cpp_class(
        self, node, content: str, file_path: str, scope_chain: list[str]
    ) -> tuple[CodeSymbol | None, list[CodeSymbol]]:
        """Extract C++ class"""
        name = self._get_cpp_name(node, content)
        if not name:
            return None, []

        source = content[node.start_byte : node.end_byte]
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        class_symbol = CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.CLASS,
            fully_qualified_name="::".join([file_path] + scope_chain + [name]),
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language=self.language,
            scope_chain=scope_chain,
        )

        members = []
        new_scope = scope_chain + [name]
        for child in node.children:
            if child.type == "field_declaration_list":
                self._extract_cpp_items(child, content, file_path, new_scope, members)

        return class_symbol, members

    def _extract_cpp_struct(
        self, node, content: str, file_path: str, scope_chain: list[str]
    ) -> tuple[CodeSymbol | None, list[CodeSymbol]]:
        """Extract C/C++ struct"""
        name = self._get_cpp_name(node, content)
        if not name:
            return None, []

        source = content[node.start_byte : node.end_byte]
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        struct_symbol = CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.STRUCT,
            fully_qualified_name="::".join([file_path] + scope_chain + [name]),
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language=self.language,
            scope_chain=scope_chain,
        )

        members = []
        return struct_symbol, members

    def _extract_cpp_enum(
        self, node, content: str, file_path: str
    ) -> CodeSymbol | None:
        """Extract C/C++ enum"""
        name = self._get_cpp_name(node, content)
        if not name:
            return None

        source = content[node.start_byte : node.end_byte]
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        return CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.ENUM,
            fully_qualified_name=f"{file_path}::{name}",
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language=self.language,
        )

    def _extract_cpp_params(self, node, content: str) -> list[dict[str, Any]]:
        """Extract function parameters"""
        params = []
        for child in node.children:
            if child.type == "parameter_declaration":
                param = {"name": "", "type": None}
                for sub in child.children:
                    if sub.type == "identifier":
                        param["name"] = content[sub.start_byte : sub.end_byte]
                    elif sub.type in ("primitive_type", "type_identifier"):
                        param["type"] = content[sub.start_byte : sub.end_byte]
                if param["name"]:
                    params.append(param)
        return params

    def _get_cpp_name(self, node, content: str) -> str | None:
        """Get name from node"""
        for child in node.children:
            if child.type in ("identifier", "type_identifier"):
                return content[child.start_byte : child.end_byte]
        return None

    def _generate_id(self, file_path: str, name: str, line: int) -> str:
        key = f"{file_path}:{name}:{line}"
        return hashlib.sha256(key.encode()).hexdigest()[:16]

    def _parse_with_regex(
        self, content: str, file_path: str, content_hash: str
    ) -> ParsedCode:
        """Fallback regex parsing"""
        symbols = []
        imports = []
        lines = content.split("\n")

        for line in lines:
            if line.strip().startswith("#include"):
                imports.append(line.strip())

        return ParsedCode(
            file_path=file_path,
            language=self.language,
            symbols=symbols,
            relations=[],
            imports=imports,
            content_hash=content_hash,
        )


class RubyParser(LanguageParser):
    """Ruby parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("ruby")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "ruby"

    @property
    def file_extensions(self) -> list[str]:
        return [".rb", ".rake", ".gemspec"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()

        if self._parser is None:
            return self._parse_with_regex(content, file_path, content_hash)

        tree = self._parser.parse(bytes(content, "utf8"))
        symbols = []
        relations = []
        imports = []

        self._extract_ruby_items(
            tree.root_node, content, file_path, [], symbols, imports
        )

        return ParsedCode(
            file_path=file_path,
            language="ruby",
            symbols=symbols,
            relations=relations,
            imports=imports,
            content_hash=content_hash,
        )

    def _extract_ruby_items(
        self,
        node,
        content: str,
        file_path: str,
        scope_chain: list[str],
        symbols: list[CodeSymbol],
        imports: list[str],
    ):
        """Extract Ruby items recursively"""
        for child in node.children:
            if child.type == "method":
                sym = self._extract_ruby_method(child, content, file_path, scope_chain)
                if sym:
                    symbols.append(sym)
            elif child.type == "singleton_method":
                sym = self._extract_ruby_method(
                    child, content, file_path, scope_chain, is_class_method=True
                )
                if sym:
                    symbols.append(sym)
            elif child.type == "class":
                cls, methods = self._extract_ruby_class(
                    child, content, file_path, scope_chain
                )
                if cls:
                    symbols.append(cls)
                    symbols.extend(methods)
            elif child.type == "module":
                mod, contents = self._extract_ruby_module(
                    child, content, file_path, scope_chain
                )
                if mod:
                    symbols.append(mod)
                    symbols.extend(contents)
            elif child.type == "call":
                # Check for require/require_relative
                call_text = content[child.start_byte : child.end_byte]
                if call_text.startswith("require"):
                    imports.append(call_text)
            elif hasattr(child, "children"):
                self._extract_ruby_items(
                    child, content, file_path, scope_chain, symbols, imports
                )

    def _extract_ruby_method(
        self,
        node,
        content: str,
        file_path: str,
        scope_chain: list[str],
        is_class_method: bool = False,
    ) -> CodeSymbol | None:
        """Extract Ruby method"""
        name = None
        params = []

        for child in node.children:
            if child.type == "identifier":
                name = content[child.start_byte : child.end_byte]
            elif child.type == "method_parameters":
                params = self._extract_ruby_params(child, content)

        if not name:
            return None

        source = content[node.start_byte : node.end_byte]
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        modifiers = ["class_method"] if is_class_method else []
        if name.startswith("_"):
            modifiers.append("private")

        return CodeSymbol(
            id=symbol_id,
            symbol_type=(
                CodeSymbolType.METHOD if scope_chain else CodeSymbolType.FUNCTION
            ),
            fully_qualified_name="::".join([file_path] + scope_chain + [name]),
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="ruby",
            modifiers=modifiers,
            scope_chain=scope_chain,
            parameters=params,
        )

    def _extract_ruby_class(
        self, node, content: str, file_path: str, scope_chain: list[str]
    ) -> tuple[CodeSymbol | None, list[CodeSymbol]]:
        """Extract Ruby class"""
        name = None
        superclass = None

        for child in node.children:
            if child.type == "constant":
                if name is None:
                    name = content[child.start_byte : child.end_byte]
            elif child.type == "superclass":
                for sub in child.children:
                    if sub.type == "constant":
                        superclass = content[sub.start_byte : sub.end_byte]

        if not name:
            return None, []

        source = content[node.start_byte : node.end_byte]
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        modifiers = []
        if superclass:
            modifiers.append(f"extends({superclass})")

        class_symbol = CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.CLASS,
            fully_qualified_name="::".join([file_path] + scope_chain + [name]),
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="ruby",
            modifiers=modifiers,
            scope_chain=scope_chain,
        )

        methods = []
        new_scope = scope_chain + [name]
        imports_placeholder: list[str] = []
        self._extract_ruby_items(
            node, content, file_path, new_scope, methods, imports_placeholder
        )

        return class_symbol, methods

    def _extract_ruby_module(
        self, node, content: str, file_path: str, scope_chain: list[str]
    ) -> tuple[CodeSymbol | None, list[CodeSymbol]]:
        """Extract Ruby module"""
        name = None

        for child in node.children:
            if child.type == "constant":
                name = content[child.start_byte : child.end_byte]
                break

        if not name:
            return None, []

        source = content[node.start_byte : node.end_byte]
        symbol_id = self._generate_id(file_path, name, node.start_point[0])

        module_symbol = CodeSymbol(
            id=symbol_id,
            symbol_type=CodeSymbolType.MODULE,
            fully_qualified_name="::".join([file_path] + scope_chain + [name]),
            simple_name=name,
            location=SourceLocation(
                file_path=file_path,
                start_line=node.start_point[0] + 1,
                end_line=node.end_point[0] + 1,
                byte_offset=node.start_byte,
                byte_length=node.end_byte - node.start_byte,
            ),
            source_code=source,
            language="ruby",
            scope_chain=scope_chain,
        )

        contents = []
        new_scope = scope_chain + [name]
        imports_placeholder: list[str] = []
        self._extract_ruby_items(
            node, content, file_path, new_scope, contents, imports_placeholder
        )

        return module_symbol, contents

    def _extract_ruby_params(self, node, content: str) -> list[dict[str, Any]]:
        """Extract method parameters"""
        params = []
        for child in node.children:
            if child.type == "identifier":
                params.append({"name": content[child.start_byte : child.end_byte]})
            elif child.type == "optional_parameter":
                for sub in child.children:
                    if sub.type == "identifier":
                        params.append(
                            {
                                "name": content[sub.start_byte : sub.end_byte],
                                "is_optional": True,
                            }
                        )
            elif child.type == "splat_parameter":
                for sub in child.children:
                    if sub.type == "identifier":
                        params.append(
                            {
                                "name": f"*{content[sub.start_byte:sub.end_byte]}",
                                "is_variadic": True,
                            }
                        )
            elif child.type == "hash_splat_parameter":
                for sub in child.children:
                    if sub.type == "identifier":
                        params.append(
                            {
                                "name": f"**{content[sub.start_byte:sub.end_byte]}",
                                "is_variadic": True,
                            }
                        )
        return params

    def _generate_id(self, file_path: str, name: str, line: int) -> str:
        key = f"{file_path}:{name}:{line}"
        return hashlib.sha256(key.encode()).hexdigest()[:16]

    def _parse_with_regex(
        self, content: str, file_path: str, content_hash: str
    ) -> ParsedCode:
        """Fallback regex parsing"""
        symbols = []
        imports = []
        lines = content.split("\n")

        for line in lines:
            stripped = line.strip()
            if stripped.startswith("require ") or stripped.startswith(
                "require_relative "
            ):
                imports.append(stripped)

        return ParsedCode(
            file_path=file_path,
            language="ruby",
            symbols=symbols,
            relations=[],
            imports=imports,
            content_hash=content_hash,
        )


class CSharpParser(LanguageParser):
    """C# parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("c_sharp")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "csharp"

    @property
    def file_extensions(self) -> list[str]:
        return [".cs"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()
        # Similar structure to JavaParser
        return ParsedCode(
            file_path=file_path,
            language="csharp",
            symbols=[],
            relations=[],
            imports=[],
            content_hash=content_hash,
        )


class PhpParser(LanguageParser):
    """PHP parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("php")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "php"

    @property
    def file_extensions(self) -> list[str]:
        return [".php", ".phtml"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()
        return ParsedCode(
            file_path=file_path,
            language="php",
            symbols=[],
            relations=[],
            imports=[],
            content_hash=content_hash,
        )


class SwiftParser(LanguageParser):
    """Swift parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("swift")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "swift"

    @property
    def file_extensions(self) -> list[str]:
        return [".swift"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()
        return ParsedCode(
            file_path=file_path,
            language="swift",
            symbols=[],
            relations=[],
            imports=[],
            content_hash=content_hash,
        )


class KotlinParser(LanguageParser):
    """Kotlin parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("kotlin")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "kotlin"

    @property
    def file_extensions(self) -> list[str]:
        return [".kt", ".kts"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()
        return ParsedCode(
            file_path=file_path,
            language="kotlin",
            symbols=[],
            relations=[],
            imports=[],
            content_hash=content_hash,
        )


class ScalaParser(LanguageParser):
    """Scala parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("scala")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "scala"

    @property
    def file_extensions(self) -> list[str]:
        return [".scala", ".sc"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()
        return ParsedCode(
            file_path=file_path,
            language="scala",
            symbols=[],
            relations=[],
            imports=[],
            content_hash=content_hash,
        )


class BashParser(LanguageParser):
    """Bash/Shell parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("bash")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "bash"

    @property
    def file_extensions(self) -> list[str]:
        return [".sh", ".bash", ".zsh", ".ksh"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()

        if self._parser is None:
            return self._parse_with_regex(content, file_path, content_hash)

        tree = self._parser.parse(bytes(content, "utf8"))
        symbols = []
        relations = []
        imports = []

        # Extract function definitions
        self._extract_bash_items(tree.root_node, content, file_path, symbols)

        # Extract source/. commands as imports
        for child in tree.root_node.children:
            if child.type == "command":
                cmd_text = content[child.start_byte : child.end_byte]
                if cmd_text.startswith("source ") or cmd_text.startswith(". "):
                    imports.append(cmd_text)

        return ParsedCode(
            file_path=file_path,
            language="bash",
            symbols=symbols,
            relations=relations,
            imports=imports,
            content_hash=content_hash,
        )

    def _extract_bash_items(
        self, node, content: str, file_path: str, symbols: list[CodeSymbol]
    ):
        """Extract bash functions"""
        for child in node.children:
            if child.type == "function_definition":
                name = None
                for sub in child.children:
                    if sub.type == "word":
                        name = content[sub.start_byte : sub.end_byte]
                        break

                if name:
                    source = content[child.start_byte : child.end_byte]
                    symbol_id = self._generate_id(file_path, name, child.start_point[0])

                    symbols.append(
                        CodeSymbol(
                            id=symbol_id,
                            symbol_type=CodeSymbolType.FUNCTION,
                            fully_qualified_name=f"{file_path}::{name}",
                            simple_name=name,
                            location=SourceLocation(
                                file_path=file_path,
                                start_line=child.start_point[0] + 1,
                                end_line=child.end_point[0] + 1,
                                byte_offset=child.start_byte,
                                byte_length=child.end_byte - child.start_byte,
                            ),
                            source_code=source,
                            language="bash",
                        )
                    )

            if hasattr(child, "children"):
                self._extract_bash_items(child, content, file_path, symbols)

    def _generate_id(self, file_path: str, name: str, line: int) -> str:
        key = f"{file_path}:{name}:{line}"
        return hashlib.sha256(key.encode()).hexdigest()[:16]

    def _parse_with_regex(
        self, content: str, file_path: str, content_hash: str
    ) -> ParsedCode:
        """Fallback regex parsing for bash"""
        symbols = []
        imports = []
        lines = content.split("\n")

        fn_pattern = re.compile(r"^(\w+)\s*\(\s*\)\s*\{")
        fn_pattern2 = re.compile(r"^function\s+(\w+)")

        for i, line in enumerate(lines):
            match = fn_pattern.match(line.strip()) or fn_pattern2.match(line.strip())
            if match:
                name = match.group(1)
                symbol_id = self._generate_id(file_path, name, i)
                symbols.append(
                    CodeSymbol(
                        id=symbol_id,
                        symbol_type=CodeSymbolType.FUNCTION,
                        fully_qualified_name=f"{file_path}::{name}",
                        simple_name=name,
                        location=SourceLocation(file_path=file_path, start_line=i + 1),
                        source_code=line,
                        language="bash",
                    )
                )

            # Check for source commands
            stripped = line.strip()
            if stripped.startswith("source ") or stripped.startswith(". "):
                imports.append(stripped)

        return ParsedCode(
            file_path=file_path,
            language="bash",
            symbols=symbols,
            relations=[],
            imports=imports,
            content_hash=content_hash,
        )


class SqlParser(LanguageParser):
    """SQL parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("sql")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "sql"

    @property
    def file_extensions(self) -> list[str]:
        return [".sql", ".psql", ".mysql"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()

        if self._parser is None:
            return self._parse_with_regex(content, file_path, content_hash)

        tree = self._parser.parse(bytes(content, "utf8"))
        symbols = []

        # Extract stored procedures, functions, views, tables
        self._extract_sql_items(tree.root_node, content, file_path, symbols)

        return ParsedCode(
            file_path=file_path,
            language="sql",
            symbols=symbols,
            relations=[],
            imports=[],
            content_hash=content_hash,
        )

    def _extract_sql_items(
        self, node, content: str, file_path: str, symbols: list[CodeSymbol]
    ):
        """Extract SQL objects"""
        for child in node.children:
            if child.type in (
                "create_function_statement",
                "create_procedure_statement",
            ):
                name = self._get_sql_name(child, content)
                if name:
                    source = content[child.start_byte : child.end_byte]
                    symbol_id = self._generate_id(file_path, name, child.start_point[0])
                    symbols.append(
                        CodeSymbol(
                            id=symbol_id,
                            symbol_type=CodeSymbolType.FUNCTION,
                            fully_qualified_name=f"{file_path}::{name}",
                            simple_name=name,
                            location=SourceLocation(
                                file_path=file_path,
                                start_line=child.start_point[0] + 1,
                                end_line=child.end_point[0] + 1,
                                byte_offset=child.start_byte,
                            ),
                            source_code=source,
                            language="sql",
                        )
                    )
            elif child.type == "create_table_statement":
                name = self._get_sql_name(child, content)
                if name:
                    source = content[child.start_byte : child.end_byte]
                    symbol_id = self._generate_id(file_path, name, child.start_point[0])
                    symbols.append(
                        CodeSymbol(
                            id=symbol_id,
                            symbol_type=CodeSymbolType.STRUCT,  # Tables as structs
                            fully_qualified_name=f"{file_path}::{name}",
                            simple_name=name,
                            location=SourceLocation(
                                file_path=file_path,
                                start_line=child.start_point[0] + 1,
                                byte_offset=child.start_byte,
                            ),
                            source_code=source,
                            language="sql",
                        )
                    )
            elif child.type == "create_view_statement":
                name = self._get_sql_name(child, content)
                if name:
                    source = content[child.start_byte : child.end_byte]
                    symbol_id = self._generate_id(file_path, name, child.start_point[0])
                    symbols.append(
                        CodeSymbol(
                            id=symbol_id,
                            symbol_type=CodeSymbolType.TYPE_ALIAS,  # Views as type aliases
                            fully_qualified_name=f"{file_path}::{name}",
                            simple_name=name,
                            location=SourceLocation(
                                file_path=file_path,
                                start_line=child.start_point[0] + 1,
                                byte_offset=child.start_byte,
                            ),
                            source_code=source,
                            language="sql",
                        )
                    )

            if hasattr(child, "children"):
                self._extract_sql_items(child, content, file_path, symbols)

    def _get_sql_name(self, node, content: str) -> str | None:
        """Get object name from SQL node"""
        for child in node.children:
            if child.type in ("identifier", "object_reference"):
                return content[child.start_byte : child.end_byte]
        return None

    def _generate_id(self, file_path: str, name: str, line: int) -> str:
        key = f"{file_path}:{name}:{line}"
        return hashlib.sha256(key.encode()).hexdigest()[:16]

    def _parse_with_regex(
        self, content: str, file_path: str, content_hash: str
    ) -> ParsedCode:
        """Fallback regex parsing"""
        symbols = []

        # Common SQL patterns
        create_pattern = re.compile(
            r"CREATE\s+(OR\s+REPLACE\s+)?(FUNCTION|PROCEDURE|TABLE|VIEW)\s+(\w+)",
            re.IGNORECASE,
        )

        for match in create_pattern.finditer(content):
            obj_type = match.group(2).upper()
            name = match.group(3)

            symbol_type = {
                "FUNCTION": CodeSymbolType.FUNCTION,
                "PROCEDURE": CodeSymbolType.FUNCTION,
                "TABLE": CodeSymbolType.STRUCT,
                "VIEW": CodeSymbolType.TYPE_ALIAS,
            }.get(obj_type, CodeSymbolType.VARIABLE)

            line_num = content[: match.start()].count("\n") + 1
            symbol_id = self._generate_id(file_path, name, line_num)

            symbols.append(
                CodeSymbol(
                    id=symbol_id,
                    symbol_type=symbol_type,
                    fully_qualified_name=f"{file_path}::{name}",
                    simple_name=name,
                    location=SourceLocation(file_path=file_path, start_line=line_num),
                    source_code=match.group(0),
                    language="sql",
                )
            )

        return ParsedCode(
            file_path=file_path,
            language="sql",
            symbols=symbols,
            relations=[],
            imports=[],
            content_hash=content_hash,
        )


class YamlParser(LanguageParser):
    """YAML parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("yaml")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "yaml"

    @property
    def file_extensions(self) -> list[str]:
        return [".yaml", ".yml"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()

        if self._parser is None:
            return self._parse_with_regex(content, file_path, content_hash)

        tree = self._parser.parse(bytes(content, "utf8"))
        symbols = []

        # Extract top-level keys as "sections"
        self._extract_yaml_items(tree.root_node, content, file_path, [], symbols)

        return ParsedCode(
            file_path=file_path,
            language="yaml",
            symbols=symbols,
            relations=[],
            imports=[],
            content_hash=content_hash,
        )

    def _extract_yaml_items(
        self,
        node,
        content: str,
        file_path: str,
        path: list[str],
        symbols: list[CodeSymbol],
    ):
        """Extract YAML keys as symbols"""
        for child in node.children:
            if child.type == "block_mapping_pair":
                key_node = None
                for sub in child.children:
                    if sub.type == "flow_node" or sub.type.endswith("_key"):
                        key_node = sub
                        break

                if key_node:
                    key_name = content[key_node.start_byte : key_node.end_byte].strip()
                    if key_name and not key_name.startswith("#"):
                        current_path = path + [key_name]
                        fqn = ".".join(current_path)

                        # Only extract top-level or important nested keys
                        if len(current_path) <= 2:
                            source = content[child.start_byte : child.end_byte]
                            symbol_id = self._generate_id(
                                file_path, fqn, child.start_point[0]
                            )

                            symbols.append(
                                CodeSymbol(
                                    id=symbol_id,
                                    symbol_type=CodeSymbolType.PROPERTY,
                                    fully_qualified_name=f"{file_path}::{fqn}",
                                    simple_name=key_name,
                                    location=SourceLocation(
                                        file_path=file_path,
                                        start_line=child.start_point[0] + 1,
                                        byte_offset=child.start_byte,
                                    ),
                                    source_code=source[:500],  # Limit source size
                                    language="yaml",
                                    scope_chain=path,
                                )
                            )

                        # Recurse for nested mappings
                        self._extract_yaml_items(
                            child, content, file_path, current_path, symbols
                        )

            elif hasattr(child, "children"):
                self._extract_yaml_items(child, content, file_path, path, symbols)

    def _generate_id(self, file_path: str, name: str, line: int) -> str:
        key = f"{file_path}:{name}:{line}"
        return hashlib.sha256(key.encode()).hexdigest()[:16]

    def _parse_with_regex(
        self, content: str, file_path: str, content_hash: str
    ) -> ParsedCode:
        """Fallback regex parsing"""
        symbols = []
        lines = content.split("\n")

        # Extract top-level keys (no leading whitespace)
        key_pattern = re.compile(r"^(\w[\w\-_]*)\s*:")

        for i, line in enumerate(lines):
            match = key_pattern.match(line)
            if match:
                name = match.group(1)
                symbol_id = self._generate_id(file_path, name, i)

                symbols.append(
                    CodeSymbol(
                        id=symbol_id,
                        symbol_type=CodeSymbolType.PROPERTY,
                        fully_qualified_name=f"{file_path}::{name}",
                        simple_name=name,
                        location=SourceLocation(file_path=file_path, start_line=i + 1),
                        source_code=line,
                        language="yaml",
                    )
                )

        return ParsedCode(
            file_path=file_path,
            language="yaml",
            symbols=symbols,
            relations=[],
            imports=[],
            content_hash=content_hash,
        )


class JsonParser(LanguageParser):
    """JSON parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("json")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "json"

    @property
    def file_extensions(self) -> list[str]:
        return [".json", ".jsonc", ".json5"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()

        if self._parser is None:
            return self._parse_with_regex(content, file_path, content_hash)

        tree = self._parser.parse(bytes(content, "utf8"))
        symbols = []

        # Extract top-level keys
        self._extract_json_items(tree.root_node, content, file_path, [], symbols)

        return ParsedCode(
            file_path=file_path,
            language="json",
            symbols=symbols,
            relations=[],
            imports=[],
            content_hash=content_hash,
        )

    def _extract_json_items(
        self,
        node,
        content: str,
        file_path: str,
        path: list[str],
        symbols: list[CodeSymbol],
    ):
        """Extract JSON keys as symbols"""
        for child in node.children:
            if child.type == "pair":
                key_node = None
                for sub in child.children:
                    if sub.type == "string":
                        key_node = sub
                        break

                if key_node:
                    # Remove quotes
                    key_name = content[key_node.start_byte : key_node.end_byte].strip(
                        "\"'"
                    )
                    current_path = path + [key_name]
                    fqn = ".".join(current_path)

                    # Only extract top-level keys for JSON
                    if len(current_path) <= 1:
                        source = content[child.start_byte : child.end_byte]
                        symbol_id = self._generate_id(
                            file_path, fqn, child.start_point[0]
                        )

                        symbols.append(
                            CodeSymbol(
                                id=symbol_id,
                                symbol_type=CodeSymbolType.PROPERTY,
                                fully_qualified_name=f"{file_path}::{fqn}",
                                simple_name=key_name,
                                location=SourceLocation(
                                    file_path=file_path,
                                    start_line=child.start_point[0] + 1,
                                    byte_offset=child.start_byte,
                                ),
                                source_code=source[:500],
                                language="json",
                                scope_chain=path,
                            )
                        )

            if hasattr(child, "children"):
                self._extract_json_items(child, content, file_path, path, symbols)

    def _generate_id(self, file_path: str, name: str, line: int) -> str:
        key = f"{file_path}:{name}:{line}"
        return hashlib.sha256(key.encode()).hexdigest()[:16]

    def _parse_with_regex(
        self, content: str, file_path: str, content_hash: str
    ) -> ParsedCode:
        """Fallback regex parsing - extract top-level keys"""
        symbols = []

        # Simple pattern for top-level keys
        key_pattern = re.compile(r'^\s*"(\w+)"\s*:')

        for i, line in enumerate(content.split("\n")):
            match = key_pattern.match(line)
            if match:
                name = match.group(1)
                symbol_id = self._generate_id(file_path, name, i)
                symbols.append(
                    CodeSymbol(
                        id=symbol_id,
                        symbol_type=CodeSymbolType.PROPERTY,
                        fully_qualified_name=f"{file_path}::{name}",
                        simple_name=name,
                        location=SourceLocation(file_path=file_path, start_line=i + 1),
                        source_code=line,
                        language="json",
                    )
                )

        return ParsedCode(
            file_path=file_path,
            language="json",
            symbols=symbols,
            relations=[],
            imports=[],
            content_hash=content_hash,
        )


class XmlParser(LanguageParser):
    """XML/HTML parser (minimal support for config files)"""

    def __init__(self):
        self._parser = None
        # XML parsing is complex, use regex fallback

    @property
    def language(self) -> str:
        return "xml"

    @property
    def file_extensions(self) -> list[str]:
        return [".xml", ".xhtml", ".xsd", ".xsl", ".pom", ".csproj", ".fsproj"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()
        return self._parse_with_regex(content, file_path, content_hash)

    def _parse_with_regex(
        self, content: str, file_path: str, content_hash: str
    ) -> ParsedCode:
        """Extract key XML elements"""
        symbols = []

        # Extract root element and key children
        root_pattern = re.compile(r"<(\w+)[^>]*>")

        match = root_pattern.search(content)
        if match:
            root_name = match.group(1)
            symbol_id = self._generate_id(file_path, root_name, 0)
            symbols.append(
                CodeSymbol(
                    id=symbol_id,
                    symbol_type=CodeSymbolType.MODULE,
                    fully_qualified_name=f"{file_path}::{root_name}",
                    simple_name=root_name,
                    location=SourceLocation(file_path=file_path, start_line=1),
                    source_code=content[:200],  # Just the beginning
                    language="xml",
                )
            )

        return ParsedCode(
            file_path=file_path,
            language="xml",
            symbols=symbols,
            relations=[],
            imports=[],
            content_hash=content_hash,
        )

    def _generate_id(self, file_path: str, name: str, line: int) -> str:
        key = f"{file_path}:{name}:{line}"
        return hashlib.sha256(key.encode()).hexdigest()[:16]


class PerlParser(LanguageParser):
    """Perl parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("perl")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "perl"

    @property
    def file_extensions(self) -> list[str]:
        return [".pl", ".pm", ".t"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()

        if self._parser is None:
            return self._parse_with_regex(content, file_path, content_hash)

        tree = self._parser.parse(bytes(content, "utf8"))
        symbols = []
        imports = []

        self._extract_perl_items(tree.root_node, content, file_path, symbols, imports)

        return ParsedCode(
            file_path=file_path,
            language="perl",
            symbols=symbols,
            relations=[],
            imports=imports,
            content_hash=content_hash,
        )

    def _extract_perl_items(
        self,
        node,
        content: str,
        file_path: str,
        symbols: list[CodeSymbol],
        imports: list[str],
    ):
        """Extract Perl subroutines and packages"""
        for child in node.children:
            if child.type == "subroutine_declaration":
                name = None
                for sub in child.children:
                    if sub.type == "identifier":
                        name = content[sub.start_byte : sub.end_byte]
                        break

                if name:
                    source = content[child.start_byte : child.end_byte]
                    symbol_id = self._generate_id(file_path, name, child.start_point[0])
                    symbols.append(
                        CodeSymbol(
                            id=symbol_id,
                            symbol_type=CodeSymbolType.FUNCTION,
                            fully_qualified_name=f"{file_path}::{name}",
                            simple_name=name,
                            location=SourceLocation(
                                file_path=file_path,
                                start_line=child.start_point[0] + 1,
                                end_line=child.end_point[0] + 1,
                                byte_offset=child.start_byte,
                            ),
                            source_code=source,
                            language="perl",
                        )
                    )

            elif child.type == "package_statement":
                name = None
                for sub in child.children:
                    if sub.type == "package":
                        name = content[sub.start_byte : sub.end_byte]
                        break

                if name:
                    source = content[child.start_byte : child.end_byte]
                    symbol_id = self._generate_id(file_path, name, child.start_point[0])
                    symbols.append(
                        CodeSymbol(
                            id=symbol_id,
                            symbol_type=CodeSymbolType.PACKAGE,
                            fully_qualified_name=f"{file_path}::{name}",
                            simple_name=name,
                            location=SourceLocation(
                                file_path=file_path,
                                start_line=child.start_point[0] + 1,
                                byte_offset=child.start_byte,
                            ),
                            source_code=source,
                            language="perl",
                        )
                    )

            elif child.type == "use_statement":
                imports.append(content[child.start_byte : child.end_byte])

            if hasattr(child, "children"):
                self._extract_perl_items(child, content, file_path, symbols, imports)

    def _generate_id(self, file_path: str, name: str, line: int) -> str:
        key = f"{file_path}:{name}:{line}"
        return hashlib.sha256(key.encode()).hexdigest()[:16]

    def _parse_with_regex(
        self, content: str, file_path: str, content_hash: str
    ) -> ParsedCode:
        """Fallback regex parsing"""
        symbols = []
        imports = []
        lines = content.split("\n")

        sub_pattern = re.compile(r"^\s*sub\s+(\w+)")
        pkg_pattern = re.compile(r"^\s*package\s+([\w:]+)")
        use_pattern = re.compile(r"^\s*use\s+")

        for i, line in enumerate(lines):
            sub_match = sub_pattern.match(line)
            if sub_match:
                name = sub_match.group(1)
                symbol_id = self._generate_id(file_path, name, i)
                symbols.append(
                    CodeSymbol(
                        id=symbol_id,
                        symbol_type=CodeSymbolType.FUNCTION,
                        fully_qualified_name=f"{file_path}::{name}",
                        simple_name=name,
                        location=SourceLocation(file_path=file_path, start_line=i + 1),
                        source_code=line,
                        language="perl",
                    )
                )

            pkg_match = pkg_pattern.match(line)
            if pkg_match:
                name = pkg_match.group(1)
                symbol_id = self._generate_id(file_path, name, i)
                symbols.append(
                    CodeSymbol(
                        id=symbol_id,
                        symbol_type=CodeSymbolType.PACKAGE,
                        fully_qualified_name=f"{file_path}::{name}",
                        simple_name=name,
                        location=SourceLocation(file_path=file_path, start_line=i + 1),
                        source_code=line,
                        language="perl",
                    )
                )

            if use_pattern.match(line):
                imports.append(line.strip())

        return ParsedCode(
            file_path=file_path,
            language="perl",
            symbols=symbols,
            relations=[],
            imports=imports,
            content_hash=content_hash,
        )


class LuaParser(LanguageParser):
    """Lua parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("lua")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "lua"

    @property
    def file_extensions(self) -> list[str]:
        return [".lua"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()
        # Placeholder - would implement full Lua parsing
        return ParsedCode(
            file_path=file_path,
            language="lua",
            symbols=[],
            relations=[],
            imports=[],
            content_hash=content_hash,
        )


class HaskellParser(LanguageParser):
    """Haskell parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("haskell")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "haskell"

    @property
    def file_extensions(self) -> list[str]:
        return [".hs", ".lhs"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()
        return ParsedCode(
            file_path=file_path,
            language="haskell",
            symbols=[],
            relations=[],
            imports=[],
            content_hash=content_hash,
        )


class ElixirParser(LanguageParser):
    """Elixir parser using Tree-sitter"""

    def __init__(self):
        self._parser = None
        self._init_parser()

    def _init_parser(self):
        try:
            from tree_sitter_language_pack import get_parser

            self._parser = get_parser("elixir")
        except (ImportError, OSError):
            # Grammar not installed / native module unavailable -> regex fallback.
            self._parser = None
        except Exception:
            # Unexpected failure: surface it (do not silently swallow), then fall back.
            import logging

            logging.warning(
                "Unexpected tree-sitter parser init failure for %s; "
                "using regex fallback",
                getattr(self, "language", "unknown"),
                exc_info=True,
            )
            self._parser = None

    @property
    def language(self) -> str:
        return "elixir"

    @property
    def file_extensions(self) -> list[str]:
        return [".ex", ".exs"]

    def parse(self, content: str, file_path: str) -> ParsedCode:
        content_hash = hashlib.sha256(content.encode()).hexdigest()
        return ParsedCode(
            file_path=file_path,
            language="elixir",
            symbols=[],
            relations=[],
            imports=[],
            content_hash=content_hash,
        )


# ============================================================================
# Language Parser Registry (Pluggable Architecture)
# ============================================================================

# Registry of available parsers - easily extensible
LANGUAGE_PARSERS: dict[str, type] = {
    # Primary languages with full implementation
    "python": PythonParser,
    "rust": RustParser,
    "go": GoParser,
    "java": JavaParser,
    "javascript": JavaScriptParser,
    "typescript": lambda: JavaScriptParser(typescript=True),
    # C/C++ family
    "c": lambda: CppParser(c_mode=True),
    "cpp": CppParser,
    "csharp": CSharpParser,
    # Dynamic/scripting languages
    "ruby": RubyParser,
    "php": PhpParser,
    "perl": PerlParser,
    "lua": LuaParser,
    # JVM languages
    "kotlin": KotlinParser,
    "scala": ScalaParser,
    # Apple ecosystem
    "swift": SwiftParser,
    # Shell/scripting
    "bash": BashParser,
    # Data/config formats
    "sql": SqlParser,
    "yaml": YamlParser,
    "json": JsonParser,
    "xml": XmlParser,
    # Functional languages
    "haskell": HaskellParser,
    "elixir": ElixirParser,
}

# Extension to language mapping for auto-detection
EXTENSION_TO_LANGUAGE: dict[str, str] = {
    # Python
    ".py": "python",
    ".pyi": "python",
    ".pyx": "python",
    # JavaScript/TypeScript
    ".js": "javascript",
    ".jsx": "javascript",
    ".mjs": "javascript",
    ".cjs": "javascript",
    ".ts": "typescript",
    ".tsx": "typescript",
    # Rust
    ".rs": "rust",
    # Go
    ".go": "go",
    # Java
    ".java": "java",
    # C/C++
    ".c": "c",
    ".h": "c",
    ".cpp": "cpp",
    ".cc": "cpp",
    ".cxx": "cpp",
    ".hpp": "cpp",
    ".hxx": "cpp",
    ".hh": "cpp",
    # C#
    ".cs": "csharp",
    # Ruby
    ".rb": "ruby",
    ".rake": "ruby",
    ".gemspec": "ruby",
    # PHP
    ".php": "php",
    ".phtml": "php",
    # Swift
    ".swift": "swift",
    # Kotlin
    ".kt": "kotlin",
    ".kts": "kotlin",
    # Scala
    ".scala": "scala",
    ".sc": "scala",
    # Shell/Bash
    ".sh": "bash",
    ".bash": "bash",
    ".zsh": "bash",
    ".ksh": "bash",
    ".fish": "bash",
    # Perl
    ".pl": "perl",
    ".pm": "perl",
    ".t": "perl",
    # Lua
    ".lua": "lua",
    # SQL
    ".sql": "sql",
    ".psql": "sql",
    ".mysql": "sql",
    # Data formats
    ".yaml": "yaml",
    ".yml": "yaml",
    ".json": "json",
    ".jsonc": "json",
    ".json5": "json",
    # XML/Markup
    ".xml": "xml",
    ".xhtml": "xml",
    ".xsd": "xml",
    ".xsl": "xml",
    ".pom": "xml",
    ".csproj": "xml",
    ".fsproj": "xml",
    # Functional languages
    ".hs": "haskell",
    ".lhs": "haskell",
    ".ex": "elixir",
    ".exs": "elixir",
}


def register_language_parser(language: str, parser_class: type) -> None:
    """
    Register a new language parser.

    This allows extending the system with custom parsers at runtime.

    Args:
        language: Language identifier (e.g., "python", "rust")
        parser_class: Parser class implementing LanguageParser interface

    Example:
        class MyLangParser(LanguageParser):
            ...

        register_language_parser("mylang", MyLangParser)
    """
    LANGUAGE_PARSERS[language] = parser_class


def register_file_extension(extension: str, language: str) -> None:
    """
    Register a file extension mapping.

    Args:
        extension: File extension including dot (e.g., ".myext")
        language: Language identifier
    """
    EXTENSION_TO_LANGUAGE[extension.lower()] = language


def get_supported_languages() -> list[str]:
    """Get list of supported languages"""
    return list(LANGUAGE_PARSERS.keys())


def get_supported_extensions() -> list[str]:
    """Get list of supported file extensions"""
    return list(EXTENSION_TO_LANGUAGE.keys())


class CodeChunkingStrategy(ChunkingStrategyInterface):
    """
    AST-aware code chunking strategy.

    Unlike text-based strategies, this:
    - Produces chunks aligned to code structure
    - Extracts symbols and relationships
    - Preserves semantic context
    """

    def __init__(self, config: CodeChunkingConfig | None = None):
        _warn_code_chunker_deprecated()
        self.config = config or CodeChunkingConfig()
        # Lazily-instantiated parsers, keyed by language. We do NOT eagerly
        # build all ~23 tree-sitter parsers here: loading every grammar to chunk
        # a single file is wasteful. Each language's parser is instantiated on
        # first use (see `_get_parser`) and cached for subsequent calls.
        self._parsers: dict[str, LanguageParser] = {}
        # `None` sentinel marks a language we already tried and failed to load,
        # so we don't repeatedly retry a broken/unavailable grammar.
        self._failed_languages: set[str] = set()
        # Set of languages this strategy is allowed to parse (config-scoped).
        self._allowed_languages: set[str] = set(
            self.config.languages or LANGUAGE_PARSERS.keys()
        )

    def _get_parser(self, language: str | None) -> LanguageParser | None:
        """Return the parser for `language`, instantiating it on first use.

        Returns None if the language is unknown, not in the configured set, or
        previously failed to initialize.
        """
        if (
            language is None
            or language not in LANGUAGE_PARSERS
            or language not in self._allowed_languages
            or language in self._failed_languages
        ):
            return None
        parser = self._parsers.get(language)
        if parser is not None:
            return parser
        parser_class = LANGUAGE_PARSERS[language]
        try:
            parser = parser_class() if callable(parser_class) else parser_class
        except (AttributeError, ImportError, OSError) as e:
            # Grammar not available in tree-sitter-language-pack -> fall back.
            import logging

            logging.debug(f"Skipping parser for {language}: {e}")
            self._failed_languages.add(language)
            return None
        self._parsers[language] = parser
        return parser

    def chunk(
        self, text: str, source_id: str, metadata: dict[str, Any] | None = None
    ) -> list[TextChunk]:
        """
        Chunk code into semantic units.

        Args:
            text: Source code content
            source_id: File path or identifier
            metadata: Additional metadata (should include 'language' if known)

        Returns:
            List of TextChunk objects, one per symbol
        """
        metadata = metadata or {}
        language = metadata.get("language") or self._detect_language(source_id)

        # TD-CG2: when the shared package is installed, it is the source of truth.
        if _victor_codegraph is not None:
            return self._chunk_via_victor_codegraph(text, source_id, language, metadata)

        parser = self._get_parser(language)
        if parser is None:
            # Fall back to semantic text chunking
            return self._fallback_chunk(text, source_id, metadata)

        parsed = parser.parse(text, source_id)

        chunks = []
        for i, symbol in enumerate(parsed.symbols):
            chunk_id = f"{source_id}#{symbol.simple_name}#{i}"

            chunk_metadata = {
                **metadata,
                "chunking_strategy": "code",
                "chunk_type": "code",
                "symbol_id": symbol.id,
                "symbol_type": symbol.symbol_type.name,
                "fully_qualified_name": symbol.fully_qualified_name,
                "simple_name": symbol.simple_name,
                "language": symbol.language,
                "file_path": symbol.location.file_path,
                "start_line": symbol.location.start_line,
                "end_line": symbol.location.end_line,
                "documentation": symbol.documentation,
                "signature": symbol.signature,
                "modifiers": symbol.modifiers,
                "scope_chain": symbol.scope_chain,
                "parameters": symbol.parameters,
                "return_type": symbol.return_type,
                "complexity": symbol.complexity,
            }

            # Include relations in metadata
            symbol_relations = [
                {
                    "to": r.to_symbol_id,
                    "type": r.relation_type.name,
                    "confidence": r.confidence,
                }
                for r in parsed.relations
                if r.from_symbol_id == symbol.id
            ]
            if symbol_relations:
                chunk_metadata["relations"] = symbol_relations

            chunks.append(
                TextChunk(
                    text=symbol.source_code,
                    start_pos=symbol.location.byte_offset,
                    end_pos=symbol.location.byte_offset + symbol.location.byte_length,
                    chunk_id=chunk_id,
                    metadata=chunk_metadata,
                )
            )

        return chunks

    def _chunk_via_victor_codegraph(
        self,
        text: str,
        source_id: str,
        language: str | None,
        metadata: dict[str, Any],
    ) -> list[TextChunk]:
        """Delegate to the shared ``victor-codegraph`` package and adapt to TextChunk.

        TD-CG2: this is the single source of truth when the ``codegraph`` extra is
        installed. ``victor_codegraph`` already applies size-capping and a real JS/TS
        parser (gaps this legacy module had), so its ``CodeChunk`` output is adapted
        one-to-one into the SDK's ``TextChunk`` shape.
        """
        cfg = _victor_codegraph.ChunkConfig(
            max_chunk_tokens=max(1, int(self.config.chunk_size / 3.5)),
            chunk_overlap_tokens=max(0, int(self.config.chunk_overlap / 3.5)),
            languages=self.config.languages,
            include_private=self.config.include_private,
            extract_relations=self.config.extract_relations,
        )
        out: list[TextChunk] = []
        for c in _victor_codegraph.chunk(
            text, language=language, file_path=source_id, config=cfg
        ):
            meta = {**metadata, **c.metadata}
            meta.setdefault("chunking_strategy", "code")
            meta.setdefault("chunk_type", "code")
            meta["source"] = "victor_codegraph"
            out.append(
                TextChunk(
                    text=c.text,
                    start_pos=c.start_pos,
                    end_pos=c.end_pos,
                    chunk_id=c.chunk_id,
                    metadata=meta,
                )
            )
        return out

    def _detect_language(self, file_path: str) -> str | None:
        """Detect language from file extension using global registry"""
        ext = os.path.splitext(file_path)[1].lower()
        return EXTENSION_TO_LANGUAGE.get(ext)

    def _fallback_chunk(
        self, text: str, source_id: str, metadata: dict[str, Any]
    ) -> list[TextChunk]:
        """Fallback to simple text chunking when parser not available"""
        from .semantic import SemanticStrategy

        fallback_config = ChunkingConfig(
            chunk_size=self.config.chunk_size,
            chunk_overlap=self.config.chunk_overlap,
            preserve_code_blocks=True,
        )
        fallback = SemanticStrategy(fallback_config)
        chunks = fallback.chunk(text, source_id, metadata)
        # Override metadata to indicate this is code chunking (fallback mode)
        for chunk in chunks:
            chunk.metadata["chunking_strategy"] = "code"
            chunk.metadata["chunk_type"] = "code_fallback"
        return chunks


# Convenience function
def create_code_chunker(
    languages: list[str] | None = None, **kwargs
) -> CodeChunkingStrategy:
    """
    Create a code-aware chunker.

    Args:
        languages: Languages to support (None = all available)
        **kwargs: Additional CodeChunkingConfig options

    Returns:
        Configured CodeChunkingStrategy
    """
    config = CodeChunkingConfig(languages=languages, **kwargs)
    return CodeChunkingStrategy(config)
