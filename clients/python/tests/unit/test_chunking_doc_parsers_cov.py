"""Offline coverage tests for chunking_strategies.document_parsers.

Fully offline: no real RE/OCR tools, no subprocess execution against real
binaries. subprocess.run, shutil.which, and os.path.exists are monkeypatched so
nothing touches the host's tool chain.
"""

import subprocess
import types

import pytest

from proximadb_sdk.chunking_strategies import document_parsers as dp
from proximadb_sdk.chunking_strategies.document_parsers import (
    BinaryAnalysis,
    BinaryParser,
    BinaryParserConfig,
    BinarySymbol,
    BinaryType,
    DocumentParser,
    DocumentType,
    ObjdumpAdapter,
    OCRConfig,
    OCRResult,
    Radare2Adapter,
    TesseractAdapter,
    ToolDetector,
    create_binary_parser,
    create_document_parser,
    detect_binary_type,
    detect_document_type,
    get_available_tools,
)
from proximadb_sdk.chunking_strategies.parser_utils import ParseError, ParserError


def _elf(e_type: int) -> bytes:
    """Build a minimal ELF header with e_type at offset 16."""
    buf = bytearray(0x40)
    buf[0:4] = b"\x7fELF"
    buf[16:18] = e_type.to_bytes(2, "little")
    return bytes(buf)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


class FakeCompleted:
    def __init__(self, stdout="", returncode=0):
        self.stdout = stdout
        self.returncode = returncode
        self.stderr = ""


@pytest.fixture(autouse=True)
def _clear_tool_cache():
    """Each test starts with an empty ToolDetector cache."""
    ToolDetector._cache.clear()
    yield
    ToolDetector._cache.clear()


def _which_none(_name):
    return None


def _which_some(present):
    """Return a fake shutil.which that resolves only names in `present`."""

    def which(name):
        return f"/usr/bin/{name}" if name in present else None

    return which


# ---------------------------------------------------------------------------
# Binary type detection
# ---------------------------------------------------------------------------


def _write(tmp_path, name, data: bytes):
    p = tmp_path / name
    p.write_bytes(data)
    return str(p)


def test_detect_pe_dll(tmp_path):
    # MZ header + PE header at offset 0x80 with DLL characteristic bit.
    buf = bytearray(0x100)
    buf[0:2] = b"MZ"
    pe_off = 0x80
    buf[0x3C:0x40] = pe_off.to_bytes(4, "little")
    buf[pe_off:pe_off + 4] = b"PE\x00\x00"
    buf[pe_off + 22:pe_off + 24] = (0x2000).to_bytes(2, "little")
    path = _write(tmp_path, "lib.dll", bytes(buf))
    assert detect_binary_type(path) == BinaryType.PE_DLL


def test_detect_pe_exe(tmp_path):
    buf = bytearray(0x100)
    buf[0:2] = b"MZ"
    pe_off = 0x80
    buf[0x3C:0x40] = pe_off.to_bytes(4, "little")
    buf[pe_off:pe_off + 4] = b"PE\x00\x00"
    buf[pe_off + 22:pe_off + 24] = (0x0000).to_bytes(2, "little")
    path = _write(tmp_path, "app.exe", bytes(buf))
    assert detect_binary_type(path) == BinaryType.PE_EXE


def test_detect_elf_exec(tmp_path):
    buf = bytearray(0x40)
    buf[0:4] = b"\x7fELF"
    buf[16:18] = (2).to_bytes(2, "little")  # ET_EXEC
    path = _write(tmp_path, "prog", bytes(buf))
    assert detect_binary_type(path) == BinaryType.ELF_EXEC


def test_detect_elf_so(tmp_path):
    buf = bytearray(0x40)
    buf[0:4] = b"\x7fELF"
    buf[16:18] = (3).to_bytes(2, "little")  # ET_DYN
    path = _write(tmp_path, "lib.so", bytes(buf))
    assert detect_binary_type(path) == BinaryType.ELF_SO


def test_detect_macho_dylib(tmp_path):
    path = _write(tmp_path, "lib.dylib", b"\xfe\xed\xfa\xce" + b"\x00" * 16)
    assert detect_binary_type(path) == BinaryType.MACHO_DYLIB


def test_detect_macho_exec(tmp_path):
    path = _write(tmp_path, "mac_bin", b"\xcf\xfa\xed\xfe" + b"\x00" * 16)
    assert detect_binary_type(path) == BinaryType.MACHO_EXEC


