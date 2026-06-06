"""Offline unit tests for proximadb_sdk.chunking_strategies.parser_utils.

Pure helpers and singletons; no network/IO. Tree-sitter import is expected to
fail gracefully (regex fallback), so parsers init without external deps.
"""

import re

import pytest

from proximadb_sdk.chunking_strategies import parser_utils as pu


# ---------------------------------------------------------------------------
# Error classes
# ---------------------------------------------------------------------------


def test_parser_error_attrs():
    e = pu.ParserError("boom", language="python", file_path="a.py")
    assert e.language == "python"
    assert e.file_path == "a.py"
    assert str(e) == "boom"


def test_parser_initialization_and_unsupported():
    assert issubclass(pu.ParserInitializationError, pu.ParserError)
    assert issubclass(pu.UnsupportedLanguageError, pu.ParserError)
    e = pu.UnsupportedLanguageError("nope", language="cobol")
    assert e.language == "cobol"


def test_parse_error_line_column():
    e = pu.ParseError("syntax", line=3, column=7, language="rust", file_path="x.rs")
    assert e.line == 3
    assert e.column == 7
    assert e.language == "rust"
    assert e.file_path == "x.rs"


# ---------------------------------------------------------------------------
# FallbackStrategy / FallbackConfig
# ---------------------------------------------------------------------------


def test_fallback_strategy_members():
    members = {s.name for s in pu.FallbackStrategy}
    assert {"NONE", "REGEX", "SEMANTIC", "EMPTY", "PARTIAL"} <= members


def test_fallback_config_defaults():
    cfg = pu.FallbackConfig()
    assert cfg.strategy == pu.FallbackStrategy.REGEX
    assert cfg.max_retries == 1
    assert cfg.retry_delay_ms == 100
    assert cfg.log_errors is True
    assert cfg.collect_metrics is True


# ---------------------------------------------------------------------------
# ParserMetrics
# ---------------------------------------------------------------------------


def test_parser_metrics_to_dict():
    m = pu.ParserMetrics(
        language="python",
        file_path="m.py",
        parse_time_ms=12.3456,
        symbol_count=5,
        relation_count=2,
        error_count=1,
        fallback_used=True,
        cache_hit=True,
        tree_sitter_available=False,
    )
    d = m.to_dict()
    assert d["language"] == "python"
    assert d["parse_time_ms"] == 12.35  # rounded to 2 places
    assert d["symbol_count"] == 5
    assert d["relation_count"] == 2
    assert d["error_count"] == 1
    assert d["fallback_used"] is True
    assert d["cache_hit"] is True
    assert d["tree_sitter_available"] is False


# ---------------------------------------------------------------------------
# MetricsCollector singleton
# ---------------------------------------------------------------------------


def test_metrics_collector_singleton_and_lifecycle():
    c1 = pu.get_metrics_collector()
    c2 = pu.MetricsCollector()
    assert c1 is c2

    c1.clear()
    assert c1.get_summary() == {}

    c1.enable()
    c1.record(
        pu.ParserMetrics(
            language="python",
            file_path="a.py",
            parse_time_ms=10.0,
            symbol_count=3,
            relation_count=1,
            error_count=0,
            fallback_used=False,
            cache_hit=True,
        )
    )
    c1.record(
        pu.ParserMetrics(
            language="python",
            file_path="b.py",
            parse_time_ms=20.0,
            symbol_count=1,
            relation_count=0,
            error_count=1,
            fallback_used=True,
            cache_hit=False,
        )
    )
    c1.record(
        pu.ParserMetrics(
            language="rust",
            file_path="c.rs",
            parse_time_ms=5.0,
            symbol_count=2,
            relation_count=2,
        )
    )

    summary = c1.get_summary()
    assert set(summary.keys()) == {"python", "rust"}
    py = summary["python"]
    assert py["total_parses"] == 2
    assert py["avg_parse_time_ms"] == 15.0
    assert py["total_symbols"] == 4
    assert py["total_relations"] == 1
    assert py["error_rate"] == 0.5
    assert py["fallback_rate"] == 0.5
    assert py["cache_hit_rate"] == 0.5

    c1.clear()
    assert c1.get_summary() == {}


def test_metrics_collector_disable_skips_record():
    c = pu.get_metrics_collector()
    c.clear()
    c.disable()
    c.record(pu.ParserMetrics(language="x", file_path="x"))
    assert c.get_summary() == {}
    c.enable()  # restore for other tests
    c.clear()


