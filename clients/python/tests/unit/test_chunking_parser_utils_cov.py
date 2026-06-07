"""Offline unit tests for proximadb_sdk.chunking_strategies.parser_utils.

Pure module: no network/server. Exercises errors, fallback config, metrics,
cache (LRU), decorators, parser family base classes, plugin registry,
config validation, and utility helpers.
"""

import re

import pytest

from proximadb_sdk.chunking_strategies import parser_utils as pu


# ---------------------------------------------------------------------------
# Fixtures to reset process-wide singletons (shared state across tests).
# ---------------------------------------------------------------------------
@pytest.fixture(autouse=True)
def _reset_singletons():
    pu.get_metrics_collector().clear()
    pu.get_metrics_collector().enable()
    pu.get_parser_cache().clear()
    reg = pu.get_plugin_registry()
    for entry in list(reg.list_plugins()):
        reg.unregister(entry["name"])
    yield
    pu.get_metrics_collector().clear()
    pu.get_parser_cache().clear()
    reg = pu.get_plugin_registry()
    for entry in list(reg.list_plugins()):
        reg.unregister(entry["name"])


# ---------------------------------------------------------------------------
# Errors
# ---------------------------------------------------------------------------
def test_parser_error_attrs():
    e = pu.ParserError("boom", language="python", file_path="a.py")
    assert e.language == "python"
    assert e.file_path == "a.py"
    assert str(e) == "boom"


def test_parser_initialization_error_is_parser_error():
    e = pu.ParserInitializationError("init", language="go")
    assert isinstance(e, pu.ParserError)
    assert e.language == "go"


def test_parse_error_line_column():
    e = pu.ParseError("bad", line=5, column=3, language="rust", file_path="x.rs")
    assert e.line == 5
    assert e.column == 3
    assert e.language == "rust"
    assert e.file_path == "x.rs"


def test_unsupported_language_error():
    e = pu.UnsupportedLanguageError("nope", language="brainfuck")
    assert isinstance(e, pu.ParserError)
    assert e.language == "brainfuck"


# ---------------------------------------------------------------------------
# Fallback config / strategy
# ---------------------------------------------------------------------------
def test_fallback_strategy_members():
    names = {s.name for s in pu.FallbackStrategy}
    assert {"NONE", "REGEX", "SEMANTIC", "EMPTY", "PARTIAL"} <= names


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
        file_path="a.py",
        parse_time_ms=1.23456,
        symbol_count=3,
        relation_count=2,
        error_count=1,
        fallback_used=True,
        cache_hit=True,
        tree_sitter_available=False,
    )
    d = m.to_dict()
    assert d["language"] == "python"
    assert d["parse_time_ms"] == 1.23  # rounded to 2 places
    assert d["symbol_count"] == 3
    assert d["relation_count"] == 2
    assert d["error_count"] == 1
    assert d["fallback_used"] is True
    assert d["cache_hit"] is True
    assert d["tree_sitter_available"] is False


# ---------------------------------------------------------------------------
# MetricsCollector
# ---------------------------------------------------------------------------
def test_metrics_collector_singleton():
    a = pu.MetricsCollector()
    b = pu.get_metrics_collector()
    assert a is b


def test_metrics_collector_summary_empty():
    c = pu.get_metrics_collector()
    c.clear()
    assert c.get_summary() == {}


def test_metrics_collector_record_and_summary():
    c = pu.get_metrics_collector()
    c.clear()
    c.record(
        pu.ParserMetrics(
            language="py", file_path="a.py", parse_time_ms=10.0,
            symbol_count=2, relation_count=1, error_count=0, cache_hit=True,
        )
    )
    c.record(
        pu.ParserMetrics(
            language="py", file_path="b.py", parse_time_ms=20.0,
            symbol_count=4, relation_count=3, error_count=1, fallback_used=True,
        )
    )
    c.record(
        pu.ParserMetrics(language="go", file_path="m.go", parse_time_ms=5.0)
    )
    summary = c.get_summary()
    assert summary["py"]["total_parses"] == 2
    assert summary["py"]["avg_parse_time_ms"] == 15.0
    assert summary["py"]["total_symbols"] == 6
    assert summary["py"]["total_relations"] == 4
    assert summary["py"]["error_rate"] == 0.5
    assert summary["py"]["fallback_rate"] == 0.5
    assert summary["py"]["cache_hit_rate"] == 0.5
    assert summary["go"]["total_parses"] == 1


