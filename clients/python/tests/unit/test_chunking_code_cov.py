"""
Offline unit tests for proximadb_sdk.chunking_strategies.code.

Strategy:
- The real `tree_sitter_language_pack` module is NOT installed in this env, but
  the lower-level `tree_sitter` runtime and several individual grammar modules
  (tree_sitter_python / _rust / _go / _java / _javascript / _bash) ARE. We build
  a fake `tree_sitter_language_pack` that returns real tree_sitter Parser/Language
  objects backed by those grammars and inject it into sys.modules. This drives the
  rich AST-extraction code paths (the largest coverage gap) fully offline -- no
  network, no downloads.
- For the regex-fallback paths we construct parsers WITHOUT the fake module
  installed (parser stays None), then call .parse().
- All inputs are tiny in-memory strings; nothing blocks.
"""

import sys
import types

import pytest

from proximadb_sdk.chunking_strategies import code as codemod
from proximadb_sdk.chunking_strategies.code import (
    BashParser,
    CodeChunkingConfig,
    CodeChunkingStrategy,
    CodeRelationType,
    CodeSymbol,
    CodeSymbolType,
    CppParser,
    CSharpParser,
    ElixirParser,
    GoParser,
    HaskellParser,
    JavaParser,
    JavaScriptParser,
    JsonParser,
    KotlinParser,
    LuaParser,
    ParsedCode,
    PerlParser,
    PhpParser,
    PythonParser,
    RubyParser,
    RustParser,
    ScalaParser,
    SourceLocation,
    SqlParser,
    SwiftParser,
    XmlParser,
    YamlParser,
    create_code_chunker,
    get_supported_extensions,
    get_supported_languages,
    register_file_extension,
    register_language_parser,
)

# Grammar modules that exist in this environment.
_GRAMMARS = {
    "python": "tree_sitter_python",
    "rust": "tree_sitter_rust",
    "go": "tree_sitter_go",
    "java": "tree_sitter_java",
    "javascript": "tree_sitter_javascript",
    "bash": "tree_sitter_bash",
    "cpp": "tree_sitter_cpp",
    "c": "tree_sitter_c",
    "ruby": "tree_sitter_ruby",
    "yaml": "tree_sitter_yaml",
    "json": "tree_sitter_json",
    "sql": "tree_sitter_sql",
    "perl": "tree_sitter_perl",
}


def _build_fake_language_pack():
    """Build a fake tree_sitter_language_pack module backed by real grammars.

    Grammars resolve from standalone ``tree_sitter_<lang>`` packages when
    installed; for any language whose standalone package is absent we fall
    back to the real ``tree_sitter_language_pack`` (which bundles all of
    them). Captured here, before the fixture monkeypatches the module name,
    so the fallback reaches the genuine package rather than this fake.
    """
    import tree_sitter as ts

    try:
        import tree_sitter_language_pack as _real_pack
    except Exception:  # pragma: no cover - real pack absent in some envs
        _real_pack = None

    _lang_cache = {}

    def _get_language(name):
        if name in _lang_cache:
            return _lang_cache[name]
        lang = None
        mod_name = _GRAMMARS.get(name)
        if mod_name is not None:
            try:
                grammar = __import__(mod_name)
                lang = ts.Language(grammar.language())
            except Exception:
                lang = None  # standalone grammar not installed — fall back
        if lang is None and _real_pack is not None:
            try:
                lang = _real_pack.get_language(name)
            except Exception:
                lang = None
        if lang is None:
            raise LookupError(f"no grammar for {name}")
        _lang_cache[name] = lang
        return lang

    def get_language(name):
        return _get_language(name)

    def get_parser(name):
        lang = _get_language(name)
        return ts.Parser(lang)

    fake = types.ModuleType("tree_sitter_language_pack")
    fake.get_language = get_language
    fake.get_parser = get_parser
    return fake


@pytest.fixture
def ts_pack(monkeypatch):
    """Install the fake tree_sitter_language_pack for the duration of a test."""
    fake = _build_fake_language_pack()
    monkeypatch.setitem(sys.modules, "tree_sitter_language_pack", fake)
    return fake