# ---------------------------------------------------------------------------
# ParserCache singleton + LRU
# ---------------------------------------------------------------------------


def test_parser_cache_basic_put_get():
    cache = pu.get_parser_cache()
    assert cache is pu.ParserCache()
    cache.clear()
    assert cache.size() == 0

    p = object()
    cache.put("python", p)
    assert cache.contains("python")
    assert cache.get("python") is p
    assert cache.size() == 1

    # miss
    assert cache.get("nope") is None
    cache.clear()


def test_parser_cache_replace_existing():
    cache = pu.get_parser_cache()
    cache.clear()
    a, b = object(), object()
    cache.put("go", a)
    cache.put("go", b)  # replace branch
    assert cache.get("go") is b
    assert cache.size() == 1
    cache.clear()


def test_parser_cache_lru_eviction():
    # Singleton already created with max_size=32; force eviction by saturating.
    cache = pu.get_parser_cache()
    cache.clear()
    max_size = cache._max_size
    sentinels = {}
    for i in range(max_size):
        obj = object()
        sentinels[f"lang{i}"] = obj
        cache.put(f"lang{i}", obj)
    assert cache.size() == max_size

    # touch lang0 so it is most-recently-used; lang1 becomes LRU
    cache.get("lang0")
    extra = object()
    cache.put("overflow", extra)
    assert cache.size() == max_size
    assert cache.contains("lang0")  # survived
    assert not cache.contains("lang1")  # evicted as LRU
    assert cache.get("overflow") is extra
    cache.clear()


# ---------------------------------------------------------------------------
# Decorators
# ---------------------------------------------------------------------------


class _Result:
    def __init__(self, symbols, relations):
        self.symbols = symbols
        self.relations = relations


class _FakeParser:
    language = "python"

    def __init__(self):
        self._parser = object()  # tree_sitter_available True

    @pu.with_metrics
    def parse(self, content, file_path):
        return _Result(symbols=[1, 2, 3], relations=[("a", "b")])

    @pu.with_metrics
    def boom(self, content, file_path):
        raise ValueError("nope")


def test_with_metrics_success_records():
    c = pu.get_metrics_collector()
    c.clear()
    p = _FakeParser()
    res = p.parse("code", "f.py")
    assert res.symbols == [1, 2, 3]
    summary = c.get_summary()
    assert summary["python"]["total_symbols"] == 3
    assert summary["python"]["total_relations"] == 1
    c.clear()


def test_with_metrics_error_records_and_reraises():
    c = pu.get_metrics_collector()
    c.clear()
    p = _FakeParser()
    with pytest.raises(ValueError):
        p.boom("code", "f.py")
    summary = c.get_summary()
    assert summary["python"]["error_rate"] == 1.0
    c.clear()


class _FallbackParser:
    language = "python"

    def __init__(self, fail=True):
        self._fail = fail
        self._parser = None
        self._partial_result = "PARTIAL"

    def _real(self, content, file_path):
        if self._fail:
            raise RuntimeError("parse failed")
        return "OK"

    def _fallback_regex_parse(self, content, file_path):
        return "REGEX"

    def _fallback_semantic_parse(self, content, file_path):
        return "SEMANTIC"

    def _create_empty_result(self, file_path):
        return "EMPTY"


def _wrap(parser, cfg):
    fn = pu.with_fallback(cfg)(_FallbackParser._real)
    return lambda c, f: fn(parser, c, f)


def test_with_fallback_success_no_fallback():
    p = _FallbackParser(fail=False)
    cfg = pu.FallbackConfig(strategy=pu.FallbackStrategy.REGEX, max_retries=0)
    assert _wrap(p, cfg)("x", "f") == "OK"


def test_with_fallback_regex():
    p = _FallbackParser(fail=True)
    cfg = pu.FallbackConfig(
        strategy=pu.FallbackStrategy.REGEX, max_retries=0, log_errors=True
    )
    assert _wrap(p, cfg)("x", "f") == "REGEX"


def test_with_fallback_semantic():
    p = _FallbackParser(fail=True)
    cfg = pu.FallbackConfig(strategy=pu.FallbackStrategy.SEMANTIC, max_retries=0)
    assert _wrap(p, cfg)("x", "f") == "SEMANTIC"