def test_metrics_collector_disable_blocks_record():
    c = pu.get_metrics_collector()
    c.clear()
    c.disable()
    c.record(pu.ParserMetrics(language="py", file_path="a.py"))
    assert c.get_summary() == {}
    c.enable()
    c.record(pu.ParserMetrics(language="py", file_path="a.py"))
    assert c.get_summary() != {}


# ---------------------------------------------------------------------------
# ParserCache (LRU)
# ---------------------------------------------------------------------------
def test_parser_cache_singleton():
    assert pu.ParserCache() is pu.get_parser_cache()


def test_parser_cache_put_get_contains_size():
    cache = pu.get_parser_cache()
    cache.clear()
    assert cache.get("python") is None
    assert cache.contains("python") is False
    obj = object()
    cache.put("python", obj)
    assert cache.get("python") is obj
    assert cache.contains("python") is True
    assert cache.size() == 1


def test_parser_cache_put_existing_updates_access_order():
    cache = pu.get_parser_cache()
    cache.clear()
    cache.put("a", object())
    cache.put("b", object())
    # Re-put existing key 'a' -> removed from access_order then re-added
    new_a = object()
    cache.put("a", new_a)
    assert cache.get("a") is new_a
    assert cache.size() == 2


def test_parser_cache_clear():
    cache = pu.get_parser_cache()
    cache.put("x", object())
    cache.clear()
    assert cache.size() == 0
    assert cache.contains("x") is False


def test_parser_cache_lru_eviction():
    # Force a tiny cache by mutating the private max_size of the singleton.
    cache = pu.get_parser_cache()
    cache.clear()
    original = cache._max_size
    try:
        cache._max_size = 2
        cache.put("a", object())
        cache.put("b", object())
        # Access 'a' so 'b' becomes LRU
        cache.get("a")
        cache.put("c", object())  # should evict 'b'
        assert cache.contains("a") is True
        assert cache.contains("c") is True
        assert cache.contains("b") is False
        assert cache.size() == 2
    finally:
        cache._max_size = original
        cache.clear()


# ---------------------------------------------------------------------------
# Concrete parser for exercising base-class behavior + decorators
# ---------------------------------------------------------------------------
class _FakeResult:
    def __init__(self, symbols, relations):
        self.symbols = symbols
        self.relations = relations


class _DummyParser(pu.BaseLanguageParser):
    @property
    def language(self) -> str:
        return "dummy"

    @property
    def file_extensions(self):
        return [".dum"]

    def parse(self, content, file_path):
        return self._create_empty_result(file_path)

    def _fallback_regex_parse(self, content, file_path):
        from proximadb_sdk.chunking_strategies.code import ParsedCode

        return ParsedCode(
            file_path=file_path,
            language=self.language,
            symbols=["regex_sym"],
            relations=[],
            imports=[],
            content_hash="regex",
        )


def test_base_parser_init_no_tree_sitter():
    p = _DummyParser()
    # tree_sitter not installed in env -> ImportError path -> _parser None
    assert p.has_tree_sitter is False
    assert p._parser is None
    assert p.tree_sitter_language_name == "dummy"


def test_base_parser_empty_and_semantic_results():
    p = _DummyParser()
    empty = p._create_empty_result("z.dum")
    assert empty.file_path == "z.dum"
    assert empty.language == "dummy"
    assert empty.symbols == []
    assert empty.content_hash == ""

    sem = p._fallback_semantic_parse("hello", "z.dum")
    assert sem.language == "dummy"
    # semantic computes a real sha256 hash
    assert len(sem.content_hash) == 64