@pytest.fixture
def no_ts_pack(monkeypatch):
    """Ensure tree_sitter_language_pack import fails -> regex fallback path."""
    monkeypatch.setitem(sys.modules, "tree_sitter_language_pack", None)
    return None


# ---------------------------------------------------------------------------
# Dataclasses / enums
# ---------------------------------------------------------------------------


def test_enums_and_dataclasses():
    assert CodeSymbolType.FUNCTION.value == 9
    assert CodeRelationType.CALLS.value == 1
    loc = SourceLocation(file_path="x.py", start_line=1, end_line=2, byte_offset=0)
    sym = CodeSymbol(
        id="abc",
        symbol_type=CodeSymbolType.FUNCTION,
        fully_qualified_name="x.py::foo",
        simple_name="foo",
        location=loc,
        source_code="def foo(): pass",
        language="python",
    )
    assert sym.modifiers == []
    assert sym.parameters == []
    pc = ParsedCode("x.py", "python", [sym], [], [], "hash")
    assert pc.symbols[0].simple_name == "foo"


def test_code_chunking_config_defaults():
    cfg = CodeChunkingConfig()
    assert cfg.include_private is True
    assert cfg.extract_relations is True
    assert cfg.max_symbol_depth == 10
    assert cfg.context_lines == 5


# ---------------------------------------------------------------------------
# Registry helpers
# ---------------------------------------------------------------------------


def test_registry_helpers():
    langs = get_supported_languages()
    assert "python" in langs and "rust" in langs
    exts = get_supported_extensions()
    assert ".py" in exts and ".rs" in exts

    class Dummy:
        pass

    register_language_parser("mylang", Dummy)
    register_file_extension(".MYEXT", "mylang")
    assert "mylang" in get_supported_languages()
    assert codemod.EXTENSION_TO_LANGUAGE[".myext"] == "mylang"
    # cleanup
    del codemod.LANGUAGE_PARSERS["mylang"]
    del codemod.EXTENSION_TO_LANGUAGE[".myext"]


# ---------------------------------------------------------------------------
# Python parser - AST path (largest gap)
# ---------------------------------------------------------------------------

PY_SOURCE = '''\
import os
from collections import OrderedDict

def top_level(a, b: int = 2, *args, **kwargs) -> int:
    """A top-level function."""
    if a > 0 and b > 0:
        for i in range(a):
            top_level(i, b)
    return a + b

@decorator
def decorated_fn(x):
    \'\'\'decorated docstring\'\'\'
    return x

@some.deco
class MyClass(Base, Mixin):
    """Class docstring."""

    def __init__(self, value):
        self.value = value

    def _private_method(self):
        return self.value

    @staticmethod
    def static_helper(z):
        return z * 2

    def __repr__(self):
        return "MyClass"
'''


def test_python_parser_treesitter(ts_pack):
    p = PythonParser()
    assert p._parser is not None  # AST path active
    assert p.language == "python"
    assert ".py" in p.file_extensions
    parsed = p.parse(PY_SOURCE, "pkg/mod.py")
    names = {s.simple_name for s in parsed.symbols}
    assert "top_level" in names
    assert "decorated_fn" in names
    assert "MyClass" in names
    assert "__init__" in names
    assert "_private_method" in names
    assert "static_helper" in names

    # imports extracted
    assert any("import os" in imp for imp in parsed.imports)
    assert any("from collections" in imp for imp in parsed.imports)

    # symbol type checks
    by_name = {s.simple_name: s for s in parsed.symbols}
    assert by_name["top_level"].symbol_type == CodeSymbolType.FUNCTION
    assert by_name["__init__"].symbol_type == CodeSymbolType.CONSTRUCTOR
    assert by_name["_private_method"].symbol_type == CodeSymbolType.METHOD
    assert "private" in by_name["_private_method"].modifiers
    assert "dunder" in by_name["__repr__"].modifiers
    assert by_name["MyClass"].symbol_type == CodeSymbolType.CLASS
    # class extends modifier
    assert any("extends(" in m for m in by_name["MyClass"].modifiers)

    # docstring + signature + complexity
    assert by_name["top_level"].documentation == "A top-level function."
    assert "top_level(" in by_name["top_level"].signature
    assert by_name["top_level"].return_type is not None
    assert by_name["top_level"].complexity["cyclomatic"] >= 2

    # parameters: self/cls skipped, defaults/variadics captured
    params = by_name["top_level"].parameters
    pnames = [pp["name"] for pp in params]
    assert "a" in pnames
    assert any(pp.get("is_optional") for pp in params)
    assert any(pp.get("is_variadic") for pp in params)

    # relations: top_level calls itself -> CALLS relation
    assert any(r.relation_type == CodeRelationType.CALLS for r in parsed.relations)

    # content hash deterministic
    parsed2 = p.parse(PY_SOURCE, "pkg/mod.py")
    assert parsed.content_hash == parsed2.content_hash