def test_with_fallback_empty():
    p = _FallbackParser(fail=True)
    cfg = pu.FallbackConfig(strategy=pu.FallbackStrategy.EMPTY, max_retries=0)
    assert _wrap(p, cfg)("x", "f") == "EMPTY"


def test_with_fallback_partial_uses_partial_result():
    p = _FallbackParser(fail=True)
    cfg = pu.FallbackConfig(strategy=pu.FallbackStrategy.PARTIAL, max_retries=0)
    assert _wrap(p, cfg)("x", "f") == "PARTIAL"


def test_with_fallback_partial_without_attr_returns_empty():
    p = _FallbackParser(fail=True)
    del p._partial_result
    cfg = pu.FallbackConfig(strategy=pu.FallbackStrategy.PARTIAL, max_retries=0)
    assert _wrap(p, cfg)("x", "f") == "EMPTY"


def test_with_fallback_none_reraises():
    p = _FallbackParser(fail=True)
    cfg = pu.FallbackConfig(strategy=pu.FallbackStrategy.NONE, max_retries=0)
    with pytest.raises(RuntimeError):
        _wrap(p, cfg)("x", "f")


def test_with_fallback_retries_then_falls_back():
    p = _FallbackParser(fail=True)
    # small delay to keep it fast; 1 retry exercises the time.sleep branch
    cfg = pu.FallbackConfig(
        strategy=pu.FallbackStrategy.EMPTY, max_retries=1, retry_delay_ms=1
    )
    assert _wrap(p, cfg)("x", "f") == "EMPTY"


def test_with_fallback_default_config():
    p = _FallbackParser(fail=True)
    fn = pu.with_fallback()(_FallbackParser._real)
    # default strategy REGEX, default max_retries=1 (delay 100ms once -> ok, fast)
    assert fn(p, "x", "f") == "REGEX"


def test_cached_parser_decorator_miss_then_hit():
    cache = pu.get_parser_cache()
    cache.clear()

    calls = []

    class _CP:
        language = "kotlin"

        def __init__(self):
            self._parser = "PARSER_OBJ"
            self._language = "kotlin"

        @pu.cached_parser
        def setup(self):
            calls.append("ran")
            return "done"

    first = _CP()
    assert first.setup() == "done"  # miss -> put
    assert cache.contains("kotlin")

    # second instance: hit branch copies _parser/_language from cached
    second = _CP()
    second._parser = None
    assert second.setup() == "done"
    assert second._parser == "PARSER_OBJ"
    assert second._language == "kotlin"
    assert len(calls) == 2
    cache.clear()


# ---------------------------------------------------------------------------
# Parser family base classes (regex helpers)
# ---------------------------------------------------------------------------


class _CParser(pu.CFamilyParser):
    @property
    def language(self):
        return "c"

    @property
    def file_extensions(self):
        return [".c"]

    def parse(self, content, file_path):
        return None

    def _fallback_regex_parse(self, content, file_path):
        return None


def test_cfamily_find_matching_brace_and_extract_block():
    p = _CParser()
    assert p.has_tree_sitter is False or p.has_tree_sitter is True  # init ran
    content = "void f() { int x = 1; }"
    start = content.index("{")
    end = p._find_matching_brace(content, start)
    assert content[end - 1] == "}"

    block, block_end = p._extract_block(content, 0)
    assert block.startswith("{") and block.endswith("}")
    assert block_end == end


def test_cfamily_find_matching_brace_nested_and_strings():
    p = _CParser()
    content = 'f() { if (a) { g("}"); } }'
    start = content.index("{")
    end = p._find_matching_brace(content, start)
    # the closing brace must be the final one, not the one in the string
    assert end == len(content)


def test_cfamily_find_matching_brace_unbalanced():
    p = _CParser()
    content = "f() { unclosed"
    start = content.index("{")
    assert p._find_matching_brace(content, start) == -1


def test_cfamily_extract_block_no_brace():
    p = _CParser()
    block, end = p._extract_block("no braces here", 0)
    assert block == ""
    assert end == -1


def test_cfamily_extract_block_unmatched_returns_empty():
    p = _CParser()
    block, end = p._extract_block("f() { unclosed", 0)
    assert block == ""
    assert end == -1