def test_base_parser_compute_content_hash_deterministic():
    p = _DummyParser()
    h1 = p._compute_content_hash("abc")
    h2 = p._compute_content_hash("abc")
    assert h1 == h2
    assert h1 != p._compute_content_hash("abd")


# ---------------------------------------------------------------------------
# Decorators
# ---------------------------------------------------------------------------
def test_with_metrics_success_records():
    collector = pu.get_metrics_collector()
    collector.clear()

    class P(_DummyParser):
        @pu.with_metrics
        def do(self, content, file_path):
            return _FakeResult(symbols=[1, 2], relations=[3])

    P().do("text", "f.dum")
    summary = collector.get_summary()
    assert summary["dummy"]["total_symbols"] == 2
    assert summary["dummy"]["total_relations"] == 1
    assert summary["dummy"]["error_rate"] == 0.0


def test_with_metrics_exception_increments_error_and_reraises():
    collector = pu.get_metrics_collector()
    collector.clear()

    class P(_DummyParser):
        @pu.with_metrics
        def do(self, content, file_path):
            raise ValueError("kaboom")

    with pytest.raises(ValueError):
        P().do("text", "f.dum")
    summary = collector.get_summary()
    assert summary["dummy"]["error_rate"] == 1.0


def test_with_fallback_regex_strategy():
    cfg = pu.FallbackConfig(strategy=pu.FallbackStrategy.REGEX, max_retries=0)

    class P(_DummyParser):
        @pu.with_fallback(cfg)
        def do(self, content, file_path):
            raise RuntimeError("fail")

    res = P().do("x", "f.dum")
    assert res.symbols == ["regex_sym"]


def test_with_fallback_none_reraises():
    cfg = pu.FallbackConfig(strategy=pu.FallbackStrategy.NONE, max_retries=0)

    class P(_DummyParser):
        @pu.with_fallback(cfg)
        def do(self, content, file_path):
            raise KeyError("nope")

    with pytest.raises(KeyError):
        P().do("x", "f.dum")


def test_with_fallback_semantic_strategy():
    cfg = pu.FallbackConfig(strategy=pu.FallbackStrategy.SEMANTIC, max_retries=0)

    class P(_DummyParser):
        @pu.with_fallback(cfg)
        def do(self, content, file_path):
            raise RuntimeError("fail")

    res = P().do("body", "f.dum")
    assert len(res.content_hash) == 64


def test_with_fallback_empty_strategy():
    cfg = pu.FallbackConfig(strategy=pu.FallbackStrategy.EMPTY, max_retries=0)

    class P(_DummyParser):
        @pu.with_fallback(cfg)
        def do(self, content, file_path):
            raise RuntimeError("fail")

    res = P().do("body", "f.dum")
    assert res.symbols == []
    assert res.content_hash == ""


def test_with_fallback_partial_with_partial_result():
    cfg = pu.FallbackConfig(strategy=pu.FallbackStrategy.PARTIAL, max_retries=0)

    class P(_DummyParser):
        @pu.with_fallback(cfg)
        def do(self, content, file_path):
            self._partial_result = _FakeResult(symbols=["p"], relations=[])
            raise RuntimeError("fail")

    res = P().do("body", "f.dum")
    assert res.symbols == ["p"]


def test_with_fallback_partial_without_partial_result_uses_empty():
    cfg = pu.FallbackConfig(strategy=pu.FallbackStrategy.PARTIAL, max_retries=0)

    class P(_DummyParser):
        @pu.with_fallback(cfg)
        def do(self, content, file_path):
            raise RuntimeError("fail")

    res = P().do("body", "f.dum")
    assert res.symbols == []