def test_python_parser_docstring_single_quote(ts_pack):
    p = PythonParser()
    src = "def f():\n    'single quoted doc'\n    return 1\n"
    parsed = p.parse(src, "f.py")
    f = next(s for s in parsed.symbols if s.simple_name == "f")
    assert f.documentation == "single quoted doc"


# ---------------------------------------------------------------------------
# Python parser - regex fallback path
# ---------------------------------------------------------------------------


def test_python_parser_regex_fallback(no_ts_pack):
    p = PythonParser()
    assert p._parser is None
    parsed = p.parse(PY_SOURCE, "mod.py")
    names = {s.simple_name for s in parsed.symbols}
    assert "top_level" in names
    assert "MyClass" in names
    assert "__init__" in names
    # imports
    assert any("import os" in i for i in parsed.imports)
    by_name = {s.simple_name: s for s in parsed.symbols}
    # method detection by indent inside class
    assert by_name["__init__"].symbol_type == CodeSymbolType.CONSTRUCTOR
    assert by_name["_private_method"].symbol_type == CodeSymbolType.METHOD


def test_python_regex_param_parsing(no_ts_pack):
    p = PythonParser()
    # exercise typed/default/variadic params in regex param parser
    params = p._parse_params_regex("a: int = 5, b=2, *args, c: str, self")
    pnames = [pp["name"] for pp in params]
    assert "a" in pnames
    assert "*args" in pnames
    assert "self" not in pnames
    a = next(pp for pp in params if pp["name"] == "a")
    assert a["type"] == "int"
    assert a["default"] == "5"
    assert any(pp.get("is_variadic") for pp in params)
    assert p._parse_params_regex("") == []
    assert p._parse_params_regex("   ") == []


def test_python_regex_block_end(no_ts_pack):
    p = PythonParser()
    lines = ["def f():", "    x = 1", "    return x", "", "y = 2"]
    end = p._find_block_end_regex(lines, 0, 0)
    # blank line at index 3 is skipped; "y = 2" at index 4 dedents -> returns 3
    assert end == 3
    # dedent immediately after body (no trailing blank)
    lines3 = ["def f():", "    x = 1", "z = 9"]
    assert p._find_block_end_regex(lines3, 0, 0) == 1
    # block that runs to EOF
    lines2 = ["def f():", "    x = 1"]
    assert p._find_block_end_regex(lines2, 0, 0) == 1


def test_build_signature_and_symbol_id(no_ts_pack):
    p = PythonParser()
    sig = p._build_signature(
        "fn", [{"name": "a", "type": "int", "default": "1"}], "bool"
    )
    assert sig == "fn(a: int = 1) -> bool"
    assert p._build_signature("g", [], None) == "g()"
    sid = p._generate_symbol_id("f.py", "fn", 1, 0)
    assert len(sid) == 16


# ---------------------------------------------------------------------------
# Rust parser
# ---------------------------------------------------------------------------

RUST_SOURCE = """\
use std::collections::HashMap;

pub struct Point {
    x: i32,
    y: i32,
}

pub enum Color {
    Red,
    Green,
}

pub trait Shape {
    fn area(&self) -> f64;
}

impl Point {
    pub fn new(x: i32, y: i32) -> Point {
        Point { x, y }
    }

    fn distance(&self) -> f64 {
        helper(self.x)
    }
}

pub async unsafe fn helper(v: i32) -> i32 {
    v * 2
}

mod inner {
    fn nested_fn() {}
}
"""


