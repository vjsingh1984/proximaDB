"""
Code-aware chunking strategy using Tree-sitter for AST parsing.

.. deprecated:: TD-CG2 (ADR-029)
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

import os
import warnings
from dataclasses import dataclass, field
from enum import IntEnum
from typing import Any

from .base import (
    OFFSET_BASIS_BYTE,
    OFFSET_CONTRACT_EXACT,
    ChunkingConfig,
    ChunkingStrategyInterface,
    TextChunk,
)

# Optional delegation target (TD-CG2). Imported softly so the SDK keeps working without
# the `codegraph` extra; when present, it is the single source of truth for code chunking.
try:  # pragma: no cover - availability depends on the optional extra
    import victor_codegraph as _victor_codegraph
except Exception:  # ImportError, or a partial/native load failure
    _victor_codegraph = None


def _warn_code_chunker_deprecated() -> None:
    """Steer callers toward the shared ``victor-codegraph`` package (ADR-029 / TD-CG2)."""

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


# ============================================================================
# Language surface — DERIVED from the shared package, not owned here.
#
# TD-CG2 S4 removed 21 in-SDK tree-sitter parser classes, the `LanguageParser`
# ABC, the `LANGUAGE_PARSERS` registry and this module's own
# `EXTENSION_TO_LANGUAGE` table from between these lines. ADR-029 decided that
# deletion: which node type names a function in the Kotlin grammar is knowledge
# about a language ecosystem, and it belongs to the package that owns a Code
# Property Graph, not to a database client.
#
# What was deleted was also non-functional. Every tree-sitter path raised
# against the pinned dependency, 10 of 23 registered languages were stubs
# returning nothing, and exactly one worked. A duplicate that works is a
# maintenance cost; a duplicate that is broken while the package advertises it
# is a correctness trap.
#
# The map is now derived rather than mirrored, so it cannot drift from the
# implementation that actually parses. Deriving is the point: a hand-maintained
# copy is how "20+ languages" outlived the parsers that were meant to back it.
# ============================================================================


def _victor_extension_map() -> dict[str, str]:
    """The shared package's extension map, or empty when it is not installed.

    Empty rather than a hardcoded fallback: an extension map with no parser
    behind it is exactly the overclaim this slice removed.
    """
    if _victor_codegraph is None:
        return {}
    try:
        from victor_codegraph import languages as _languages

        return dict(_languages.EXTENSION_TO_LANGUAGE)
    except Exception:  # noqa: BLE001 - absence is a supported state
        return {}


#: Extensions the shared package does not map, whose LANGUAGE it nevertheless
#: parses. Mapping `.pyx` onto the working Python parser adds reach; it is not
#: the overclaim this slice removed, because the parser behind it is real.
#:
#: The distinction is enforced, not asserted: a test requires every value here
#: to be a language the installed package actually supports, so this overlay
#: cannot quietly grow back into a table of languages nothing can parse.
_EXTENSION_ALIASES: dict[str, str] = {
    ".fish": "bash",
    ".gemspec": "ruby",
    ".hh": "cpp",
    ".hxx": "cpp",
    ".ksh": "bash",
    ".kts": "kotlin",
    ".mysql": "sql",
    ".phtml": "php",
    ".psql": "sql",
    ".pyx": "python",
    ".rake": "ruby",
    ".sc": "scala",
    ".zsh": "bash",
}


def _build_extension_map() -> dict[str, str]:
    """Derived map, plus aliases that resolve to languages it already supports."""
    base = _victor_extension_map()
    if not base:
        return {}
    supported = set(base.values())
    return {
        **base,
        **{ext: lang for ext, lang in _EXTENSION_ALIASES.items() if lang in supported},
    }


#: File extension -> language. Derived; see above. Kept as a module-level name
#: because `code_knowledge.py` and `repository_indexer.py` read it through
#: `get_supported_extensions()` to decide which files to index.
EXTENSION_TO_LANGUAGE: dict[str, str] = _build_extension_map()


def get_supported_languages() -> list[str]:
    """Languages the installed parser package can identify."""
    return sorted(set(EXTENSION_TO_LANGUAGE.values()))


def get_supported_extensions() -> list[str]:
    """File extensions the installed parser package can identify."""
    return list(EXTENSION_TO_LANGUAGE.keys())


#: Languages that genuinely extract SYMBOLS through the delegated path, as
#: opposed to being merely covered by window chunks. Measured, and asserted by
#: the R1 test in `tests/chunking/test_code_language_surface.py` -- so "which
#: languages do we support?" is a CI fact rather than a docstring claim, which
#: is what TD-CG2 found the whole ecosystem was missing.
#:
#: The remainder of EXTENSION_TO_LANGUAGE is still detected and still chunked;
#: those files are covered by window or fallback chunks and simply carry no
#: symbol metadata. Covered-without-symbols and unsupported are different
#: states, and conflating them is what let "20+ languages" stand unchallenged.
SYMBOL_EXTRACTING_LANGUAGES: frozenset[str] = frozenset(
    {
        "bash",
        "c",
        "cpp",
        "csharp",
        "go",
        "java",
        "javascript",
        "kotlin",
        "lua",
        "php",
        "python",
        "rust",
        "scala",
        "swift",
        "tsx",
        "typescript",
    }
)

#: Detected and chunked, but yielding no symbols today. Named explicitly so the
#: gap is visible in code rather than discovered by a user. `ruby` and `perl`
#: are upstream gaps in the shared package's node tables; `haskell` and `elixir`
#: have no grammar entry there at all; `json`, `xml` and `yaml` are data
#: formats, for which "symbol" is not a meaningful notion.
COVERED_WITHOUT_SYMBOLS: frozenset[str] = frozenset(
    {"ruby", "perl", "haskell", "elixir", "sql", "json", "xml", "yaml"}
)


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
        # Languages this strategy is allowed to parse (config-scoped). The set
        # comes from the installed parser package, so it cannot claim a language
        # nothing can actually parse.
        self._allowed_languages: set[str] = set(
            self.config.languages or get_supported_languages()
        )

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
        explicit_language = metadata.get("language")

        # The shared package is now the only implementation. Pass only an
        # EXPLICIT caller override, never an extension guess of our own
        # (TD-CG2 R7): this module used to map `.tsx` to "typescript" and
        # override the package's own `.tsx -> tsx` detection, costing HALF the
        # symbols in the file (measured 2 of 4).
        if _victor_codegraph is None:
            raise RuntimeError(
                "code chunking requires the 'codegraph' extra, which is not "
                "installed. Install `proximadb[codegraph]`. "
                "(The in-SDK tree-sitter parsers were removed in TD-CG2 / "
                "ADR-029; they had been non-functional against the pinned "
                "dependency.) Failing here is deliberate: silently returning "
                "text windows would look like working code chunking while "
                "extracting no symbols at all."
            )
        return self._chunk_via_victor_codegraph(
            text, source_id, explicit_language, metadata
        )

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
            # TD-CG2 R6: forwarded, because the shared package implements it and
            # this module never passed it on -- so `include_tests=False` was
            # accepted and ignored, and a caller excluding test files got them
            # all anyway. Third instance of the same class in this file, after
            # `extract_relations` and the dropped size fields: a config field
            # that is read by nobody is indistinguishable from one left unset.
            include_tests=self.config.include_tests,
            extract_relations=self.config.extract_relations,
        )
        # Relations are NOT surfaced by the shared package's `chunk()` entry
        # point -- they come from `parse()`. Passing `extract_relations` to the
        # chunk config therefore accepted the flag and did nothing, and
        # `code_knowledge.py` builds its graph EDGES from
        # `chunk.metadata["relations"]`: the delegation left the code knowledge
        # graph with nodes and no edges, silently.
        #
        # Restoring them costs a second parse (measured +66% on this path). That
        # is the right trade here: the co-design mandate is explicit that CPU is
        # not this system's dominant cost term, and a knowledge graph with no
        # edges is not a cheaper graph, it is a broken one. Gated on the flag, so
        # a caller that does not want relations does not pay for them.
        relations_by_symbol: dict[str, list[dict[str, Any]]] = {}
        if self.config.extract_relations:
            try:
                parsed = _victor_codegraph.parse(
                    text, language=language, file_path=source_id
                )
            except Exception:  # noqa: BLE001 - relations are enrichment, not the chunk
                parsed = None
            if parsed is not None:
                for relation in getattr(parsed, "relations", ()):
                    relations_by_symbol.setdefault(relation.from_symbol_id, []).append(
                        {
                            "to": relation.to_symbol_id,
                            "type": getattr(
                                relation.relation_type,
                                "name",
                                str(relation.relation_type),
                            ),
                            "confidence": getattr(relation, "confidence", 1.0),
                        }
                    )

        out: list[TextChunk] = []
        for c in _victor_codegraph.chunk(
            text, language=language, file_path=source_id, config=cfg
        ):
            meta = {**metadata, **c.metadata}
            meta.setdefault("chunking_strategy", "code")
            # TD-CG2 R8: a caller must be able to tell a real symbol from a
            # window over a language nothing understood. Labelling everything
            # "code" made those indistinguishable, so a consumer building a code
            # graph could not tell which chunks carry structure. Three states,
            # because they are three different things: a symbol, a window the
            # shared package emitted over code it could not attribute, and our
            # own text fallback for file types it does not handle at all.
            meta["chunk_type"] = (
                "code" if c.metadata.get("symbol_id") else "code_window"
            )
            meta["source"] = "victor_codegraph"
            # TD-CG2 R5. These offsets are UTF-8 BYTE offsets, while every text
            # strategy fills the same field with CHARACTER offsets -- one type,
            # two incompatible units. A consumer slicing a Python `str` with a
            # byte offset silently corrupts any non-ASCII source, and until now
            # there was no way to tell which it held.
            #
            # `document_processor` persists these values, so the unit is a
            # STORED contract. The marker is therefore additive and readers must
            # treat its ABSENCE as legacy: mixed-read-safe, no flag day.
            meta["offset_basis"] = OFFSET_BASIS_BYTE
            meta["offset_contract"] = OFFSET_CONTRACT_EXACT
            symbol_relations = relations_by_symbol.get(meta.get("symbol_id", ""))
            if symbol_relations:
                meta["relations"] = symbol_relations
            out.append(
                TextChunk(
                    text=c.text,
                    start_pos=c.start_pos,
                    end_pos=c.end_pos,
                    chunk_id=c.chunk_id,
                    metadata=meta,
                )
            )
        if (
            not out
            and text.strip()
            and not self._recognised_by_victor(source_id, language)
        ):
            # The shared package knows nothing about this file type -- it covers
            # far fewer extensions than this module advertises. Returning its
            # empty list SILENTLY DISCARDS the whole document, which is the
            # defect class ADR-091 exists to remove, and it is a regression the
            # delegation introduced: the legacy path fell through to text
            # chunking here. Fall back, and LABEL it, so a caller can tell a
            # real symbol chunk from a text window (TD-CG2 R8).
            #
            # Gated on "could not handle it", NOT on "returned nothing". Those
            # are different, and conflating them defeats every deliberate
            # exclusion: with `include_tests=False` the package correctly
            # returns nothing for a test file, and an ungated fallback would
            # text-chunk it straight back in.
            return self._fallback_chunk(text, source_id, metadata)
        return out

    @staticmethod
    def _recognised_by_victor(source_id: str, explicit_language: str | None) -> bool:
        """True when the shared package can identify this file's language.

        The distinction that makes the fallback safe: a package that RECOGNISES
        a file and returns no chunks has made a decision, and overriding it
        would silently undo the caller's configuration.
        """
        if explicit_language:
            return True
        try:
            return _victor_codegraph.detect_language(source_id) is not None
        except Exception:  # noqa: BLE001 - detection is advisory
            return False

    def _detect_language(self, file_path: str) -> str | None:
        """Detect language from file extension using global registry"""
        ext = os.path.splitext(file_path)[1].lower()
        return EXTENSION_TO_LANGUAGE.get(ext)

    def _fallback_chunk(
        self, text: str, source_id: str, metadata: dict[str, Any]
    ) -> list[TextChunk]:
        """Fallback to simple text chunking when parser not available"""
        from .semantic import SemanticStrategy

        # Forward every size field. Naming a couple by hand is the same
        # dropped-config bug this package already fixed in three other places:
        # a field left out is indistinguishable from one left unset, so the
        # fallback would quietly run on defaults rather than the caller's
        # configuration.
        fallback_config = ChunkingConfig(
            chunk_size=self.config.chunk_size,
            chunk_overlap=self.config.chunk_overlap,
            min_chunk_size=self.config.min_chunk_size,
            max_chunk_size=self.config.max_chunk_size,
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