def test_with_fallback_retries_then_succeeds():
    cfg = pu.FallbackConfig(
        strategy=pu.FallbackStrategy.NONE, max_retries=2, retry_delay_ms=0,
        log_errors=False,
    )
    state = {"calls": 0}

    class P(_DummyParser):
        @pu.with_fallback(cfg)
        def do(self, content, file_path):
            state["calls"] += 1
            if state["calls"] < 2:
                raise RuntimeError("transient")
            return _FakeResult(symbols=["ok"], relations=[])

    res = P().do("body", "f.dum")
    assert res.symbols == ["ok"]
    assert state["calls"] == 2


def test_with_fallback_default_config_regex():
    # max_retries=0 keeps it fast; default config arg path covered separately.
    cfg = pu.FallbackConfig(strategy=pu.FallbackStrategy.REGEX, max_retries=0)

    class P(_DummyParser):
        @pu.with_fallback(cfg)
        def do(self, content, file_path):
            raise RuntimeError("x")

    assert P().do("c", "f.dum").symbols == ["regex_sym"]


def test_with_fallback_no_arg_uses_default_config():
    # Calling with_fallback() without args -> default FallbackConfig (REGEX).
    class P(_DummyParser):
        @pu.with_fallback()
        def do(self, content, file_path):
            raise RuntimeError("x")

    # default max_retries=1 with retry_delay_ms=100 -> ~0.1s sleep, acceptable.
    assert P().do("c", "f.dum").symbols == ["regex_sym"]


def test_cached_parser_decorator_miss_caches_instance():
    pu.get_parser_cache().clear()
    calls = {"n": 0}

    class P(_DummyParser):
        @pu.cached_parser
        def setup(self):
            calls["n"] += 1
            return "done"

    p1 = P()
    assert p1.setup() == "done"  # miss -> caches p1
    assert pu.get_parser_cache().contains("dummy")
    assert calls["n"] == 1


def test_cached_parser_decorator_hit_path_copies_attrs():
    # On a cache hit the decorator copies cached._parser / cached._language onto
    # self. BaseLanguageParser sets _parser but NOT _language, so the hit path
    # raises AttributeError -- this asserts that real (quirky) behavior.
    pu.get_parser_cache().clear()

    class P(_DummyParser):
        @pu.cached_parser
        def setup(self):
            return "done"

    P().setup()  # miss caches first instance
    with pytest.raises(AttributeError):
        P().setup()  # hit -> tries cached._language -> AttributeError


def test_cached_parser_decorator_hit_with_language_attr():
    # If the cached instance happens to have _language set, the hit path
    # succeeds without re-running the wrapped body side effects.
    pu.get_parser_cache().clear()
    calls = {"n": 0}

    class P(_DummyParser):
        @pu.cached_parser
        def setup(self):
            calls["n"] += 1
            self._language = "dummy"
            return "done"

    P().setup()  # miss -> caches instance that now has _language
    assert calls["n"] == 1
    assert P().setup() == "done"  # hit path copies attrs, then runs body
    assert calls["n"] == 2


# ---------------------------------------------------------------------------
# CFamilyParser helpers
# ---------------------------------------------------------------------------
class _CParser(pu.CFamilyParser):
    @property
    def language(self):
        return "c"

    @property
    def file_extensions(self):
        return [".c"]

    def parse(self, content, file_path):
        return self._create_empty_result(file_path)

    def _fallback_regex_parse(self, content, file_path):
        return self._create_empty_result(file_path)


def test_cfamily_find_matching_brace():
    p = _CParser()
    code = "{ a { b } c }"
    end = p._find_matching_brace(code, 0)
    assert code[end - 1] == "}"
    assert end == len(code)


def test_cfamily_find_matching_brace_with_string():
    p = _CParser()
    code = '{ "}" }'  # the } inside the string must be ignored
    end = p._find_matching_brace(code, 0)
    assert end == len(code)


def test_cfamily_find_matching_brace_unbalanced():
    p = _CParser()
    assert p._find_matching_brace("{ no close", 0) == -1