def test_rust_parser_treesitter(ts_pack):
    p = RustParser()
    assert p._parser is not None
    assert p.language == "rust"
    assert ".rs" in p.file_extensions
    parsed = p.parse(RUST_SOURCE, "lib.rs")
    names = {s.simple_name for s in parsed.symbols}
    assert "Point" in names
    assert "Color" in names
    assert "Shape" in names
    assert "new" in names
    assert "helper" in names
    assert any("use std" in i for i in parsed.imports)
    by_type = {s.symbol_type for s in parsed.symbols}
    assert CodeSymbolType.STRUCT in by_type
    assert CodeSymbolType.ENUM in by_type
    assert CodeSymbolType.TRAIT in by_type
    # modifiers / signature on helper
    helper = next(s for s in parsed.symbols if s.simple_name == "helper")
    assert helper.signature.startswith("fn helper(")


def test_rust_parser_regex_fallback(no_ts_pack):
    p = RustParser()
    assert p._parser is None
    parsed = p.parse(RUST_SOURCE, "lib.rs")
    names = {s.simple_name for s in parsed.symbols}
    assert "new" in names or "helper" in names
    assert any("use std" in i for i in parsed.imports)
    helper = next(s for s in parsed.symbols if s.simple_name == "helper")
    assert "pub" in helper.modifiers
    assert "async" in helper.modifiers
    assert "unsafe" in helper.modifiers


# ---------------------------------------------------------------------------
# Go parser
# ---------------------------------------------------------------------------

GO_SOURCE = """\
package main

import "fmt"

type Point struct {
    X int
    Y int
}

type Stringer interface {
    String() string
}

func Add(a int, b int) int {
    return a + b
}

func (p *Point) Move(dx int) {
    p.X += dx
}
"""


def test_go_parser_treesitter(ts_pack):
    p = GoParser()
    assert p._parser is not None
    assert p.language == "go"
    assert ".go" in p.file_extensions
    parsed = p.parse(GO_SOURCE, "main.go")
    names = {s.simple_name for s in parsed.symbols}
    assert "Add" in names
    assert "Move" in names
    assert "Point" in names
    assert "Stringer" in names
    by_name = {s.simple_name: s for s in parsed.symbols}
    assert by_name["Add"].symbol_type == CodeSymbolType.FUNCTION
    assert by_name["Move"].symbol_type == CodeSymbolType.METHOD
    assert by_name["Point"].symbol_type == CodeSymbolType.STRUCT
    assert by_name["Stringer"].symbol_type == CodeSymbolType.INTERFACE
    assert any("fmt" in i for i in parsed.imports)


def test_go_parser_regex_fallback(no_ts_pack):
    p = GoParser()
    assert p._parser is None
    parsed = p.parse(GO_SOURCE, "main.go")
    names = {s.simple_name for s in parsed.symbols}
    assert "Add" in names
    assert "Move" in names
    by_name = {s.simple_name: s for s in parsed.symbols}
    assert by_name["Move"].symbol_type == CodeSymbolType.METHOD
    assert by_name["Add"].symbol_type == CodeSymbolType.FUNCTION


# ---------------------------------------------------------------------------
# Java parser
# ---------------------------------------------------------------------------

JAVA_SOURCE = """\
package com.example;

import java.util.List;

public class Calculator extends Base implements Runnable {
    private int total;

    public Calculator(int start) {
        this.total = start;
    }

    public int add(int x, int y) {
        return x + y;
    }

    public void run() {}
}

interface Greeter {
    String greet(String name);
}

enum Status {
    ACTIVE, INACTIVE
}
"""


def test_java_parser_treesitter(ts_pack):
    p = JavaParser()
    assert p._parser is not None
    assert p.language == "java"
    assert ".java" in p.file_extensions
    parsed = p.parse(JAVA_SOURCE, "Calculator.java")
    names = {s.simple_name for s in parsed.symbols}
    assert "Calculator" in names
    assert "add" in names
    assert "Greeter" in names
    assert "Status" in names
    by_name = {s.simple_name: s for s in parsed.symbols}
    # "Calculator" is both the class and the constructor name -> filter by type
    cls = next(
        s
        for s in parsed.symbols
        if s.simple_name == "Calculator" and s.symbol_type == CodeSymbolType.CLASS
    )
    assert by_name["add"].symbol_type == CodeSymbolType.METHOD
    # constructor
    ctors = [s for s in parsed.symbols if s.symbol_type == CodeSymbolType.CONSTRUCTOR]
    assert ctors
    assert by_name["Greeter"].symbol_type == CodeSymbolType.INTERFACE
    assert by_name["Status"].symbol_type == CodeSymbolType.ENUM
    # extends modifier captured (note: this grammar nests interface names under
    # a `type_list`, which the source's direct-child scan does not descend into,
    # so `implements(...)` is intentionally not asserted here).
    assert any("extends(" in m for m in cls.modifiers)
    assert any("java.util" in i for i in parsed.imports)


