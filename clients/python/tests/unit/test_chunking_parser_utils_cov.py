"""Offline unit tests for proximadb_sdk.chunking_strategies.parser_utils.

Pure module: no network/server. Exercises the live surface that remains after
the dead parallel parser stack was removed: errors, metrics, the
metrics decorator, the BaseLanguageParser skeleton, and config validation.
"""

import pytest

from proximadb_sdk.chunking_strategies import parser_utils as pu


# ---------------------------------------------------------------------------
# Fixtures to reset process-wide singletons (shared state across tests).
# ---------------------------------------------------------------------------
@pytest.fixture(autouse=True)
def _reset_singletons():
    pu.get_metrics_collector().clear()
    pu.get_metrics_collector().enable()
    yield
    pu.get_metrics_collector().clear()


# ---------------------------------------------------------------------------
# Errors
# ---------------------------------------------------------------------------
def test_parser_error_attrs():
    e = pu.ParserError("boom", language="python", file_path="a.py")
    assert e.language == "python"
    assert e.file_path == "a.py"
    assert str(e) == "boom"


def test_parse_error_line_column():
    e = pu.ParseError("bad", line=5, column=3, language="rust", file_path="x.rs")
    assert isinstance(e, pu.ParserError)
    assert e.line == 5
    assert e.column == 3
    assert e.language == "rust"
    assert e.file_path == "x.rs"


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
            language="py",
            file_path="a.py",
            parse_time_ms=10.0,
            symbol_count=2,
            relation_count=1,
            error_count=0,
            cache_hit=True,
        )
    )
    c.record(
        pu.ParserMetrics(
            language="py",
            file_path="b.py",
            parse_time_ms=20.0,
            symbol_count=4,
            relation_count=3,
            error_count=1,
            fallback_used=True,
        )
    )
    c.record(pu.ParserMetrics(language="go", file_path="m.go", parse_time_ms=5.0))
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
# Concrete parser for exercising base-class behavior + the metrics decorator
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
    # tree_sitter has no "dummy" grammar -> fallback path -> _parser None
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
# with_metrics decorator
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


def test_validate_languages_uses_real_registry():
    # validate_languages now consults the live per-language registry from
    # code.py: known languages do not warn; unknown ones do.
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
# Module exports sanity
# ---------------------------------------------------------------------------
def test_all_exports_present():
    for name in pu.__all__:
        assert hasattr(pu, name), name


def test_dead_stack_removed():
    # The parallel parser stack was removed; these must no longer be present.
    for name in (
        "ParserCache",
        "get_parser_cache",
        "ParserPlugin",
        "ParserPluginRegistry",
        "get_plugin_registry",
        "CFamilyParser",
        "JVMFamilyParser",
        "DynamicLanguageParser",
        "FunctionalLanguageParser",
        "MarkupParser",
        "FallbackConfig",
        "FallbackStrategy",
        "with_fallback",
        "cached_parser",
        "parser_context",
        "detect_language_from_content",
        "UnsupportedLanguageError",
        "ParserInitializationError",
    ):
        assert not hasattr(pu, name), f"{name} should have been removed"