def test_cfamily_extract_block():
    p = _CParser()
    block, end = p._extract_block("int f() { return 1; }", 0)
    assert block.startswith("{")
    assert block.endswith("}")
    assert end > 0


def test_cfamily_extract_block_no_brace():
    p = _CParser()
    assert p._extract_block("int x;", 0) == ("", -1)


def test_cfamily_extract_block_unbalanced():
    p = _CParser()
    assert p._extract_block("int f() { no close", 0) == ("", -1)


def test_cfamily_patterns_match():
    assert pu.CFamilyParser.FUNCTION_PATTERN.search("public void foo() {")
    assert pu.CFamilyParser.CLASS_PATTERN.search("public class Bar")


# ---------------------------------------------------------------------------
# JVMFamilyParser helpers
# ---------------------------------------------------------------------------
class _JVMParser(pu.JVMFamilyParser):
    @property
    def language(self):
        return "java"

    @property
    def file_extensions(self):
        return [".java"]

    def parse(self, content, file_path):
        return self._create_empty_result(file_path)

    def _fallback_regex_parse(self, content, file_path):
        return self._create_empty_result(file_path)


def test_jvm_extract_package():
    p = _JVMParser()
    assert p._extract_package("package com.example.app;\nclass X {}") == (
        "com.example.app"
    )


def test_jvm_extract_package_none():
    p = _JVMParser()
    assert p._extract_package("class X {}") is None


def test_jvm_extract_imports():
    p = _JVMParser()
    src = "import java.util.List;\nimport static foo.Bar.baz;\n"
    imports = p._extract_imports(src)
    assert "java.util.List" in imports
    assert "foo.Bar.baz" in imports


def test_jvm_extract_annotations():
    p = _JVMParser()
    src = "@Override\n@Deprecated\npublic void foo() {}"
    pos = src.index("public")
    anns = p._extract_annotations(src, pos)
    assert "Override" in anns
    assert "Deprecated" in anns


def test_jvm_extract_annotations_stops_on_code():
    p = _JVMParser()
    src = "int x = 1;\n@Foo\npublic void foo() {}"
    pos = src.index("public")
    anns = p._extract_annotations(src, pos)
    assert anns == ["Foo"]


# ---------------------------------------------------------------------------
# DynamicLanguageParser helpers
# ---------------------------------------------------------------------------
class _PyParser(pu.DynamicLanguageParser):
    @property
    def language(self):
        return "python"

    @property
    def file_extensions(self):
        return [".py"]

    def parse(self, content, file_path):
        return self._create_empty_result(file_path)

    def _fallback_regex_parse(self, content, file_path):
        return self._create_empty_result(file_path)


def test_dynamic_patterns():
    assert pu.DynamicLanguageParser.PYTHON_FUNCTION_PATTERN.search("def foo():")
    assert pu.DynamicLanguageParser.PYTHON_CLASS_PATTERN.search("class Foo:")
    assert pu.DynamicLanguageParser.RUBY_METHOD_PATTERN.search("  def bar?")
    assert pu.DynamicLanguageParser.RUBY_CLASS_PATTERN.search("class Baz < Qux")


def test_dynamic_indentation_block_end_stops_at_dedent():
    # The helper iterates lines[1:] (the first line after `start` is always
    # implicitly part of the block), skipping blanks/comments, and stops at the
    # first line whose indent <= base_indent.
    p = _PyParser()
    src = "    a = 1\n    b = 2\nx = 9\n"
    end = p._find_indentation_block_end(src, 0, 0)
    # lines[1]="    b = 2" (indent 4 > 0) advances; lines[2]="x = 9" breaks.
    # end_offset = len("    b = 2") + 1 = 10.
    assert end == 10
    assert "x = 9" in src[end:]


def test_dynamic_indentation_block_end_skips_comment_then_breaks():
    # A comment line is skipped (counted into the offset), but a following
    # dedented code line still ends the block.
    p = _PyParser()
    src = "    a = 1\n    # note\nx = 9\n"
    end = p._find_indentation_block_end(src, 0, 0)
    # lines[1]="    # note" is a comment -> counted; lines[2]="x = 9" breaks.
    assert end == len("    # note") + 1
    assert "x = 9" in src[end:]