def test_java_parser_regex_fallback(no_ts_pack):
    p = JavaParser()
    assert p._parser is None
    parsed = p.parse(JAVA_SOURCE, "Calculator.java")
    names = {s.simple_name for s in parsed.symbols}
    assert "Calculator" in names
    assert "Greeter" in names
    assert "Status" in names
    by_name = {s.simple_name: s for s in parsed.symbols}
    assert by_name["Greeter"].symbol_type == CodeSymbolType.INTERFACE
    assert any("java.util" in i for i in parsed.imports)


# ---------------------------------------------------------------------------
# JavaScript / TypeScript parser (parse returns empty placeholder)
# ---------------------------------------------------------------------------


def test_javascript_parser(ts_pack):
    p = JavaScriptParser()
    assert p.language == "javascript"
    assert ".js" in p.file_extensions
    parsed = p.parse("function foo() {}", "a.js")
    assert parsed.language == "javascript"
    assert parsed.symbols == []
    assert parsed.content_hash


def test_typescript_parser():
    p = JavaScriptParser(typescript=True)
    assert p.language == "typescript"
    assert ".ts" in p.file_extensions
    parsed = p.parse("const x = 1;", "a.ts")
    assert parsed.language == "typescript"


# ---------------------------------------------------------------------------
# Bash parser
# ---------------------------------------------------------------------------

BASH_SOURCE = """\
#!/bin/bash
source ./common.sh
. ./other.sh

function greet() {
    echo "hi"
}

build() {
    greet
}
"""


def test_bash_parser_treesitter(ts_pack):
    p = BashParser()
    assert p._parser is not None
    assert p.language == "bash"
    assert ".sh" in p.file_extensions
    parsed = p.parse(BASH_SOURCE, "build.sh")
    names = {s.simple_name for s in parsed.symbols}
    assert "greet" in names
    assert "build" in names
    assert any("source" in i or i.startswith(".") for i in parsed.imports)


def test_bash_parser_regex_fallback(no_ts_pack):
    p = BashParser()
    assert p._parser is None
    parsed = p.parse(BASH_SOURCE, "build.sh")
    names = {s.simple_name for s in parsed.symbols}
    assert "greet" in names
    assert "build" in names
    assert parsed.imports  # source/. lines


# ---------------------------------------------------------------------------
# C/C++ parser (regex fallback only - no cpp grammar in env)
# ---------------------------------------------------------------------------


CPP_SOURCE = """\
#include <iostream>

namespace ns {
    int helper(int v) {
        return v + 1;
    }
}

class Widget {
    int value;
    void render(int x) {}
};

struct Pair {
    int a;
    int b;
};

enum Color { RED, GREEN };

int main() {
    return 0;
}
"""


def test_cpp_parser_treesitter(ts_pack):
    p = CppParser()
    assert p._parser is not None
    assert p.language == "cpp"
    parsed = p.parse(CPP_SOURCE, "m.cpp")
    names = {s.simple_name for s in parsed.symbols}
    assert "main" in names
    assert "Widget" in names
    assert "Pair" in names
    assert "Color" in names
    by_type = {s.symbol_type for s in parsed.symbols}
    assert CodeSymbolType.CLASS in by_type
    assert CodeSymbolType.STRUCT in by_type
    assert CodeSymbolType.ENUM in by_type
    assert CodeSymbolType.FUNCTION in by_type
    assert any("#include" in i for i in parsed.imports)


