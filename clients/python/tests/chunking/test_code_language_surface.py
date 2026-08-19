"""TD-CG2 R1 — the advertised language surface, as a CI fact.

The audit's finding was not that code chunking was slow or imprecise; it was
that the package advertised "AST-based parsing for 20+ languages" while most of
them returned nothing. A docstring cannot be wrong in CI. This can.

R1 converts "which languages do we support?" into a parameterised assertion over
a minimal valid source per language. The counterpart matters just as much: the
languages that do NOT extract symbols are named too, and asserted to still be
COVERED. Covered-without-symbols and unsupported are different states, and
conflating them is exactly how the overclaim survived.
"""

from __future__ import annotations

import pytest

from proximadb_sdk.chunking_strategies.code import (
    COVERED_WITHOUT_SYMBOLS,
    EXTENSION_TO_LANGUAGE,
    SYMBOL_EXTRACTING_LANGUAGES,
    CodeChunkingConfig,
    CodeChunkingStrategy,
)

#: One minimal, valid source per language: a free function and a type where the
#: language has both, so a parser that finds only one is still visibly partial.
SAMPLES: dict[str, tuple[str, str]] = {
    "python": (
        "t.py",
        "def alpha(x):\n    return x + 1\n\nclass Beta:\n    def gamma(self):\n        return 2\n",
    ),
    "javascript": (
        "t.js",
        "function alpha(x) { return x + 1; }\nclass Beta { gamma() { return 2; } }\n",
    ),
    "typescript": (
        "t.ts",
        "function alpha(x: number): number { return x + 1; }\nclass Beta { gamma(): number { return 2; } }\n",
    ),
    "tsx": (
        "t.tsx",
        "export function Alpha() { return <div/>; }\nexport const Beta = () => <span/>;\nclass Gamma { d() { return 1; } }\n",
    ),
    "go": (
        "t.go",
        "package m\nfunc Alpha(x int) int { return x + 1 }\ntype Beta struct{}\n",
    ),
    "rust": (
        "t.rs",
        "fn alpha(x: i32) -> i32 { x + 1 }\nstruct Beta;\nimpl Beta { fn gamma(&self) {} }\n",
    ),
    "java": ("t.java", "class Beta { int gamma() { return 2; } }\n"),
    "c": ("t.c", "int alpha(int x) { return x + 1; }\n"),
    "cpp": (
        "t.cpp",
        "int alpha(int x) { return x + 1; }\nclass Beta { public: int gamma(); };\n",
    ),
    "csharp": ("t.cs", "class Beta { int Gamma() { return 2; } }\n"),
    "kotlin": (
        "t.kt",
        "fun alpha(x: Int): Int { return x + 1 }\nclass Beta { fun gamma(): Int { return 2 } }\n",
    ),
    "php": (
        "t.php",
        "<?php\nfunction alpha($x) { return $x + 1; }\nclass Beta { function gamma() { return 2; } }\n",
    ),
    "swift": (
        "t.swift",
        "func alpha(x: Int) -> Int { return x + 1 }\nclass Beta { func gamma() -> Int { return 2 } }\n",
    ),
    "scala": (
        "t.scala",
        "object Beta { def gamma(): Int = 2 }\ndef alpha(x: Int): Int = x + 1\n",
    ),
    "lua": ("t.lua", "function alpha(x)\n  return x + 1\nend\n"),
    "bash": ("t.sh", "alpha() {\n  echo 1\n}\n"),
    "ruby": (
        "t.rb",
        "def alpha(x)\n  x + 1\nend\n\nclass Beta\n  def gamma\n    2\n  end\nend\n",
    ),
    "perl": ("t.pl", "sub alpha {\n  return 1;\n}\n"),
    "haskell": ("t.hs", "alpha :: Int -> Int\nalpha x = x + 1\n"),
    "elixir": ("t.ex", "defmodule Beta do\n  def gamma, do: 2\nend\n"),
    "sql": ("t.sql", "CREATE TABLE beta (id INT);\n"),
    "json": ("t.json", '{"alpha": 1, "beta": {"gamma": 2}}\n'),
    "xml": ("t.xml", "<root><alpha>1</alpha></root>\n"),
    "yaml": ("t.yaml", "alpha: 1\nbeta:\n  gamma: 2\n"),
}