def test_dynamic_indentation_block_end_no_dedent():
    # No dedented line: the loop consumes the remaining lines and returns the
    # accumulated offset (which stops before the implicit first line span).
    p = _PyParser()
    src = "    a = 1\n    b = 2\n"
    end = p._find_indentation_block_end(src, 0, 0)
    # lines[1]="    b = 2" advances (+10), trailing "" skipped (+1) -> 11.
    assert end == 11


# ---------------------------------------------------------------------------
# Functional / Markup base classes
# ---------------------------------------------------------------------------
def test_functional_patterns():
    assert pu.FunctionalLanguageParser.HASKELL_FUNCTION_PATTERN.search(
        "foo :: Int -> Int"
    )
    assert pu.FunctionalLanguageParser.HASKELL_MODULE_PATTERN.search("module Main")
    assert pu.FunctionalLanguageParser.ELIXIR_MODULE_PATTERN.search("defmodule Foo")
    assert pu.FunctionalLanguageParser.ELIXIR_FUNCTION_PATTERN.search("def hello")


class _Markup(pu.MarkupParser):
    @property
    def language(self):
        return "json"

    @property
    def file_extensions(self):
        return [".json"]

    def parse(self, content, file_path):
        return self._create_empty_result(file_path)

    def _fallback_regex_parse(self, content, file_path):
        return self._create_empty_result(file_path)


def test_markup_unimplemented_methods_raise():
    m = _Markup()
    with pytest.raises(NotImplementedError):
        m._extract_keys("{}")
    with pytest.raises(NotImplementedError):
        m._build_hierarchy("{}")


# ---------------------------------------------------------------------------
# ParserPlugin
# ---------------------------------------------------------------------------
def test_parser_plugin_basic():
    plugin = pu.ParserPlugin(
        name="dummy",
        parser_class=_DummyParser,
        languages=["Dummy"],
        extensions=[".dum", ".txt"],
        priority=5,
        metadata={"k": "v"},
    )
    assert plugin.supports_language("dummy") is True
    assert plugin.supports_language("DUMMY") is True
    assert plugin.supports_language("python") is False
    # supports_extension normalizes the *query* to add a leading dot, but
    # compares against the stored list verbatim. So stored ".txt" matches both
    # ".txt" and "txt" queries; a stored extension lacking the dot would not.
    assert plugin.supports_extension(".dum") is True
    assert plugin.supports_extension("dum") is True
    assert plugin.supports_extension(".txt") is True
    assert plugin.supports_extension("txt") is True
    assert plugin.supports_extension(".py") is False
    assert plugin.metadata == {"k": "v"}


def test_parser_plugin_extension_without_dot_in_store_not_matched():
    # A stored extension lacking a leading dot is never matched because the
    # query is always dot-normalized -- documents the real quirk.
    plugin = pu.ParserPlugin("n", _DummyParser, ["dummy"], ["raw"])
    assert plugin.supports_extension("raw") is False
    assert plugin.supports_extension(".raw") is False


def test_parser_plugin_default_metadata():
    plugin = pu.ParserPlugin("n", _DummyParser, ["dummy"], [".dum"])
    assert plugin.metadata == {}


def test_parser_plugin_get_parser_memoized():
    plugin = pu.ParserPlugin("n", _DummyParser, ["dummy"], [".dum"])
    a = plugin.get_parser()
    b = plugin.get_parser()
    assert a is b
    assert isinstance(a, _DummyParser)


# ---------------------------------------------------------------------------
# ParserPluginRegistry
# ---------------------------------------------------------------------------
def _mk_plugin(name, langs, exts, prio=0):
    return pu.ParserPlugin(name, _DummyParser, langs, exts, priority=prio)