def test_detect_unknown_binary(tmp_path):
    path = _write(tmp_path, "data.bin", b"NOPE" + b"\x00" * 16)
    assert detect_binary_type(path) == BinaryType.UNKNOWN


def test_detect_binary_missing_file_returns_unknown():
    # open() raises -> caught -> UNKNOWN
    assert detect_binary_type("/nonexistent/path/xyz.bin") == BinaryType.UNKNOWN


# ---------------------------------------------------------------------------
# Document type detection
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "name,expected",
    [
        ("a.pdf", DocumentType.PDF),
        ("a.tiff", DocumentType.TIFF),
        ("a.tif", DocumentType.TIFF),
        ("a.png", DocumentType.PNG),
        ("a.jpg", DocumentType.JPEG),
        ("a.jpeg", DocumentType.JPEG),
        ("a.bmp", DocumentType.BMP),
        ("a.webp", DocumentType.WEBP),
        ("a.txt", DocumentType.UNKNOWN),
    ],
)
def test_detect_document_type(name, expected):
    assert detect_document_type(name) == expected


# ---------------------------------------------------------------------------
# ToolDetector
# ---------------------------------------------------------------------------


def test_tool_detector_find_and_cache(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"objdump"}))
    assert ToolDetector.find_tool("objdump") == "/usr/bin/objdump"
    assert ToolDetector.is_available("objdump") is True
    assert ToolDetector.find_tool("nope") is None
    assert ToolDetector.is_available("nope") is False
    # second call hits cache (which would still be fine, but verify cached key)
    assert "objdump" in ToolDetector._cache


def test_tool_detector_re_and_ocr_lists(monkeypatch):
    monkeypatch.setattr(
        dp.shutil, "which", _which_some({"r2", "objdump", "tesseract", "pdftotext"})
    )
    re_tools = ToolDetector.get_available_re_tools()
    assert "r2" in re_tools and "objdump" in re_tools
    ocr_tools = ToolDetector.get_available_ocr_tools()
    assert "tesseract" in ocr_tools and "pdftotext" in ocr_tools


def test_tool_detector_has_wine_true(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"wine"}))
    assert ToolDetector.has_wine() is True


def test_tool_detector_has_wine_false(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    assert ToolDetector.has_wine() is False


def test_tool_detector_system_info(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    info = ToolDetector.get_system_info()
    assert set(info) == {"platform", "machine", "re_tools", "ocr_tools", "has_wine"}
    assert info["re_tools"] == [] and info["ocr_tools"] == []


def test_get_available_tools(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"tesseract"}))
    tools = get_available_tools()
    assert "tesseract" in tools["ocr_tools"]
    assert "re_tools" in tools and "platform" in tools


# ---------------------------------------------------------------------------
# Radare2Adapter
# ---------------------------------------------------------------------------


def test_radare2_not_available(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    a = Radare2Adapter()
    assert a.tool_name == "radare2"
    assert a.is_available() is False


def test_radare2_analyze_missing_tool(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    path = _write(tmp_path, "x.so", _elf(3))
    a = Radare2Adapter()
    with pytest.raises(ParserError):
        a.analyze(path, BinaryParserConfig())


def test_radare2_analyze_success(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"r2"}))
    path = _write(tmp_path, "x.so", _elf(3))

    outputs = {
        "iI": "arch x86\nbits 64\n",
        "ie": "vaddr=0x1000 entry0\n",
        "is": "0x1000 1 32 func .text main\n",
        "ii": "0x2000 1 imp printf\n",
        "iE": "0x3000 1 exp my_export\n",
        "iz~[2:]": "hello world\nfoo\n",
        "iS": "0x0 4096 .text\n",
    }

    def fake_run(cmd, **kw):
        # cmd is [r2_path, "-q", "-c", <command>, file]
        sub = cmd[3]
        return FakeCompleted(stdout=outputs.get(sub, ""))

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    a = Radare2Adapter()
    assert a.is_available() is True
    res = a.analyze(path, BinaryParserConfig())
    assert isinstance(res, BinaryAnalysis)
    assert res.architecture == "x86"
    assert res.entry_point == "0x1000"
    assert res.binary_type == BinaryType.ELF_SO
    assert any(s.name == "main" for s in res.symbols)
    assert "printf" in res.imports
    assert "my_export" in res.exports
    assert "hello world" in res.strings
    assert res.metadata["tool"] == "radare2"
    assert len(res.content_hash) == 64


def test_radare2_analyze_timeout(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"r2"}))
    path = _write(tmp_path, "x.so", b"\x7fELF" + b"\x00" * 16)

    def fake_run(cmd, **kw):
        raise subprocess.TimeoutExpired(cmd, 60)

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    a = Radare2Adapter()
    with pytest.raises(ParseError):
        a.analyze(path, BinaryParserConfig())