def test_cfamily_regex_patterns_match():
    assert _CParser.FUNCTION_PATTERN.search("public void doIt(int a) {")
    assert _CParser.CLASS_PATTERN.search("public class Foo")
    assert _CParser.BLOCK_START.search("{")
    assert _CParser.BLOCK_END.search("}")


class _JVMParser(pu.JVMFamilyParser):
    @property
    def language(self):
        return "java"

    @property
    def file_extensions(self):
        return [".java"]

    def parse(self, content, file_path):
        return None

    def _fallback_regex_parse(self, content, file_path):
        return None


def test_jvm_extract_package_imports_annotations():
    p = _JVMParser()
    content = (
        "package com.example.app;\n"
        "import java.util.List;\n"
        "import static org.junit.Assert.*;\n"
        "@Override\n"
        "@Deprecated\n"
        "public void run() {}\n"
    )
    assert p._extract_package(content) == "com.example.app"
    imports = p._extract_imports(content)
    assert "java.util.List" in imports
    assert "org.junit.Assert.*" in imports

    pos = content.index("public void run")
    annotations = p._extract_annotations(content, pos)
    assert "Override" in annotations
    assert "Deprecated" in annotations


def test_jvm_extract_package_none():
    p = _JVMParser()
    assert p._extract_package("class Foo {}") is None


def test_jvm_extract_annotations_stops_at_code():
    p = _JVMParser()
    content = "int x = 5;\n@Tag\nvoid m() {}\n"
    pos = content.index("void m")
    anns = p._extract_annotations(content, pos)
    assert anns == ["Tag"]


class _DynParser(pu.DynamicLanguageParser):
    @property
    def language(self):
        return "python"

    @property
    def file_extensions(self):
        return [".py"]

    def parse(self, content, file_path):
        return None

    def _fallback_regex_parse(self, content, file_path):
        return None


def test_dynamic_patterns_match():
    assert _DynParser.PYTHON_FUNCTION_PATTERN.search("    def foo(a, b):")
    assert _DynParser.PYTHON_CLASS_PATTERN.search("class Foo(Base):")
    assert _DynParser.RUBY_METHOD_PATTERN.search("  def self.bar?")
    assert _DynParser.RUBY_CLASS_PATTERN.search("class Baz < Parent")


def test_dynamic_find_indentation_block_end():
    p = _DynParser()
    content = (
        "def foo():\n"
        "    x = 1\n"
        "\n"
        "    # comment\n"
        "    y = 2\n"
        "z = 3\n"
    )
    start = content.index("def foo")
    end = p._find_indentation_block_end(content, start, base_indent=0)
    # the dedented "z = 3" line terminates the block (is not consumed)
    assert "z = 3" in content[end:]
    assert 0 < end < len(content)


def test_dynamic_find_indentation_block_end_consumes_indented_body():
    p = _DynParser()
    content = "def foo():\n    a = 1\n    b = 2\n"
    start = content.index("def foo")
    end = p._find_indentation_block_end(content, start, base_indent=0)
    # all lines are part of the block (no dedent), advances past first body line
    assert end > start
    assert end <= len(content)


class _FuncParser(pu.FunctionalLanguageParser):
    @property
    def language(self):
        return "haskell"

    @property
    def file_extensions(self):
        return [".hs"]

    def parse(self, content, file_path):
        return None

    def _fallback_regex_parse(self, content, file_path):
        return None


def test_functional_patterns_match():
    assert _FuncParser.HASKELL_FUNCTION_PATTERN.search("add :: Int -> Int -> Int")
    assert _FuncParser.HASKELL_MODULE_PATTERN.search("module Data.List")
    assert _FuncParser.ELIXIR_MODULE_PATTERN.search("defmodule MyApp.Thing")
    assert _FuncParser.ELIXIR_FUNCTION_PATTERN.search("defp helper")


class _Markup(pu.MarkupParser):
    @property
    def language(self):
        return "json"

    @property
    def file_extensions(self):
        return [".json"]

    def parse(self, content, file_path):
        return None

    def _fallback_regex_parse(self, content, file_path):
        return None


def test_markup_not_implemented():
    p = _Markup()
    with pytest.raises(NotImplementedError):
        p._extract_keys("{}")
    with pytest.raises(NotImplementedError):
        p._build_hierarchy("{}")


# ---------------------------------------------------------------------------
# BaseLanguageParser fallback/empty/semantic + hash
# ---------------------------------------------------------------------------