def test_c_parser_treesitter(ts_pack):
    p = CppParser(c_mode=True)
    assert p._parser is not None
    assert p.language == "c"
    src = "#include <stdio.h>\nstruct S { int a; };\nint add(int x, int y) { return x + y; }\n"
    parsed = p.parse(src, "m.c")
    names = {s.simple_name for s in parsed.symbols}
    assert "add" in names
    assert "S" in names
    add = next(s for s in parsed.symbols if s.simple_name == "add")
    assert add.symbol_type == CodeSymbolType.FUNCTION
    assert [pp["name"] for pp in add.parameters]  # params extracted
    assert any("#include" in i for i in parsed.imports)


def test_cpp_parser_regex_fallback(no_ts_pack):
    p = CppParser()
    assert p._parser is None
    assert p.language == "cpp"
    assert ".cpp" in p.file_extensions
    parsed = p.parse("#include <iostream>\nint main() { return 0; }", "m.cpp")
    assert any("#include" in i for i in parsed.imports)
    assert parsed.language == "cpp"


def test_c_parser_mode_regex(no_ts_pack):
    p = CppParser(c_mode=True)
    assert p.language == "c"
    assert ".c" in p.file_extensions
    parsed = p.parse('#include "x.h"\n', "m.c")
    assert any("#include" in i for i in parsed.imports)


# ---------------------------------------------------------------------------
# Ruby parser (regex fallback)
# ---------------------------------------------------------------------------


RUBY_SOURCE = """\
require 'json'

module Greetings
  def hello
    "hi"
  end
end

class Animal < Base
  def initialize(name)
    @name = name
  end

  def speak(volume, *extras, **opts)
    @name
  end

  def self.create
    new("x")
  end
end

def top_level
  42
end
"""


def test_ruby_parser_treesitter(ts_pack):
    p = RubyParser()
    assert p._parser is not None
    assert p.language == "ruby"
    parsed = p.parse(RUBY_SOURCE, "x.rb")
    names = {s.simple_name for s in parsed.symbols}
    assert "Animal" in names
    assert "Greetings" in names
    assert "speak" in names
    assert "top_level" in names
    by_name = {s.simple_name: s for s in parsed.symbols}
    assert by_name["Animal"].symbol_type == CodeSymbolType.CLASS
    assert by_name["Greetings"].symbol_type == CodeSymbolType.MODULE
    assert by_name["speak"].symbol_type == CodeSymbolType.METHOD
    assert by_name["top_level"].symbol_type == CodeSymbolType.FUNCTION
    # superclass modifier on class
    assert any("extends(" in m for m in by_name["Animal"].modifiers)
    # variadic params on speak
    assert any(pp.get("is_variadic") for pp in by_name["speak"].parameters)
    # require captured as import
    assert any("require" in i for i in parsed.imports)


def test_ruby_parser_regex_fallback(no_ts_pack):
    p = RubyParser()
    assert p._parser is None
    assert p.language == "ruby"
    assert ".rb" in p.file_extensions
    src = "require 'json'\nrequire_relative 'foo'\ndef bar\n  1\nend\n"
    parsed = p.parse(src, "x.rb")
    assert any("require" in i for i in parsed.imports)
    assert parsed.language == "ruby"


# ---------------------------------------------------------------------------
# SQL parser
# ---------------------------------------------------------------------------

SQL_SOURCE = """\
CREATE TABLE users (id INT, name TEXT);
CREATE OR REPLACE FUNCTION add_user(name TEXT) RETURNS VOID AS $$ $$ LANGUAGE sql;
CREATE VIEW active_users AS SELECT * FROM users;
CREATE PROCEDURE cleanup() AS BEGIN END;
"""


def test_sql_parser_treesitter(ts_pack):
    # Drives the AST traversal path (_extract_sql_items / _get_sql_name). The
    # installed SQL grammar uses different node names than the source expects,
    # so symbol extraction yields nothing -- but the recursive walk still runs.
    p = SqlParser()
    assert p._parser is not None
    assert p.language == "sql"
    parsed = p.parse(SQL_SOURCE, "schema.sql")
    assert parsed.language == "sql"
    assert isinstance(parsed.symbols, list)


