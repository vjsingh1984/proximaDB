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

from proximadb_sdk.chunking_strategies.code import (
    CodeChunkingConfig,
    CodeChunkingStrategy,
    CodeRelationType,
    CodeSymbol,
    CodeSymbolType,
    ParsedCode,
    SourceLocation,
    create_code_chunker,
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


# ---------------------------------------------------------------------------
# Python parser - regex fallback path
# ---------------------------------------------------------------------------


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


# ---------------------------------------------------------------------------
# JavaScript / TypeScript parser (parse returns empty placeholder)
# ---------------------------------------------------------------------------


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


# ---------------------------------------------------------------------------
# SQL parser
# ---------------------------------------------------------------------------

SQL_SOURCE = """\
CREATE TABLE users (id INT, name TEXT);
CREATE OR REPLACE FUNCTION add_user(name TEXT) RETURNS VOID AS $$ $$ LANGUAGE sql;
CREATE VIEW active_users AS SELECT * FROM users;
CREATE PROCEDURE cleanup() AS BEGIN END;
"""


# ---------------------------------------------------------------------------
# YAML parser (regex fallback)
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# JSON parser (regex fallback)
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# XML parser (always regex)
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Perl parser (regex fallback)
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Placeholder parsers (return empty)
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# CodeChunkingStrategy - top level
# ---------------------------------------------------------------------------


def test_strategy_init_default(no_ts_pack):
    """The allowed-language set comes from the installed package (TD-CG2 S4)."""
    strat = CodeChunkingStrategy()
    assert "python" in strat._allowed_languages
    assert not hasattr(strat, "_parsers"), "the in-SDK parser cache should be gone"


def test_strategy_init_specific_languages(no_ts_pack):
    """A configured language list still scopes the strategy (TD-CG2 S4).

    `_get_parser` and the `_parsers` cache are gone with the in-SDK parsers.
    The configured set is taken at face value here; whether a language can
    actually be parsed is asserted by R1 in
    `tests/chunking/test_code_language_surface.py`, against the installed
    package rather than against a table this module maintains.
    """
    cfg = CodeChunkingConfig(languages=["python", "unknownlang"])
    strat = CodeChunkingStrategy(cfg)
    assert strat._allowed_languages == {"python", "unknownlang"}
    assert not hasattr(strat, "_get_parser")


def test_strategy_chunk_python_ast(ts_pack):
    """The delegated code path's metadata contract.

    Updated for the delegation rather than skipped. The original asserted that
    `chunks[0]` carries `symbol_id`, which assumed every chunk is a symbol
    chunk. The shared package additionally emits WINDOW chunks for code that
    belongs to no symbol -- imports, module constants, module-level statements
    -- which the in-SDK parser left covered by nothing at all. So chunk[0] is
    now the imports window, and that is an improvement (TD-CG2 R4: the union of
    chunk spans must cover every non-whitespace byte), not a regression.

    What must still hold is that symbol chunks exist, carry symbol identity, and
    carry their relations -- `code_knowledge.py` builds graph EDGES from
    `metadata["relations"]`, so losing them yields a knowledge graph of nodes
    with no edges.
    """
    strat = create_code_chunker(languages=["python"])
    chunks = strat.chunk(PY_SOURCE, "pkg/mod.py", {"extra": "v"})
    assert chunks
    for chunk in chunks:
        assert chunk.metadata["chunking_strategy"] == "code"
        assert chunk.metadata["extra"] == "v"
        assert "#" in chunk.chunk_id

    symbol_chunks = [c for c in chunks if c.metadata.get("symbol_id")]
    assert symbol_chunks, "no symbol chunks: the delegation extracted nothing"
    assert all("fully_qualified_name" in c.metadata for c in symbol_chunks)
    assert all(c.metadata["language"] == "python" for c in symbol_chunks)

    # top_level calls itself, so at least one symbol must carry a relation.
    assert any("relations" in c.metadata for c in chunks)


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