def test_registry_singleton():
    assert pu.ParserPluginRegistry() is pu.get_plugin_registry()


def test_registry_register_and_lookup():
    reg = pu.get_plugin_registry()
    assert reg.register(_mk_plugin("p1", ["dummy"], [".dum"])) is True
    # duplicate name rejected
    assert reg.register(_mk_plugin("p1", ["dummy"], [".dum"])) is False

    parser = reg.get_parser_for_language("DUMMY")
    assert isinstance(parser, _DummyParser)
    parser_ext = reg.get_parser_for_extension("dum")
    assert isinstance(parser_ext, _DummyParser)
    assert "dummy" in reg.get_supported_languages()
    assert ".dum" in reg.get_supported_extensions()


def test_registry_priority_ordering():
    reg = pu.get_plugin_registry()
    reg.register(_mk_plugin("low", ["shared"], [".sh"], prio=1))
    reg.register(_mk_plugin("high", ["shared"], [".sh"], prio=10))
    # the parser instance comes from the highest-priority ('high') plugin
    high_parser = reg._plugins["high"].get_parser()
    assert reg.get_parser_for_language("shared") is high_parser
    assert reg.get_parser_for_extension(".sh") is high_parser


def test_registry_lookup_missing():
    reg = pu.get_plugin_registry()
    assert reg.get_parser_for_language("nonexistent") is None
    assert reg.get_parser_for_extension(".nope") is None


def test_registry_unregister():
    reg = pu.get_plugin_registry()
    reg.register(_mk_plugin("temp", ["templang"], [".tmp"]))
    assert reg.unregister("temp") is True
    # gone from indices
    assert reg.get_parser_for_language("templang") is None
    assert reg.get_parser_for_extension(".tmp") is None
    assert "templang" not in reg.get_supported_languages()
    # unregister again -> False
    assert reg.unregister("temp") is False


def test_registry_list_plugins_shape():
    reg = pu.get_plugin_registry()
    reg.register(_mk_plugin("listme", ["l"], [".l"], prio=3))
    entry = next(p for p in reg.list_plugins() if p["name"] == "listme")
    assert entry["languages"] == ["l"]
    assert entry["extensions"] == [".l"]
    assert entry["priority"] == 3
    assert entry["metadata"] == {}


# ---------------------------------------------------------------------------
# ConfigValidator
# ---------------------------------------------------------------------------
def test_validate_chunk_size_ok():
    r = pu.ConfigValidator.validate_chunk_size(500)
    assert r.valid is True
    assert r.errors == []


def test_validate_chunk_size_below_min():
    r = pu.ConfigValidator.validate_chunk_size(5, min_chunk_size=10)
    assert r.valid is False
    assert any("min_chunk_size" in e for e in r.errors)


def test_validate_chunk_size_above_max():
    r = pu.ConfigValidator.validate_chunk_size(200000, max_chunk_size=100000)
    assert r.valid is False
    assert any("max_chunk_size" in e for e in r.errors)


def test_validate_chunk_size_small_warning():
    r = pu.ConfigValidator.validate_chunk_size(50)
    assert r.valid is True
    assert any("very small" in w for w in r.warnings)


def test_validate_chunk_size_large_warning():
    r = pu.ConfigValidator.validate_chunk_size(20000)
    assert any("large" in w for w in r.warnings)


def test_validate_overlap_ok():
    r = pu.ConfigValidator.validate_overlap(50, 500)
    assert r.valid is True


def test_validate_overlap_negative():
    r = pu.ConfigValidator.validate_overlap(-1, 500)
    assert r.valid is False
    assert any(">= 0" in e for e in r.errors)


def test_validate_overlap_too_large():
    r = pu.ConfigValidator.validate_overlap(600, 500)
    assert r.valid is False
    assert any("< chunk_size" in e for e in r.errors)


def test_validate_overlap_high_redundancy_warning():
    r = pu.ConfigValidator.validate_overlap(300, 500)
    assert r.valid is True
    assert any(">50%" in w for w in r.warnings)