def test_base_semantic_empty_and_hash():
    p = _DynParser()
    semantic = p._fallback_semantic_parse("hello", "f.py")
    assert semantic.file_path == "f.py"
    assert semantic.language == "python"
    assert semantic.symbols == []
    assert semantic.content_hash  # sha256 hex

    empty = p._create_empty_result("g.py")
    assert empty.content_hash == ""
    assert empty.language == "python"

    h = p._compute_content_hash("abc")
    assert len(h) == 64
    assert re.fullmatch(r"[0-9a-f]{64}", h)


def test_base_tree_sitter_language_name_defaults_to_language():
    p = _DynParser()
    assert p.tree_sitter_language_name == "python"


# ---------------------------------------------------------------------------
# Plugin architecture
# ---------------------------------------------------------------------------


def _make_plugin_class(lang="python"):
    class _P(pu.BaseLanguageParser):
        @property
        def language(self):
            return lang

        @property
        def file_extensions(self):
            return [".py"]

        def parse(self, content, file_path):
            return None

        def _fallback_regex_parse(self, content, file_path):
            return None

    return _P


def test_parser_plugin_get_parser_and_supports():
    cls = _make_plugin_class()
    plugin = pu.ParserPlugin(
        name="p1",
        parser_class=cls,
        languages=["Python"],
        extensions=[".py", ".pyi"],
        priority=5,
        metadata={"k": "v"},
    )
    assert plugin.metadata == {"k": "v"}
    inst1 = plugin.get_parser()
    inst2 = plugin.get_parser()
    assert inst1 is inst2  # cached instance

    assert plugin.supports_language("python")
    assert plugin.supports_language("PYTHON")
    assert not plugin.supports_language("rust")

    # query without a leading dot is normalized to ".py" before comparison
    assert plugin.supports_extension("py")
    assert plugin.supports_extension(".pyi")
    assert not plugin.supports_extension(".rs")


def test_parser_plugin_default_metadata():
    cls = _make_plugin_class()
    plugin = pu.ParserPlugin("p", cls, ["go"], [".go"])
    assert plugin.metadata == {}


def test_plugin_registry_register_unregister_and_lookup():
    reg = pu.get_plugin_registry()
    assert reg is pu.ParserPluginRegistry()

    cls = _make_plugin_class("python")
    low = pu.ParserPlugin("lowprio", cls, ["python"], ["py"], priority=1)
    high = pu.ParserPlugin("hiprio", cls, ["python"], ["py"], priority=10)

    # clean slate for these names
    reg.unregister("lowprio")
    reg.unregister("hiprio")

    assert reg.register(low) is True
    assert reg.register(low) is False  # duplicate
    assert reg.register(high) is True

    # highest priority wins for both language and extension lookup
    by_lang = reg.get_parser_for_language("PYTHON")
    assert by_lang is not None
    by_ext = reg.get_parser_for_extension("py")
    assert by_ext is not None
    by_ext_dot = reg.get_parser_for_extension(".py")
    assert by_ext_dot is not None

    names = {p["name"] for p in reg.list_plugins()}
    assert {"lowprio", "hiprio"} <= names

    assert "python" in reg.get_supported_languages()
    assert ".py" in reg.get_supported_extensions()

    # unregister and confirm cleanup
    assert reg.unregister("lowprio") is True
    assert reg.unregister("hiprio") is True
    assert reg.unregister("nonexistent") is False


def test_plugin_registry_lookup_misses():
    reg = pu.get_plugin_registry()
    assert reg.get_parser_for_language("nonexistent_lang_xyz") is None
    assert reg.get_parser_for_extension(".nonexistent_xyz") is None
    assert reg.get_parser_for_extension("nonexistent_xyz") is None


# ---------------------------------------------------------------------------
# ConfigValidator
# ---------------------------------------------------------------------------


def test_validate_chunk_size_valid_with_small_warning():
    res = pu.ConfigValidator.validate_chunk_size(50)
    assert res.valid is True
    assert any("very small" in w for w in res.warnings)


def test_validate_chunk_size_large_warning():
    res = pu.ConfigValidator.validate_chunk_size(20000)
    assert res.valid is True
    assert any("large" in w for w in res.warnings)