def test_radare2_analyze_generic_error(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"r2"}))
    path = _write(tmp_path, "x.so", b"\x7fELF" + b"\x00" * 16)

    def fake_run(cmd, **kw):
        raise RuntimeError("boom")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    a = Radare2Adapter()
    with pytest.raises(ParseError):
        a.analyze(path, BinaryParserConfig())


# ---------------------------------------------------------------------------
# ObjdumpAdapter
# ---------------------------------------------------------------------------


def test_objdump_availability(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"objdump"}))
    a = ObjdumpAdapter()
    assert a.tool_name == "objdump"
    assert a.is_available() is True


def test_objdump_analyze_success(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"objdump", "strings"}))
    path = _write(tmp_path, "x.so", b"\x7fELF" + bytes([0, 0, 3, 0]) + b"\x00" * 16)

    def fake_run(cmd, **kw):
        prog = cmd[0]
        if prog == "strings":
            return FakeCompleted(stdout="alpha\nbeta\n\n")
        flag = cmd[1]
        if flag == "-f":
            return FakeCompleted(stdout="architecture: i386:x86-64, flags 0x...\n")
        if flag == "-t":
            return FakeCompleted(stdout="0000000000001000 g F .text 0000 main\n")
        if flag == "-T":
            return FakeCompleted(
                stdout=(
                    "0000000000000000 DF *UND* 0000 printf\n"
                    "0000000000002000 g DF .text 0000 my_export\n"
                )
            )
        if flag == "-h":
            return FakeCompleted(stdout="  0 .text 00001000 00400000\n")
        return FakeCompleted(stdout="")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    a = ObjdumpAdapter()
    res = a.analyze(path, BinaryParserConfig())
    assert res.architecture == "i386:x86-64,"
    assert any(s.name == "main" for s in res.symbols)
    assert "printf" in res.imports
    assert "my_export" in res.exports
    assert res.sections and res.sections[0]["name"] == ".text"
    assert "alpha" in res.strings and "" not in res.strings
    assert res.metadata["tool"] == "objdump"


def test_objdump_analyze_no_strings_tool(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"objdump"}))
    path = _write(tmp_path, "x.so", b"\x7fELF" + b"\x00" * 16)
    monkeypatch.setattr(dp.subprocess, "run", lambda cmd, **kw: FakeCompleted(stdout=""))
    a = ObjdumpAdapter()
    res = a.analyze(path, BinaryParserConfig())
    assert res.strings == []


def test_objdump_analyze_timeout(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"objdump"}))
    path = _write(tmp_path, "x.so", b"\x7fELF" + b"\x00" * 16)

    def fake_run(cmd, **kw):
        raise subprocess.TimeoutExpired(cmd, 30)

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    a = ObjdumpAdapter()
    with pytest.raises(ParseError):
        a.analyze(path, BinaryParserConfig())


def test_objdump_analyze_generic_error(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"objdump"}))
    path = _write(tmp_path, "x.so", b"\x7fELF" + b"\x00" * 16)

    def fake_run(cmd, **kw):
        raise RuntimeError("kaboom")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    a = ObjdumpAdapter()
    with pytest.raises(ParseError):
        a.analyze(path, BinaryParserConfig())


# ---------------------------------------------------------------------------
# TesseractAdapter
# ---------------------------------------------------------------------------


def test_tesseract_availability(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"tesseract"}))
    a = TesseractAdapter()
    assert a.tool_name == "tesseract"
    assert a.is_available() is True


def test_tesseract_ocr_image(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"tesseract"}))
    path = _write(tmp_path, "scan.png", b"\x89PNG\r\n\x1a\n" + b"\x00" * 16)

    tsv = (
        "level\tpage\tblock\tpar\tline\tword\tleft\ttop\twidth\theight\tconf\ttext\n"
        "5\t1\t1\t1\t1\t1\t0\t0\t10\t10\t90.0\tHello\n"
        "5\t1\t1\t1\t1\t2\t0\t0\t10\t10\t80.0\tWorld\n"
    )

    def fake_run(cmd, **kw):
        if "tsv" in cmd:
            return FakeCompleted(stdout=tsv, returncode=0)
        return FakeCompleted(stdout="Hello World\n")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    a = TesseractAdapter()
    res = a.extract_text(path, OCRConfig())
    assert isinstance(res, OCRResult)
    assert res.text == "Hello World"
    assert res.document_type == DocumentType.PNG
    assert res.confidence == pytest.approx(85.0)
    assert res.pages[0]["page"] == 1
    assert res.metadata["tool"] == "tesseract"