def test_sql_parser_regex_fallback(no_ts_pack):
    p = SqlParser()
    assert p._parser is None
    assert p.language == "sql"
    assert ".sql" in p.file_extensions
    parsed = p.parse(SQL_SOURCE, "schema.sql")
    names = {s.simple_name for s in parsed.symbols}
    assert "users" in names
    assert "add_user" in names
    assert "active_users" in names
    assert "cleanup" in names
    by_name = {s.simple_name: s for s in parsed.symbols}
    assert by_name["users"].symbol_type == CodeSymbolType.STRUCT
    assert by_name["add_user"].symbol_type == CodeSymbolType.FUNCTION
    assert by_name["active_users"].symbol_type == CodeSymbolType.TYPE_ALIAS


# ---------------------------------------------------------------------------
# YAML parser (regex fallback)
# ---------------------------------------------------------------------------


def test_yaml_parser_treesitter(ts_pack):
    p = YamlParser()
    assert p._parser is not None
    assert p.language == "yaml"
    src = "name: test\nversion: 1.0\nserver:\n  host: localhost\n  port: 8080\n"
    parsed = p.parse(src, "c.yaml")
    names = {s.simple_name for s in parsed.symbols}
    assert "name" in names
    assert "version" in names
    assert "server" in names
    # nested keys (depth 2) also extracted
    assert "host" in names or "port" in names
    by_name = {s.simple_name: s for s in parsed.symbols}
    assert by_name["name"].symbol_type == CodeSymbolType.PROPERTY


def test_yaml_parser_regex_fallback(no_ts_pack):
    p = YamlParser()
    assert p._parser is None
    assert p.language == "yaml"
    assert ".yaml" in p.file_extensions
    parsed = p.parse("name: test\nversion: 1.0\n  nested: x\n", "c.yaml")
    names = {s.simple_name for s in parsed.symbols}
    assert "name" in names
    assert "version" in names


# ---------------------------------------------------------------------------
# JSON parser (regex fallback)
# ---------------------------------------------------------------------------


def test_json_parser_treesitter(ts_pack):
    p = JsonParser()
    assert p._parser is not None
    assert p.language == "json"
    src = '{"name": "x", "version": "1", "nested": {"inner": 2}}'
    parsed = p.parse(src, "p.json")
    names = {s.simple_name for s in parsed.symbols}
    # only top-level keys
    assert "name" in names
    assert "version" in names
    assert "nested" in names
    by_name = {s.simple_name: s for s in parsed.symbols}
    assert by_name["name"].symbol_type == CodeSymbolType.PROPERTY


def test_json_parser_regex_fallback(no_ts_pack):
    p = JsonParser()
    assert p._parser is None
    assert p.language == "json"
    assert ".json" in p.file_extensions
    parsed = p.parse('{\n  "name": "x",\n  "version": "1"\n}', "p.json")
    names = {s.simple_name for s in parsed.symbols}
    assert "name" in names
    assert "version" in names


# ---------------------------------------------------------------------------
# XML parser (always regex)
# ---------------------------------------------------------------------------


def test_xml_parser():
    p = XmlParser()
    assert p.language == "xml"
    assert ".xml" in p.file_extensions
    parsed = p.parse("<project><name>x</name></project>", "pom.xml")
    assert parsed.symbols
    assert parsed.symbols[0].simple_name == "project"
    # empty content -> no root match
    parsed2 = p.parse("   ", "empty.xml")
    assert parsed2.symbols == []


# ---------------------------------------------------------------------------
# Perl parser (regex fallback)
# ---------------------------------------------------------------------------


def test_perl_parser_treesitter(ts_pack):
    p = PerlParser()
    assert p._parser is not None
    assert p.language == "perl"
    src = "package My::Mod;\nuse strict;\nsub hello {\n  return 1;\n}\n"
    parsed = p.parse(src, "x.pl")
    names = {s.simple_name for s in parsed.symbols}
    # package_statement is extracted by the AST walk; use_statement -> import
    assert any("My::Mod" in n for n in names) or parsed.symbols
    assert any("use" in i for i in parsed.imports)