def test_validate_chunk_size_below_min():
    res = pu.ConfigValidator.validate_chunk_size(
        500, min_chunk_size=1000, max_chunk_size=100000
    )
    assert res.valid is False
    assert any("min_chunk_size" in e for e in res.errors)


def test_validate_chunk_size_above_max():
    res = pu.ConfigValidator.validate_chunk_size(
        500000, min_chunk_size=0, max_chunk_size=100000
    )
    assert res.valid is False
    assert any("max_chunk_size" in e for e in res.errors)


def test_validate_overlap_valid():
    res = pu.ConfigValidator.validate_overlap(100, 1000)
    assert res.valid is True
    assert res.errors == []


def test_validate_overlap_negative():
    res = pu.ConfigValidator.validate_overlap(-5, 1000)
    assert res.valid is False
    assert any(">= 0" in e for e in res.errors)


def test_validate_overlap_ge_chunk_size():
    res = pu.ConfigValidator.validate_overlap(1000, 1000)
    assert res.valid is False
    assert any("< chunk_size" in e for e in res.errors)


def test_validate_overlap_high_redundancy_warning():
    res = pu.ConfigValidator.validate_overlap(600, 1000)
    assert res.valid is True
    assert any("redundancy" in w for w in res.warnings)


def test_validate_languages_warns_unsupported():
    res = pu.ConfigValidator.validate_languages(["definitely_not_a_lang"])
    assert res.valid is True
    assert any("may not be fully supported" in w for w in res.warnings)


def test_validate_config_full():
    class Cfg:
        chunk_size = 50  # triggers small warning
        chunk_overlap = 1000  # >= chunk_size -> error
        min_chunk_size = 0
        max_chunk_size = 100000
        languages = ["unknown_lang"]

    res = pu.ConfigValidator.validate_config(Cfg())
    assert res.valid is False
    assert any("chunk_overlap" in e for e in res.errors)
    assert res.warnings  # small chunk + language warnings


def test_validate_config_empty_object():
    class Empty:
        pass

    res = pu.ConfigValidator.validate_config(Empty())
    assert res.valid is True
    assert res.errors == []


def test_validate_config_valid_sizes():
    class Cfg:
        chunk_size = 1000
        chunk_overlap = 100
        languages = []

    res = pu.ConfigValidator.validate_config(Cfg())
    assert res.valid is True


def test_validation_result_defaults():
    r = pu.ValidationResult(valid=True)
    assert r.errors == []
    assert r.warnings == []


# ---------------------------------------------------------------------------
# parser_context
# ---------------------------------------------------------------------------


def test_parser_context_cache_miss():
    cache = pu.get_parser_cache()
    cache.clear()
    with pu.parser_context("python") as parser:
        assert parser is None  # cache miss
    cache.clear()


def test_parser_context_cache_hit():
    cache = pu.get_parser_cache()
    cache.clear()
    sentinel = object()
    cache.put("python", sentinel)
    with pu.parser_context("python") as parser:
        assert parser is sentinel
    cache.clear()


# ---------------------------------------------------------------------------
# detect_language_from_content
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "shebang,expected",
    [
        ("#!/usr/bin/env python3\n", "python"),
        ("#!/usr/bin/node\n", "javascript"),
        ("#!/usr/bin/env deno\n", "javascript"),
        ("#!/usr/bin/ruby\n", "ruby"),
        ("#!/usr/bin/perl\n", "perl"),
        ("#!/bin/bash\n", "bash"),
    ],
)
def test_detect_language_shebang(shebang, expected):
    assert pu.detect_language_from_content(shebang + "code") == expected


def test_detect_language_go():
    content = "package main\n\nfunc main() {}\n"
    assert pu.detect_language_from_content(content) == "go"


def test_detect_language_rust():
    content = "fn main() {\n    let x = 1;\n}\n"
    assert pu.detect_language_from_content(content) == "rust"


def test_detect_language_java():
    content = "public class Foo {\n}\n"
    assert pu.detect_language_from_content(content) == "java"


def test_detect_language_python_body():
    content = "def foo():\n    return 1\n"
    assert pu.detect_language_from_content(content) == "python"


def test_detect_language_javascript_body():
    content = "function hello() { return 1; }\n"
    assert pu.detect_language_from_content(content) == "javascript"


def test_detect_language_unknown():
    assert pu.detect_language_from_content("just some plain prose text") is None


def test_detect_language_empty():
    assert pu.detect_language_from_content("") is None