def test_tesseract_unsupported_type(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"tesseract"}))
    path = _write(tmp_path, "data.txt", b"plain text")
    a = TesseractAdapter()
    with pytest.raises(ParseError):
        a.extract_text(path, OCRConfig())


def test_tesseract_pdf_via_pdftotext(monkeypatch, tmp_path):
    # No pdftoppm; pdftotext path.
    monkeypatch.setattr(dp.shutil, "which", _which_some({"tesseract", "pdftotext"}))
    path = _write(tmp_path, "doc.pdf", b"%PDF-1.4\n%%EOF")

    def fake_run(cmd, **kw):
        if cmd[0] == "pdftotext":
            return FakeCompleted(stdout="page one text\n")
        return FakeCompleted(stdout="")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    a = TesseractAdapter()
    res = a.extract_text(path, OCRConfig())
    assert res.document_type == DocumentType.PDF
    assert "page one text" in res.text
    assert res.confidence == 100.0
    assert res.metadata["page_count"] == 1


def test_tesseract_pdf_via_pdftoppm(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"tesseract", "pdftoppm"}))
    path = _write(tmp_path, "doc.pdf", b"%PDF-1.4\n%%EOF")

    created = {}

    def fake_run(cmd, **kw):
        if cmd[0] == "pdftoppm":
            # Simulate writing one page image into the temp dir prefix.
            prefix = cmd[-1]
            import os as _os

            img = prefix + "-1.png"
            with open(img, "wb") as fh:
                fh.write(b"\x89PNG\r\n\x1a\n")
            created["img"] = img
            return FakeCompleted(stdout="")
        # tesseract calls on the page image
        if "tsv" in cmd:
            return FakeCompleted(
                stdout="a\tb\tc\td\te\tf\tg\th\ti\tj\t75.0\tHi\n", returncode=0
            )
        return FakeCompleted(stdout="Hi there\n")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    a = TesseractAdapter()
    res = a.extract_text(path, OCRConfig())
    assert res.document_type == DocumentType.PDF
    assert "Hi there" in res.text
    assert res.metadata["page_count"] == 1


def test_tesseract_pdf_no_tools(monkeypatch, tmp_path):
    # tesseract present but neither pdftoppm nor pdftotext
    monkeypatch.setattr(dp.shutil, "which", _which_some({"tesseract"}))
    path = _write(tmp_path, "doc.pdf", b"%PDF-1.4\n%%EOF")
    monkeypatch.setattr(dp.subprocess, "run", lambda cmd, **kw: FakeCompleted())
    a = TesseractAdapter()
    with pytest.raises(ParseError):
        a.extract_text(path, OCRConfig())


# ---------------------------------------------------------------------------
# BinaryParser
# ---------------------------------------------------------------------------


def _stub_analysis(file_path):
    return BinaryAnalysis(
        file_path=file_path,
        binary_type=BinaryType.ELF_SO,
        architecture="x86",
        symbols=[
            BinarySymbol(name="main", address="0x1000", symbol_type="func"),
            BinarySymbol(name="gvar", address="0x2000", symbol_type="data"),
            BinarySymbol(name="imp", address="0x3000", symbol_type="import"),
        ],
        imports=["printf"],
        exports=["main"],
        strings=["hello"],
        sections=[],
        content_hash="abc",
    )


class _StubAdapter:
    tool_name = "stub"

    def __init__(self, fail=False):
        self.fail = fail

    def is_available(self):
        return True

    def analyze(self, file_path, config):
        if self.fail:
            raise ParseError("stub failed", file_path=file_path)
        return _stub_analysis(file_path)


def test_binary_parser_properties(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    p = BinaryParser()
    assert p.language == "binary"
    assert p.tree_sitter_language_name == "binary"
    assert ".exe" in p.file_extensions
    assert p.has_tree_sitter is False


def test_binary_parser_init_adapters(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"r2", "objdump"}))
    p = BinaryParser()
    names = {a.tool_name for a in p._adapters}
    assert "radare2" in names and "objdump" in names