def _chunk(language: str):
    filename, source = SAMPLES[language]
    strategy = CodeChunkingStrategy(
        CodeChunkingConfig(chunk_size=400, max_chunk_size=400, min_chunk_size=1)
    )
    return source, strategy.chunk(source, filename)


@pytest.mark.parametrize("language", sorted(SYMBOL_EXTRACTING_LANGUAGES))
def test_r1_advertised_languages_extract_symbols(language):
    """Every language we claim symbol extraction for must actually deliver it."""
    assert language in SAMPLES, f"{language} is advertised but has no sample"
    _, chunks = _chunk(language)
    symbols = [c for c in chunks if c.metadata.get("symbol_id")]
    assert symbols, (
        f"{language} is in SYMBOL_EXTRACTING_LANGUAGES but yielded no symbol. "
        "Either the upstream parser regressed or the claim is wrong -- fix one "
        "of them, do not relax this test."
    )


@pytest.mark.parametrize("language", sorted(COVERED_WITHOUT_SYMBOLS))
def test_unsupported_languages_are_still_covered(language):
    """No symbols is acceptable; losing the file is not.

    This is the assertion that makes the honest split safe to state. A language
    we do not extract symbols for must still be chunked, or "unsupported"
    quietly means "discarded".
    """
    source, chunks = _chunk(language)
    assert chunks, f"{language} produced NO chunks: the document was discarded"
    covered = sum(c.end_pos - c.start_pos for c in chunks)
    assert (
        covered >= len(source.strip()) * 0.5
    ), f"{language} covered only {covered} of {len(source)} characters"


@pytest.mark.parametrize("language", sorted(COVERED_WITHOUT_SYMBOLS))
def test_r8_non_symbol_chunks_are_distinguishable(language):
    """A window over unparsed text must not masquerade as a symbol chunk.

    Without this a consumer building a code graph cannot tell which chunks
    carry structure, so it treats a text window as a symbol and the graph
    silently fills with nodes that mean nothing.
    """
    _, chunks = _chunk(language)
    for chunk in chunks:
        assert chunk.metadata.get("chunk_type") in {"code_window", "code_fallback"}, (
            f"{language}: a symbol-less chunk is labelled "
            f"{chunk.metadata.get('chunk_type')!r}, indistinguishable from a symbol"
        )


def test_the_two_sets_partition_the_advertised_map():
    """The claim and the gap together must account for everything advertised.

    Otherwise a language can be dropped from both and vanish from the record --
    which is how an overclaim becomes invisible rather than false.
    """
    advertised = set(EXTENSION_TO_LANGUAGE.values())
    accounted = SYMBOL_EXTRACTING_LANGUAGES | COVERED_WITHOUT_SYMBOLS
    assert (
        not advertised - accounted
    ), f"unaccounted languages: {advertised - accounted}"
    assert not SYMBOL_EXTRACTING_LANGUAGES & COVERED_WITHOUT_SYMBOLS


def test_every_advertised_language_has_a_sample():
    # Otherwise R1's coverage silently shrinks when a language is added.
    advertised = set(EXTENSION_TO_LANGUAGE.values())
    missing = advertised - set(SAMPLES)
    assert not missing, f"advertised languages with no R1 sample: {missing}"


def test_r6_include_tests_is_forwarded():
    """`include_tests=False` must actually exclude test files.

    The shared package implements it (2 chunks -> 0 on a test module); this
    module simply never passed it on, so the flag was accepted and ignored. A
    caller asking to exclude tests from its index got every one of them.
    """
    source = "def test_alpha():\n    assert True\n\ndef test_beta():\n    assert 1\n"

    included = CodeChunkingStrategy(
        CodeChunkingConfig(include_tests=True, min_chunk_size=1)
    ).chunk(source, "test_foo.py")
    assert included, "precondition: test files are chunked when included"

    excluded = CodeChunkingStrategy(
        CodeChunkingConfig(include_tests=False, min_chunk_size=1)
    ).chunk(source, "test_foo.py")
    assert not excluded, (
        "include_tests=False returned chunks, so the flag is still being "
        "accepted and ignored"
    )