def test_validate_languages():
    pu.get_plugin_registry().register(_mk_plugin("p", ["python"], [".py"]))
    r = pu.ConfigValidator.validate_languages(["python", "unknownlang"])
    assert r.valid is True
    assert any("unknownlang" in w for w in r.warnings)
    assert not any("python" in w for w in r.warnings)


class _Cfg:
    def __init__(self, **kw):
        for k, v in kw.items():
            setattr(self, k, v)


def test_validate_config_full_ok():
    cfg = _Cfg(chunk_size=500, chunk_overlap=50, languages=None)
    r = pu.ConfigValidator.validate_config(cfg)
    assert r.valid is True


def test_validate_config_full_errors():
    cfg = _Cfg(
        chunk_size=5,
        min_chunk_size=10,
        chunk_overlap=10,  # >= chunk_size error too
        languages=["bogus"],
    )
    r = pu.ConfigValidator.validate_config(cfg)
    assert r.valid is False
    assert r.errors  # collected from chunk_size and overlap
    # language warning surfaces
    assert any("bogus" in w for w in r.warnings)


def test_validate_config_no_attrs():
    cfg = _Cfg()
    r = pu.ConfigValidator.validate_config(cfg)
    assert r.valid is True
    assert r.errors == []


def test_validation_result_defaults():
    r = pu.ValidationResult(valid=True)
    assert r.errors == []
    assert r.warnings == []


# ---------------------------------------------------------------------------
# parser_context
# ---------------------------------------------------------------------------
def test_parser_context_cache_miss():
    pu.get_parser_cache().clear()
    with pu.parser_context("python") as parser:
        assert parser is None


def test_parser_context_cache_hit():
    cache = pu.get_parser_cache()
    cache.clear()
    obj = object()
    cache.put("python", obj)
    with pu.parser_context("python") as parser:
        assert parser is obj


# ---------------------------------------------------------------------------
# detect_language_from_content
# ---------------------------------------------------------------------------
@pytest.mark.parametrize(
    "shebang,expected",
    [
        ("#!/usr/bin/env python3\nprint(1)", "python"),
        ("#!/usr/bin/node\nconsole.log(1)", "javascript"),
        ("#!/usr/bin/deno\n", "javascript"),
        ("#!/usr/bin/ruby\nputs 1", "ruby"),
        ("#!/usr/bin/perl\n", "perl"),
        ("#!/bin/bash\n", "bash"),
    ],
)
def test_detect_language_shebang(shebang, expected):
    assert pu.detect_language_from_content(shebang) == expected


def test_detect_language_go():
    src = "package main\n\nfunc main() {}\n"
    assert pu.detect_language_from_content(src) == "go"


def test_detect_language_rust():
    src = "fn main() {\n    let x = 1;\n}"
    assert pu.detect_language_from_content(src) == "rust"


def test_detect_language_java():
    src = "public class Hello {}\n"
    assert pu.detect_language_from_content(src) == "java"


def test_detect_language_python_no_shebang():
    src = "def hello():\n    return 1\n"
    assert pu.detect_language_from_content(src) == "python"


def test_detect_language_javascript():
    src = "function foo() { return 1; }"
    assert pu.detect_language_from_content(src) == "javascript"


def test_detect_language_unknown():
    src = "lorem ipsum dolor sit amet\nno code here at all"
    assert pu.detect_language_from_content(src) is None


def test_detect_language_empty():
    assert pu.detect_language_from_content("") is None


# ---------------------------------------------------------------------------
# Module exports sanity
# ---------------------------------------------------------------------------
def test_all_exports_present():
    for name in pu.__all__:
        assert hasattr(pu, name), name


def test_patterns_are_compiled():
    assert isinstance(pu.CFamilyParser.BLOCK_START, re.Pattern)
    assert isinstance(pu.JVMFamilyParser.PACKAGE_PATTERN, re.Pattern)