def test_binary_parser_parse_success(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    p = create_binary_parser()
    p._adapters = [_StubAdapter()]
    path = _write(tmp_path, "x.so", b"\x7fELF")
    parsed = p.parse("", path)
    assert parsed.language == "binary"
    assert parsed.content_hash == "abc"
    assert len(parsed.symbols) == 3
    assert "printf" in parsed.imports


def test_binary_parser_parse_file_not_found(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    p = BinaryParser()
    with pytest.raises(ParseError):
        p.parse("", "/no/such/file.so")


def test_binary_parser_parse_adapter_failure_raises_last_error(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    p = BinaryParser()
    p._adapters = [_StubAdapter(fail=True)]
    path = _write(tmp_path, "x.so", b"\x7fELF")
    with pytest.raises(ParseError):
        p.parse("", path)


def test_binary_parser_parse_no_adapters(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    p = BinaryParser()
    p._adapters = []
    path = _write(tmp_path, "x.so", b"\x7fELF")
    with pytest.raises(ParseError):
        p.parse("", path)


def test_binary_parser_fallback_and_map(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    from proximadb_sdk.chunking_strategies.code import CodeSymbolType

    p = BinaryParser()
    res = p._fallback_regex_parse("content", "f.so")
    assert res.symbols == []
    assert res.language == "binary"
    assert p._map_symbol_type("func") == CodeSymbolType.FUNCTION
    assert p._map_symbol_type("data") == CodeSymbolType.VARIABLE
    assert p._map_symbol_type("import") == CodeSymbolType.MODULE
    assert p._map_symbol_type("export") == CodeSymbolType.FUNCTION
    assert p._map_symbol_type("weirdtype") == CodeSymbolType.VARIABLE


def test_binary_parser_get_analysis(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    p = BinaryParser()
    p._adapters = [_StubAdapter()]
    path = _write(tmp_path, "x.so", b"\x7fELF")
    res = p.get_analysis(path)
    assert res.architecture == "x86"


def test_binary_parser_get_analysis_all_fail(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    p = BinaryParser()
    p._adapters = [_StubAdapter(fail=True)]
    path = _write(tmp_path, "x.so", b"\x7fELF")
    with pytest.raises(ParseError):
        p.get_analysis(path)


# ---------------------------------------------------------------------------
# DocumentParser
# ---------------------------------------------------------------------------


class _StubOCR:
    def __init__(self, pages=None):
        self._pages = pages

    def extract_text(self, file_path, config):
        pages = self._pages
        if pages is None:
            pages = [
                {"page": 1, "text": "first page text", "confidence": 90.0},
                {"page": 2, "text": "   ", "confidence": 0.0},  # blank -> skipped
            ]
        return OCRResult(
            file_path=file_path,
            document_type=DocumentType.PDF,
            text="combined",
            pages=pages,
            confidence=90.0,
            content_hash="dochash",
        )


def test_document_parser_properties(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    p = DocumentParser()
    assert p.language == "document"
    assert p.tree_sitter_language_name == "document"
    assert ".pdf" in p.file_extensions
    assert p.has_ocr is False  # no tesseract


def test_document_parser_has_ocr_true(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_some({"tesseract"}))
    p = DocumentParser()
    assert p.has_ocr is True


def test_document_parser_parse_success(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    p = create_document_parser()
    p._adapter = _StubOCR()
    path = _write(tmp_path, "doc.pdf", b"%PDF")
    parsed = p.parse("", path)
    assert parsed.language == "document"
    assert parsed.content_hash == "dochash"
    # blank page is skipped
    assert len(parsed.symbols) == 1
    assert parsed.symbols[0].simple_name == "Page 1"


def test_document_parser_parse_file_not_found(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    p = DocumentParser()
    p._adapter = _StubOCR()
    with pytest.raises(ParseError):
        p.parse("", "/no/such/doc.pdf")


def test_document_parser_parse_no_adapter(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    p = DocumentParser()
    p._adapter = None
    path = _write(tmp_path, "doc.pdf", b"%PDF")
    with pytest.raises(ParseError):
        p.parse("", path)


def test_document_parser_fallback(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    p = DocumentParser()
    res = p._fallback_regex_parse("content", "doc.pdf")
    assert res.symbols == []
    assert res.language == "document"


def test_document_parser_get_ocr_result(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    p = DocumentParser()
    p._adapter = _StubOCR()
    path = _write(tmp_path, "doc.pdf", b"%PDF")
    res = p.get_ocr_result(path)
    assert res.content_hash == "dochash"


def test_document_parser_get_ocr_result_no_adapter(monkeypatch, tmp_path):
    monkeypatch.setattr(dp.shutil, "which", _which_none)
    p = DocumentParser()
    p._adapter = None
    path = _write(tmp_path, "doc.pdf", b"%PDF")
    with pytest.raises(ParseError):
        p.get_ocr_result(path)