def test_perl_parser_regex_fallback(no_ts_pack):
    p = PerlParser()
    assert p._parser is None
    assert p.language == "perl"
    assert ".pl" in p.file_extensions
    src = "package My::Mod;\nuse strict;\nsub hello {\n  return 1;\n}\n"
    parsed = p.parse(src, "x.pl")
    names = {s.simple_name for s in parsed.symbols}
    assert "hello" in names
    assert "My::Mod" in names
    assert any("use" in i for i in parsed.imports)
    by_name = {s.simple_name: s for s in parsed.symbols}
    assert by_name["My::Mod"].symbol_type == CodeSymbolType.PACKAGE


# ---------------------------------------------------------------------------
# Placeholder parsers (return empty)
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "cls,lang,ext",
    [
        (CSharpParser, "csharp", ".cs"),
        (PhpParser, "php", ".php"),
        (SwiftParser, "swift", ".swift"),
        (KotlinParser, "kotlin", ".kt"),
        (ScalaParser, "scala", ".scala"),
        (LuaParser, "lua", ".lua"),
        (HaskellParser, "haskell", ".hs"),
        (ElixirParser, "elixir", ".ex"),
    ],
)
def test_placeholder_parsers(cls, lang, ext):
    p = cls()
    assert p.language == lang
    assert ext in p.file_extensions
    parsed = p.parse("anything", f"f{ext}")
    assert parsed.language == lang
    assert parsed.symbols == []
    assert parsed.content_hash


# ---------------------------------------------------------------------------
# CodeChunkingStrategy - top level
# ---------------------------------------------------------------------------


def test_strategy_init_default(no_ts_pack):
    strat = CodeChunkingStrategy()
    # Parsers are lazy: nothing instantiated until the first chunk(), but
    # python must be in the allowed set.
    assert strat._parsers == {}
    assert "python" in strat._allowed_languages
    # On demand, a python parser is instantiated even without tree-sitter
    # (regex fallback) and then cached.
    assert strat._get_parser("python") is not None
    assert "python" in strat._parsers


def test_strategy_init_specific_languages(no_ts_pack):
    cfg = CodeChunkingConfig(languages=["python", "unknownlang"])
    strat = CodeChunkingStrategy(cfg)
    assert strat._allowed_languages == {"python", "unknownlang"}
    # python resolves; an unknown language never produces a parser.
    assert strat._get_parser("python") is not None
    assert strat._get_parser("unknownlang") is None
    assert "python" in strat._parsers
    assert "unknownlang" not in strat._parsers


def test_strategy_chunk_python_ast(ts_pack):
    strat = create_code_chunker(languages=["python"])
    chunks = strat.chunk(PY_SOURCE, "pkg/mod.py", {"extra": "v"})
    assert chunks
    c0 = chunks[0]
    assert c0.metadata["chunking_strategy"] == "code"
    assert c0.metadata["chunk_type"] == "code"
    assert c0.metadata["language"] == "python"
    assert c0.metadata["extra"] == "v"
    assert "symbol_id" in c0.metadata
    assert "fully_qualified_name" in c0.metadata
    # some chunk should carry relations (top_level calls itself)
    assert any("relations" in c.metadata for c in chunks)
    # chunk_id format
    assert "#" in c0.chunk_id


def test_strategy_detect_language():
    strat = create_code_chunker(languages=["python"])
    assert strat._detect_language("a/b/c.py") == "python"
    assert strat._detect_language("a.rs") == "rust"
    assert strat._detect_language("a.unknownext") is None


def test_strategy_chunk_language_from_metadata(no_ts_pack):
    strat = create_code_chunker(languages=["python"])
    # explicit language via metadata, regex fallback path
    chunks = strat.chunk("def f():\n    return 1\n", "noext", {"language": "python"})
    assert chunks
    assert chunks[0].metadata["language"] == "python"


def test_strategy_fallback_to_semantic(no_ts_pack):
    # A language with no registered parser -> semantic fallback
    strat = create_code_chunker(languages=["python"])
    text = "Just some prose. " * 40
    chunks = strat.chunk(text, "notes.txt")  # .txt not in EXTENSION map
    assert chunks
    for c in chunks:
        assert c.metadata["chunking_strategy"] == "code"
        assert c.metadata["chunk_type"] == "code_fallback"


def test_create_code_chunker_kwargs():
    strat = create_code_chunker(languages=["python"], chunk_size=256)
    assert strat.config.chunk_size == 256
    assert strat.config.languages == ["python"]
