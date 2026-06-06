"""
Offline unit tests for proximadb_sdk.chunking_strategies.code.

Fully offline: tree-sitter-language-pack is NOT installed in this environment,
so every parser naturally falls back to its regex path. To also exercise the
AST (tree-sitter) extraction paths, we build a tiny fake tree-sitter node model
and inject a fake `tree_sitter_language_pack` module into sys.modules, then
re-instantiate the parser so its `_init_parser` picks up our fake parser.

No network, no real parser, no sleeps.
"""

import sys
import types

import pytest

from proximadb_sdk.chunking_strategies.code import (
    BashParser,
    CodeChunkingConfig,
    CodeChunkingStrategy,
    CodeRelation,
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
    EXTENSION_TO_LANGUAGE,
    LANGUAGE_PARSERS,
)
from proximadb_sdk.chunking_strategies.base import ChunkingConfig


# ---------------------------------------------------------------------------
# Fake tree-sitter node model
# ---------------------------------------------------------------------------
class FakeNode:
    """Minimal stand-in for a tree-sitter Node over a source string."""

    def __init__(
        self,
        ntype,
        content,
        start_byte=0,
        end_byte=None,
        children=None,
        start_point=(0, 0),
        end_point=(0, 0),
    ):
        self.type = ntype
        self._content = content
        self.start_byte = start_byte
        self.end_byte = end_byte if end_byte is not None else len(content)
        self.children = children or []
        self.start_point = start_point
        self.end_point = end_point


def tok(ntype, content, text, **kw):
    """Build a leaf node whose byte span points at `text` inside `content`."""
    idx = content.index(text)
    return FakeNode(
        ntype,
        content,
        start_byte=idx,
        end_byte=idx + len(text),
        start_point=kw.get("start_point", (0, 0)),
        end_point=kw.get("end_point", (0, 0)),
        children=kw.get("children", []),
    )


class FakeTree:
    def __init__(self, root):
        self.root_node = root


class FakeParser:
    """Returns a pre-built FakeTree regardless of input bytes."""

    def __init__(self, root):
        self._root = root

    def parse(self, _bytes):
        return FakeTree(self._root)


def install_fake_pack(monkeypatch, root):
    """Inject a fake tree_sitter_language_pack that yields a FakeParser."""
    mod = types.ModuleType("tree_sitter_language_pack")

    def get_parser(_lang):
        return FakeParser(root)

    def get_language(_lang):
        return object()

    mod.get_parser = get_parser
    mod.get_language = get_language
    monkeypatch.setitem(sys.modules, "tree_sitter_language_pack", mod)
    return mod


# ---------------------------------------------------------------------------
# Dataclasses / enums
# ---------------------------------------------------------------------------
def test_enums_and_dataclasses():
    assert CodeSymbolType.FUNCTION.value == 9
    assert CodeRelationType.CALLS.value == 1
    loc = SourceLocation(file_path="a.py", start_line=1, end_line=3)
    sym = CodeSymbol(
        id="x",
        symbol_type=CodeSymbolType.FUNCTION,
        fully_qualified_name="a::x",
        simple_name="x",
        location=loc,
        source_code="def x(): pass",
        language="python",
    )
    assert sym.modifiers == [] and sym.parameters == []
    rel = CodeRelation(
        from_symbol_id="a", to_symbol_id="b", relation_type=CodeRelationType.CALLS
    )
    assert rel.confidence == 1.0
    pc = ParsedCode(
        file_path="a.py",
        language="python",
        symbols=[sym],
        relations=[rel],
        imports=["import os"],
        content_hash="h",
    )
    assert pc.symbols[0] is sym


def test_code_chunking_config_defaults():
    cfg = CodeChunkingConfig()
    assert cfg.include_private is True
    assert cfg.extract_relations is True
    assert cfg.context_lines == 5


# ---------------------------------------------------------------------------
# Registry helpers
# ---------------------------------------------------------------------------
def test_registry_helpers():
    langs = get_supported_languages()
    exts = get_supported_extensions()
    assert "python" in langs
    assert ".py" in exts

    class Dummy:
        pass

    register_language_parser("dummylang", Dummy)
    register_file_extension(".DUMMY", "dummylang")
    assert LANGUAGE_PARSERS["dummylang"] is Dummy
    assert EXTENSION_TO_LANGUAGE[".dummy"] == "dummylang"


# ---------------------------------------------------------------------------
# Python parser - regex fallback path
# ---------------------------------------------------------------------------
PY_SRC = '''\
import os
from typing import Any

def top_level(a, b=2, *args, **kwargs):
    """A docstring."""
    return a + b

async def _private(x: int) -> str:
    return str(x)

class Foo(Base):
    """Class doc."""

    def __init__(self, val):
        self.val = val

    def method(self, n: int = 3):
        return helper(n)

def helper(n):
    return n
'''


def test_python_regex_fallback():
    p = PythonParser()
    p._parser = None  # force regex
    parsed = p.parse(PY_SRC, "pkg/mod.py")
    assert parsed.language == "python"
    names = {s.simple_name for s in parsed.symbols}
    assert {"top_level", "_private", "Foo", "__init__", "method", "helper"} <= names
    assert any("import os" in i for i in parsed.imports)
    # symbol types
    by_name = {s.simple_name: s for s in parsed.symbols}
    assert by_name["Foo"].symbol_type == CodeSymbolType.CLASS
    assert by_name["__init__"].symbol_type == CodeSymbolType.CONSTRUCTOR
    assert by_name["method"].symbol_type == CodeSymbolType.METHOD
    assert "async" in by_name["_private"].modifiers
    assert "private" in by_name["_private"].modifiers
    assert "extends(Base)" in by_name["Foo"].modifiers
    # parameters parsed (b=2 default, x: int typed)
    assert any(prm.get("type") == "int" for prm in by_name["method"].parameters)


def test_parse_params_regex_variants():
    p = PythonParser()
    out = p._parse_params_regex("self, a, b: int, c: int = 5, d=7, *args")
    by = {prm["name"]: prm for prm in out}
    assert "self" not in by
    assert by["b"]["type"] == "int"
    assert by["c"]["type"] == "int" and by["c"]["default"] == "5"
    assert by["d"]["default"] == "7"
    assert by["*args"]["is_variadic"] is True
    assert p._parse_params_regex("  ") == []


def test_build_signature():
    p = PythonParser()
    sig = p._build_signature(
        "f",
        [{"name": "a", "type": "int", "default": "0"}, {"name": "b"}],
        "str",
    )
    assert sig == "f(a: int = 0, b) -> str"
    assert p._build_signature("g", [], None) == "g()"


def test_find_block_end_regex():
    p = PythonParser()
    lines = ["def f():", "    a = 1", "", "    b = 2", "x = 3"]
    end = p._find_block_end_regex(lines, 0, 0)
    assert end == 3


# ---------------------------------------------------------------------------
# Python parser - tree-sitter (fake AST) path
# ---------------------------------------------------------------------------
def build_python_ast():
    """Build a fake python AST covering function, class, decorated defs."""
    content = (
        "import os\n"
        "@deco\n"
        "def fn(a, b: int = 1, *args, **kw):\n"
        '    """doc text"""\n'
        "    return helper()\n"
        "class C:\n"
        "    def m(self):\n"
        "        pass\n"
        "def helper():\n"
        "    return 1\n"
    )

    # import statement
    import_node = tok("import_statement", content, "import os")

    # --- decorated function fn ---
    fn_ident = tok("identifier", content, "fn")
    # parameters node with children
    params_text = "(a, b: int = 1, *args, **kw)"
    params_idx = content.index(params_text)
    p_a = tok("identifier", content, "a")
    # typed_default_parameter for "b: int = 1"
    btxt = "b"
    b_ident = FakeNode("identifier", content, content.index("b: int"), content.index("b: int") + 1)
    b_type = tok("type", content, "int")
    b_default = tok("integer", content, "1")
    typed_default = FakeNode(
        "typed_default_parameter",
        content,
        params_idx,
        params_idx + len(params_text),
        children=[b_ident, b_type, b_default],
    )
    # list/dict splat
    args_ident = FakeNode("identifier", content, content.index("*args") + 1, content.index("*args") + 5)
    list_splat = FakeNode("list_splat_pattern", content, children=[args_ident])
    kw_ident = FakeNode("identifier", content, content.index("**kw") + 2, content.index("**kw") + 4)
    dict_splat = FakeNode("dictionary_splat_pattern", content, children=[kw_ident])
    params_node = FakeNode(
        "parameters",
        content,
        params_idx,
        params_idx + len(params_text),
        children=[p_a, typed_default, list_splat, dict_splat],
    )
    # body block with docstring + a call
    doc_str = tok("string", content, '"""doc text"""')
    doc_expr = FakeNode("expression_statement", content, children=[doc_str])
    call_ident = tok("identifier", content, "helper")
    call_node = FakeNode(
        "call",
        content,
        content.index("helper()"),
        content.index("helper()") + 8,
        children=[call_ident],
        start_point=(4, 11),
    )
    if_node = FakeNode("if_statement", content, children=[])
    body_block = FakeNode("block", content, children=[doc_expr, call_node, if_node])
    fn_def = FakeNode(
        "function_definition",
        content,
        content.index("def fn"),
        content.index("    return helper()") + len("    return helper()"),
        children=[fn_ident, params_node, body_block],
        start_point=(2, 0),
        end_point=(4, 20),
    )
    decorator = tok("decorator", content, "@deco")
    decorated = FakeNode(
        "decorated_definition", content, children=[decorator, fn_def]
    )

    # --- class C with method m ---
    c_ident = tok("identifier", content, "C")
    m_ident = tok("identifier", content, "m")
    self_ident = tok("identifier", content, "self")
    m_params = FakeNode("parameters", content, children=[self_ident])
    m_block = FakeNode("block", content, children=[])
    m_def = FakeNode(
        "function_definition",
        content,
        content.index("def m"),
        content.index("pass") + 4,
        children=[m_ident, m_params, m_block],
        start_point=(6, 4),
        end_point=(7, 12),
    )
    c_block = FakeNode("block", content, children=[m_def])
    class_def = FakeNode(
        "class_definition",
        content,
        content.index("class C"),
        content.index("pass") + 4,
        children=[c_ident, c_block],
        start_point=(5, 0),
        end_point=(7, 12),
    )

    # --- helper function ---
    h_ident = tok("identifier", content, "helper")
    h_params = FakeNode("parameters", content, children=[])
    h_block = FakeNode("block", content, children=[])
    helper_def = FakeNode(
        "function_definition",
        content,
        content.index("def helper"),
        len(content),
        children=[h_ident, h_params, h_block],
        start_point=(8, 0),
        end_point=(9, 12),
    )

    root = FakeNode(
        "module",
        content,
        0,
        len(content),
        children=[import_node, decorated, class_def, helper_def],
    )
    return root, content


def test_python_treesitter_path(monkeypatch):
    root, content = build_python_ast()
    install_fake_pack(monkeypatch, root)
    p = PythonParser()
    assert p._parser is not None  # picked up the fake
    parsed = p.parse(content, "mod.py")
    by = {s.simple_name: s for s in parsed.symbols}
    assert "fn" in by and "C" in by and "m" in by and "helper" in by
    fn = by["fn"]
    assert fn.documentation == "doc text"
    assert "@deco" in fn.modifiers
    assert fn.complexity["cyclomatic"] >= 2  # if_statement bumps it
    # params: a, b (typed+default), *args, **kw  (self excluded elsewhere)
    pnames = {prm["name"] for prm in fn.parameters}
    assert "a" in pnames and "*args" in pnames and "**kw" in pnames
    assert any(prm.get("type") == "int" for prm in fn.parameters)
    assert by["m"].symbol_type == CodeSymbolType.METHOD
    assert any("import os" in i for i in parsed.imports)
    # relation fn -> helper should be present
    assert any(r.relation_type == CodeRelationType.CALLS for r in parsed.relations)


def test_python_treesitter_no_name(monkeypatch):
    # function_definition with no identifier -> returns None
    content = "def f(): pass"
    block = FakeNode("block", content, children=[])
    fn_def = FakeNode("function_definition", content, children=[block])  # no identifier
    root = FakeNode("module", content, children=[fn_def])
    install_fake_pack(monkeypatch, root)
    p = PythonParser()
    parsed = p.parse(content, "x.py")
    assert parsed.symbols == []


def test_extract_docstring_variants():
    p = PythonParser()
    content = "'single'"
    s = tok("string", content, "'single'")
    expr = FakeNode("expression_statement", content, children=[s])
    block = FakeNode("block", content, children=[expr])
    assert p._extract_docstring_ts(block, content) == "single"
    assert p._extract_docstring_ts(None, content) is None
    empty = FakeNode("block", content, children=[])
    assert p._extract_docstring_ts(empty, content) is None


def test_get_callee_name_attribute():
    p = PythonParser()
    content = "obj.method()"
    inner = tok("identifier", content, "method")
    attr = FakeNode("attribute", content, children=[inner])
    call = FakeNode("call", content, children=[attr])
    assert p._get_callee_name_ts(call, content) == "method"


# ---------------------------------------------------------------------------
# JavaScript parser (placeholder parse + extensions)
# ---------------------------------------------------------------------------
def test_javascript_parser():
    js = JavaScriptParser()
    assert js.language == "javascript"
    assert ".js" in js.file_extensions
    ts = JavaScriptParser(typescript=True)
    assert ts.language == "typescript"
    assert ".ts" in ts.file_extensions
    parsed = ts.parse("const x = 1;", "a.ts")
    assert parsed.symbols == [] and parsed.language == "typescript"


# ---------------------------------------------------------------------------
# Rust parser
# ---------------------------------------------------------------------------
RUST_SRC = """\
use std::collections::HashMap;

pub async fn run(x: i32) -> i32 {
    x + 1
}

unsafe fn danger() {}
"""


def test_rust_regex_fallback():
    r = RustParser()
    r._parser = None
    parsed = r.parse(RUST_SRC, "lib.rs")
    assert parsed.language == "rust"
    by = {s.simple_name: s for s in parsed.symbols}
    assert "run" in by and "danger" in by
    assert "pub" in by["run"].modifiers and "async" in by["run"].modifiers
    assert "unsafe" in by["danger"].modifiers
    assert any("use std" in i for i in parsed.imports)
    assert r.language == "rust" and ".rs" in r.file_extensions


def test_rust_helpers():
    r = RustParser()
    sig = r._build_rust_signature("f", [{"name": "x", "type": "i32"}], "bool")
    assert sig == "fn f(x: i32) -> bool"
    assert r._build_rust_signature("g", [], None) == "fn g()"


def test_rust_treesitter_path(monkeypatch):
    content = (
        "use std::fmt;\n"
        "pub fn build() -> Thing {\n"
        "    Thing::new()\n"
        "}\n"
        "struct Thing { id: u32 }\n"
        "enum Color { Red }\n"
    )
    # use declaration
    use_decl = tok("use_declaration", content, "use std::fmt;")

    # function build
    vis = tok("visibility_modifier", content, "pub")
    fn_ident = tok("identifier", content, "build")
    rt_type = tok("type_identifier", content, "Thing")
    ret = FakeNode("return_type", content, children=[rt_type])
    # parameter list (empty)
    params = FakeNode("parameters", content, children=[])
    fn_item = FakeNode(
        "function_item",
        content,
        content.index("pub fn"),
        content.index("}\n") + 1,
        children=[vis, fn_ident, params, ret],
        start_point=(1, 0),
        end_point=(3, 1),
    )

    # struct Thing { id: u32 }
    st_ident = tok("type_identifier", content, "Thing")
    field_id = tok("field_identifier", content, "id")
    field_ty = tok("type_identifier", content, "u32")
    field_decl = FakeNode("field_declaration", content, children=[field_id, field_ty])
    field_list = FakeNode("field_declaration_list", content, children=[field_decl])
    struct_item = FakeNode(
        "struct_item",
        content,
        content.index("struct Thing"),
        content.index("}\nenum"),
        children=[st_ident, field_list],
        start_point=(4, 0),
        end_point=(4, 25),
    )

    # enum Color
    en_ident = tok("type_identifier", content, "Color")
    enum_item = FakeNode(
        "enum_item",
        content,
        content.index("enum Color"),
        len(content),
        children=[en_ident],
        start_point=(5, 0),
        end_point=(5, 18),
    )

    root = FakeNode(
        "source_file",
        content,
        0,
        len(content),
        children=[use_decl, fn_item, struct_item, enum_item],
    )
    install_fake_pack(monkeypatch, root)
    r = RustParser()
    assert r._parser is not None
    parsed = r.parse(content, "lib.rs")
    by = {s.simple_name: s for s in parsed.symbols}
    assert "build" in by
    assert "Thing" in by and by["Thing"].symbol_type == CodeSymbolType.STRUCT
    assert "id" in by and by["id"].symbol_type == CodeSymbolType.FIELD
    assert "Color" in by and by["Color"].symbol_type == CodeSymbolType.ENUM
    assert "pub" in by["build"].modifiers
    assert any("use std" in i for i in parsed.imports)


def test_rust_trait_and_impl(monkeypatch):
    content = (
        "trait Speak {\n    fn hello();\n}\n"
        "impl Speak for Dog {\n    fn hello() {}\n}\n"
    )
    # trait
    tr_ident = tok("type_identifier", content, "Speak")
    sig_ident = tok("identifier", content, "hello")
    sig_item = FakeNode(
        "function_signature_item",
        content,
        children=[sig_ident],
        start_point=(1, 4),
        end_point=(1, 14),
    )
    decl_list_tr = FakeNode("declaration_list", content, children=[sig_item])
    trait_item = FakeNode(
        "trait_item",
        content,
        content.index("trait Speak"),
        content.index("}\nimpl") + 1,
        children=[tr_ident, decl_list_tr],
        start_point=(0, 0),
        end_point=(2, 1),
    )
    # impl Speak for Dog
    speak_id = tok("type_identifier", content, "Speak")
    # second type_identifier "Dog"
    dog_idx = content.index("Dog")
    dog_id = FakeNode("type_identifier", content, dog_idx, dog_idx + 3)
    impl_fn_ident = FakeNode(
        "identifier", content, content.index("fn hello() {}") + 3,
        content.index("fn hello() {}") + 8,
    )
    impl_fn = FakeNode(
        "function_item",
        content,
        content.index("fn hello() {}"),
        content.index("fn hello() {}") + 13,
        children=[impl_fn_ident],
        start_point=(4, 4),
        end_point=(4, 17),
    )
    decl_list_impl = FakeNode("declaration_list", content, children=[impl_fn])
    impl_item = FakeNode(
        "impl_item",
        content,
        content.index("impl Speak"),
        len(content),
        children=[speak_id, dog_id, decl_list_impl],
    )
    root = FakeNode(
        "source_file", content, 0, len(content), children=[trait_item, impl_item]
    )
    install_fake_pack(monkeypatch, root)
    r = RustParser()
    parsed = r.parse(content, "t.rs")
    by = {s.simple_name: s for s in parsed.symbols}
    assert "Speak" in by and by["Speak"].symbol_type == CodeSymbolType.TRAIT
    # trait method + impl method both named hello
    hellos = [s for s in parsed.symbols if s.simple_name == "hello"]
    assert len(hellos) >= 2


# ---------------------------------------------------------------------------
# Go parser
# ---------------------------------------------------------------------------
GO_SRC = """\
import "fmt"

func Add(a int, b int) int {
    return a + b
}

func (s *Server) Handle() {
}
"""


def test_go_regex_fallback():
    g = GoParser()
    g._parser = None
    parsed = g.parse(GO_SRC, "main.go")
    by = {s.simple_name: s for s in parsed.symbols}
    assert "Add" in by and by["Add"].symbol_type == CodeSymbolType.FUNCTION
    assert "Handle" in by and by["Handle"].symbol_type == CodeSymbolType.METHOD
    assert g.language == "go" and ".go" in g.file_extensions


def test_go_treesitter_path(monkeypatch):
    content = (
        'import "fmt"\n'
        "func Add(a int) int {\n  return a\n}\n"
        "func (s *Server) Run() {\n}\n"
        "type Server struct {}\n"
        "type Reader interface {}\n"
    )
    import_decl = tok("import_declaration", content, 'import "fmt"')

    # func Add(a int) int
    add_ident = tok("identifier", content, "Add")
    a_ident = tok("identifier", content, "a")
    a_type = tok("type_identifier", content, "int")
    a_param = FakeNode("parameter_declaration", content, children=[a_ident, a_type])
    add_params = FakeNode("parameter_list", content, children=[a_param])
    result = tok("result", content, "int", start_point=(1, 0))
    func_decl = FakeNode(
        "function_declaration",
        content,
        content.index("func Add"),
        content.index("}\nfunc") + 1,
        children=[add_ident, add_params, result],
        start_point=(1, 0),
        end_point=(3, 1),
    )

    # method func (s *Server) Run()
    recv_ident = tok("type_identifier", content, "Server")
    recv_param_decl = FakeNode(
        "parameter_declaration", content, children=[recv_ident]
    )
    recv_list = FakeNode("parameter_list", content, children=[recv_param_decl])
    run_ident = tok("field_identifier", content, "Run")
    run_params = FakeNode("parameter_list", content, children=[])
    method_decl = FakeNode(
        "method_declaration",
        content,
        content.index("func (s"),
        content.index("}\ntype") + 1,
        children=[recv_list, run_ident, run_params],
        start_point=(4, 0),
        end_point=(5, 1),
    )

    # type Server struct {}
    srv_id = tok("type_identifier", content, "Server")
    struct_t = FakeNode("struct_type", content, children=[])
    srv_spec = FakeNode(
        "type_spec",
        content,
        children=[srv_id, struct_t],
        start_point=(6, 0),
        end_point=(6, 20),
    )
    type_decl1 = FakeNode("type_declaration", content, children=[srv_spec])

    # type Reader interface {}
    rd_id = tok("type_identifier", content, "Reader")
    iface_t = FakeNode("interface_type", content, children=[])
    rd_spec = FakeNode(
        "type_spec",
        content,
        children=[rd_id, iface_t],
        start_point=(7, 0),
        end_point=(7, 22),
    )
    type_decl2 = FakeNode("type_declaration", content, children=[rd_spec])

    root = FakeNode(
        "source_file",
        content,
        0,
        len(content),
        children=[import_decl, func_decl, method_decl, type_decl1, type_decl2],
    )
    install_fake_pack(monkeypatch, root)
    g = GoParser()
    parsed = g.parse(content, "main.go")
    by = {s.simple_name: s for s in parsed.symbols}
    assert "Add" in by and by["Add"].symbol_type == CodeSymbolType.FUNCTION
    assert "Run" in by and by["Run"].symbol_type == CodeSymbolType.METHOD
    assert "Server" in by and by["Server"].symbol_type == CodeSymbolType.STRUCT
    assert "Reader" in by and by["Reader"].symbol_type == CodeSymbolType.INTERFACE
    assert any("import" in i for i in parsed.imports)


# ---------------------------------------------------------------------------
# Java parser
# ---------------------------------------------------------------------------
JAVA_SRC = """\
import java.util.List;

public class Greeter {
    private String name;
}

interface Speaker {
}

enum Color {
}
"""


def test_java_regex_fallback():
    j = JavaParser()
    j._parser = None
    parsed = j.parse(JAVA_SRC, "G.java")
    by = {s.simple_name: s for s in parsed.symbols}
    assert "Greeter" in by and by["Greeter"].symbol_type == CodeSymbolType.CLASS
    assert "Speaker" in by and by["Speaker"].symbol_type == CodeSymbolType.INTERFACE
    assert "Color" in by and by["Color"].symbol_type == CodeSymbolType.ENUM
    assert any("import" in i for i in parsed.imports)
    assert j.language == "java"


def test_java_treesitter_path(monkeypatch):
    content = (
        "import java.util.List;\n"
        "class Greeter extends Base implements I {\n"
        "  public String greet(int n) {}\n"
        "  Greeter(int x) {}\n"
        "}\n"
        "interface Speaker {\n"
        "  void talk();\n"
        "}\n"
        "enum Color {}\n"
    )
    import_decl = tok("import_declaration", content, "import java.util.List;")

    # method
    mod_pub = tok("modifier", content, "public")
    modifiers_node = FakeNode("modifiers", content, children=[mod_pub])
    ret_type = tok("type_identifier", content, "String")
    greet_ident = tok("identifier", content, "greet")
    pn_ident = tok("identifier", content, "n")
    pn_type = tok("type_identifier", content, "int")
    fparam = FakeNode("formal_parameter", content, children=[pn_ident, pn_type])
    fparams = FakeNode("formal_parameters", content, children=[fparam])
    method_decl = FakeNode(
        "method_declaration",
        content,
        content.index("public String greet"),
        content.index("greet(int n) {}") + 15,
        children=[modifiers_node, ret_type, greet_ident, fparams],
        start_point=(2, 2),
        end_point=(2, 30),
    )

    # constructor
    ctor_ident = tok("identifier", content, "Greeter")
    ctor_pident = FakeNode(
        "identifier", content, content.index("int x") + 4, content.index("int x") + 5
    )
    ctor_ptype = FakeNode(
        "type_identifier", content, content.index("int x"), content.index("int x") + 3
    )
    ctor_fparam = FakeNode(
        "formal_parameter", content, children=[ctor_pident, ctor_ptype]
    )
    ctor_fparams = FakeNode("formal_parameters", content, children=[ctor_fparam])
    ctor_decl = FakeNode(
        "constructor_declaration",
        content,
        content.index("Greeter(int x)"),
        content.index("Greeter(int x)") + 17,
        children=[ctor_ident, ctor_fparams],
        start_point=(3, 2),
        end_point=(3, 18),
    )

    class_body = FakeNode("class_body", content, children=[method_decl, ctor_decl])
    cls_ident = tok("identifier", content, "Greeter")
    superclass_id = tok("type_identifier", content, "Base")
    superclass = FakeNode("superclass", content, children=[superclass_id])
    iface_id = tok("type_identifier", content, "I")
    super_ifaces = FakeNode("super_interfaces", content, children=[iface_id])
    class_decl = FakeNode(
        "class_declaration",
        content,
        content.index("class Greeter"),
        content.index("}\ninterface") + 1,
        children=[cls_ident, superclass, super_ifaces, class_body],
        start_point=(1, 0),
        end_point=(4, 1),
    )

    # interface with method
    talk_ident = tok("identifier", content, "talk")
    talk_void = tok("void_type", content, "void")
    talk_method = FakeNode(
        "method_declaration",
        content,
        content.index("void talk();"),
        content.index("void talk();") + 12,
        children=[talk_void, talk_ident],
        start_point=(6, 2),
        end_point=(6, 14),
    )
    iface_body = FakeNode("interface_body", content, children=[talk_method])
    spk_ident = tok("identifier", content, "Speaker")
    iface_decl = FakeNode(
        "interface_declaration",
        content,
        content.index("interface Speaker"),
        content.index("}\nenum") + 1,
        children=[spk_ident, iface_body],
        start_point=(5, 0),
        end_point=(7, 1),
    )

    # enum
    color_ident = tok("identifier", content, "Color")
    enum_decl = FakeNode(
        "enum_declaration",
        content,
        content.index("enum Color"),
        len(content),
        children=[color_ident],
        start_point=(8, 0),
        end_point=(8, 13),
    )

    root = FakeNode(
        "program",
        content,
        0,
        len(content),
        children=[import_decl, class_decl, iface_decl, enum_decl],
    )
    install_fake_pack(monkeypatch, root)
    j = JavaParser()
    parsed = j.parse(content, "G.java")
    by = {s.simple_name: s for s in parsed.symbols}
    cls = next(
        s for s in parsed.symbols if s.symbol_type == CodeSymbolType.CLASS
    )
    assert cls.simple_name == "Greeter"
    assert any("extends(Base)" in m for m in cls.modifiers)
    assert any("implements(I)" in m for m in cls.modifiers)
    assert by["greet"].symbol_type == CodeSymbolType.METHOD
    assert by["greet"].return_type == "String"
    # constructor 'Greeter' appears too; both class and ctor share name -> take ctor
    ctors = [s for s in parsed.symbols if s.symbol_type == CodeSymbolType.CONSTRUCTOR]
    assert ctors and ctors[0].simple_name == "Greeter"
    assert by["Speaker"].symbol_type == CodeSymbolType.INTERFACE
    assert by["talk"].symbol_type == CodeSymbolType.METHOD
    assert by["Color"].symbol_type == CodeSymbolType.ENUM


# ---------------------------------------------------------------------------
# Cpp parser
# ---------------------------------------------------------------------------
def test_cpp_regex_and_props():
    c = CppParser(c_mode=True)
    assert c.language == "c" and ".c" in c.file_extensions
    parsed = c.parse("#include <stdio.h>\nint main() { return 0; }\n", "a.c")
    assert any("#include" in i for i in parsed.imports)
    cpp = CppParser()
    assert cpp.language == "cpp" and ".cpp" in cpp.file_extensions


def test_cpp_treesitter_path(monkeypatch):
    content = (
        "#include <vector>\n"
        "int add(int a) { return a; }\n"
        "class Widget { };\n"
        "struct Point { };\n"
        "enum Mode { };\n"
        "namespace ns { int helper() { return 1; } }\n"
    )
    include = tok("preproc_include", content, "#include <vector>")

    # function add
    ret_t = tok("primitive_type", content, "int")
    fn_ident = tok("identifier", content, "add")
    a_ident = FakeNode(
        "identifier", content, content.index("int a") + 4, content.index("int a") + 5
    )
    a_type = FakeNode(
        "primitive_type", content, content.index("int a"), content.index("int a") + 3
    )
    a_param = FakeNode("parameter_declaration", content, children=[a_ident, a_type])
    plist = FakeNode("parameter_list", content, children=[a_param])
    declr = FakeNode("function_declarator", content, children=[fn_ident, plist])
    func_def = FakeNode(
        "function_definition",
        content,
        content.index("int add"),
        content.index("class Widget"),
        children=[ret_t, declr],
        start_point=(1, 0),
        end_point=(1, 28),
    )

    # class Widget
    w_ident = tok("type_identifier", content, "Widget")
    w_fieldlist = FakeNode("field_declaration_list", content, children=[])
    class_spec = FakeNode(
        "class_specifier",
        content,
        content.index("class Widget"),
        content.index("struct Point"),
        children=[w_ident, w_fieldlist],
        start_point=(2, 0),
        end_point=(2, 17),
    )

    # struct Point
    p_ident = tok("type_identifier", content, "Point")
    struct_spec = FakeNode(
        "struct_specifier",
        content,
        content.index("struct Point"),
        content.index("enum Mode"),
        children=[p_ident],
        start_point=(3, 0),
        end_point=(3, 17),
    )

    # enum Mode
    m_ident = tok("type_identifier", content, "Mode")
    enum_spec = FakeNode(
        "enum_specifier",
        content,
        content.index("enum Mode"),
        content.index("namespace"),
        children=[m_ident],
        start_point=(4, 0),
        end_point=(4, 14),
    )

    # namespace ns { int helper ... }
    ns_ident = tok("identifier", content, "ns")
    h_ret = FakeNode(
        "primitive_type", content, content.index("int helper"),
        content.index("int helper") + 3,
    )
    h_ident = tok("identifier", content, "helper")
    h_declr = FakeNode("function_declarator", content, children=[h_ident])
    h_func = FakeNode(
        "function_definition",
        content,
        content.index("int helper"),
        content.index("} }"),
        children=[h_ret, h_declr],
        start_point=(5, 14),
        end_point=(5, 42),
    )
    ns_node = FakeNode(
        "namespace_definition", content, children=[ns_ident, h_func]
    )

    root = FakeNode(
        "translation_unit",
        content,
        0,
        len(content),
        children=[include, func_def, class_spec, struct_spec, enum_spec, ns_node],
    )
    install_fake_pack(monkeypatch, root)
    cpp = CppParser()
    parsed = cpp.parse(content, "a.cpp")
    by = {s.simple_name: s for s in parsed.symbols}
    assert "add" in by and by["add"].symbol_type == CodeSymbolType.FUNCTION
    assert "Widget" in by and by["Widget"].symbol_type == CodeSymbolType.CLASS
    assert "Point" in by and by["Point"].symbol_type == CodeSymbolType.STRUCT
    assert "Mode" in by and by["Mode"].symbol_type == CodeSymbolType.ENUM
    assert "helper" in by  # found inside namespace
    assert any("#include" in i for i in parsed.imports)


# ---------------------------------------------------------------------------
# Ruby parser
# ---------------------------------------------------------------------------
def test_ruby_regex_fallback():
    rb = RubyParser()
    rb._parser = None
    parsed = rb.parse('require "json"\nrequire_relative "x"\n', "a.rb")
    assert any("require" in i for i in parsed.imports)
    assert rb.language == "ruby" and ".rb" in rb.file_extensions


def test_ruby_treesitter_path(monkeypatch):
    content = (
        'require "json"\n'
        "class Animal < Base\n"
        "  def speak(name, count = 1, *rest, **opts)\n  end\n"
        "  def self.create\n  end\n"
        "end\n"
        "module Helpers\n"
        "  def util\n  end\n"
        "end\n"
    )
    require_call = tok("call", content, 'require "json"')

    # class Animal < Base
    cls_const = tok("constant", content, "Animal")
    base_const = tok("constant", content, "Base")
    superclass = FakeNode("superclass", content, children=[base_const])
    # method speak with params
    speak_ident = tok("identifier", content, "speak")
    p_name = tok("identifier", content, "name")
    opt_inner = FakeNode(
        "identifier", content, content.index("count = 1"), content.index("count = 1") + 5
    )
    opt = FakeNode("optional_parameter", content, children=[opt_inner])
    splat_inner = FakeNode(
        "identifier", content, content.index("*rest") + 1, content.index("*rest") + 5
    )
    splat = FakeNode("splat_parameter", content, children=[splat_inner])
    hsplat_inner = FakeNode(
        "identifier", content, content.index("**opts") + 2, content.index("**opts") + 6
    )
    hsplat = FakeNode("hash_splat_parameter", content, children=[hsplat_inner])
    method_params = FakeNode(
        "method_parameters", content, children=[p_name, opt, splat, hsplat]
    )
    speak_method = FakeNode(
        "method",
        content,
        content.index("def speak"),
        content.index("end\n  def self") + 3,
        children=[speak_ident, method_params],
        start_point=(2, 2),
        end_point=(3, 5),
    )
    # singleton method self.create
    create_ident = tok("identifier", content, "create")
    singleton = FakeNode(
        "singleton_method",
        content,
        content.index("def self.create"),
        content.index("end\nend") + 3,
        children=[create_ident],
        start_point=(4, 2),
        end_point=(5, 5),
    )
    class_node = FakeNode(
        "class",
        content,
        content.index("class Animal"),
        content.index("end\nmodule"),
        children=[cls_const, superclass, speak_method, singleton],
        start_point=(1, 0),
        end_point=(6, 3),
    )

    # module Helpers with method util
    mod_const = tok("constant", content, "Helpers")
    util_ident = tok("identifier", content, "util")
    util_method = FakeNode(
        "method",
        content,
        content.index("def util"),
        content.index("end\nend\n", content.index("Helpers")) + 3,
        children=[util_ident],
        start_point=(8, 2),
        end_point=(9, 5),
    )
    module_node = FakeNode(
        "module",
        content,
        content.index("module Helpers"),
        len(content),
        children=[mod_const, util_method],
        start_point=(7, 0),
        end_point=(10, 3),
    )

    root = FakeNode(
        "program",
        content,
        0,
        len(content),
        children=[require_call, class_node, module_node],
    )
    install_fake_pack(monkeypatch, root)
    rb = RubyParser()
    parsed = rb.parse(content, "a.rb")
    by = {s.simple_name: s for s in parsed.symbols}
    assert "Animal" in by and by["Animal"].symbol_type == CodeSymbolType.CLASS
    assert any("extends(Base)" in m for m in by["Animal"].modifiers)
    assert "speak" in by and by["speak"].symbol_type == CodeSymbolType.METHOD
    speak_params = {prm["name"] for prm in by["speak"].parameters}
    assert "name" in speak_params
    assert any(n.startswith("*rest") for n in speak_params)
    assert any(n.startswith("**opts") for n in speak_params)
    assert "create" in by and "class_method" in by["create"].modifiers
    assert "Helpers" in by and by["Helpers"].symbol_type == CodeSymbolType.MODULE
    assert "util" in by
    assert any("require" in i for i in parsed.imports)


# ---------------------------------------------------------------------------
# Bash parser
# ---------------------------------------------------------------------------
def test_bash_regex_fallback():
    b = BashParser()
    b._parser = None
    src = "function greet {\n  echo hi\n}\nrun() {\n echo go\n}\nsource ./lib.sh\n. ./x.sh\n"
    parsed = b.parse(src, "s.sh")
    by = {s.simple_name for s in parsed.symbols}
    assert "greet" in by and "run" in by
    assert any("source" in i or i.startswith(".") for i in parsed.imports)
    assert b.language == "bash" and ".sh" in b.file_extensions


def test_bash_treesitter_path(monkeypatch):
    content = "greet() {\n  echo hi\n}\nsource ./lib.sh\n"
    word = tok("word", content, "greet")
    func_def = FakeNode(
        "function_definition",
        content,
        content.index("greet()"),
        content.index("}\nsource") + 1,
        children=[word],
        start_point=(0, 0),
        end_point=(2, 1),
    )
    src_cmd = tok("command", content, "source ./lib.sh")
    root = FakeNode(
        "program", content, 0, len(content), children=[func_def, src_cmd]
    )
    install_fake_pack(monkeypatch, root)
    b = BashParser()
    parsed = b.parse(content, "s.sh")
    assert any(s.simple_name == "greet" for s in parsed.symbols)
    assert any("source" in i for i in parsed.imports)


# ---------------------------------------------------------------------------
# SQL parser
# ---------------------------------------------------------------------------
SQL_SRC = """\
CREATE FUNCTION fn_add() RETURNS int AS $$ SELECT 1 $$;
CREATE OR REPLACE PROCEDURE do_thing() AS BEGIN END;
CREATE TABLE users (id int);
CREATE VIEW active_users AS SELECT * FROM users;
"""


def test_sql_regex_fallback():
    s = SqlParser()
    s._parser = None
    parsed = s.parse(SQL_SRC, "schema.sql")
    by = {sym.simple_name: sym for sym in parsed.symbols}
    assert by["fn_add"].symbol_type == CodeSymbolType.FUNCTION
    assert by["do_thing"].symbol_type == CodeSymbolType.FUNCTION
    assert by["users"].symbol_type == CodeSymbolType.STRUCT
    assert by["active_users"].symbol_type == CodeSymbolType.TYPE_ALIAS
    assert s.language == "sql" and ".sql" in s.file_extensions


def test_sql_treesitter_path(monkeypatch):
    content = (
        "CREATE FUNCTION f1() RETURNS int;\n"
        "CREATE TABLE t1 (id int);\n"
        "CREATE VIEW v1 AS SELECT 1;\n"
    )
    f_name = tok("identifier", content, "f1")
    fn_stmt = FakeNode(
        "create_function_statement",
        content,
        content.index("CREATE FUNCTION"),
        content.index(";\nCREATE TABLE") + 1,
        children=[f_name],
        start_point=(0, 0),
        end_point=(0, 33),
    )
    t_name = tok("identifier", content, "t1")
    tbl_stmt = FakeNode(
        "create_table_statement",
        content,
        content.index("CREATE TABLE"),
        content.index(";\nCREATE VIEW") + 1,
        children=[t_name],
        start_point=(1, 0),
        end_point=(1, 25),
    )
    v_name = tok("identifier", content, "v1")
    view_stmt = FakeNode(
        "create_view_statement",
        content,
        content.index("CREATE VIEW"),
        len(content),
        children=[v_name],
        start_point=(2, 0),
        end_point=(2, 27),
    )
    root = FakeNode(
        "program", content, 0, len(content), children=[fn_stmt, tbl_stmt, view_stmt]
    )
    install_fake_pack(monkeypatch, root)
    s = SqlParser()
    parsed = s.parse(content, "schema.sql")
    by = {sym.simple_name: sym for sym in parsed.symbols}
    assert by["f1"].symbol_type == CodeSymbolType.FUNCTION
    assert by["t1"].symbol_type == CodeSymbolType.STRUCT
    assert by["v1"].symbol_type == CodeSymbolType.TYPE_ALIAS


# ---------------------------------------------------------------------------
# YAML parser
# ---------------------------------------------------------------------------
def test_yaml_regex_fallback():
    y = YamlParser()
    y._parser = None
    parsed = y.parse("name: app\nversion: 1\n  nested: x\n", "c.yaml")
    by = {s.simple_name for s in parsed.symbols}
    assert "name" in by and "version" in by
    assert y.language == "yaml" and ".yaml" in y.file_extensions


def test_yaml_treesitter_path(monkeypatch):
    content = "name: app\nmeta:\n  key: val\n"
    # top-level pair name: app
    k1 = tok("flow_node", content, "name")
    pair1 = FakeNode(
        "block_mapping_pair",
        content,
        content.index("name:"),
        content.index("\nmeta"),
        children=[k1],
        start_point=(0, 0),
        end_point=(0, 9),
    )
    # nested pair under meta
    k_nested = tok("flow_node", content, "key")
    pair_nested = FakeNode(
        "block_mapping_pair",
        content,
        content.index("key:"),
        content.index("val") + 3,
        children=[k_nested],
        start_point=(2, 2),
        end_point=(2, 12),
    )
    k2 = tok("flow_node", content, "meta")
    pair2 = FakeNode(
        "block_mapping_pair",
        content,
        content.index("meta:"),
        len(content),
        children=[k2, pair_nested],
        start_point=(1, 0),
        end_point=(2, 12),
    )
    mapping = FakeNode("block_mapping", content, children=[pair1, pair2])
    root = FakeNode("stream", content, 0, len(content), children=[mapping])
    install_fake_pack(monkeypatch, root)
    y = YamlParser()
    parsed = y.parse(content, "c.yaml")
    by = {s.simple_name for s in parsed.symbols}
    assert "name" in by and "meta" in by and "key" in by


# ---------------------------------------------------------------------------
# JSON parser
# ---------------------------------------------------------------------------
def test_json_regex_fallback():
    j = JsonParser()
    j._parser = None
    parsed = j.parse('{\n  "name": "app",\n  "version": 1\n}\n', "p.json")
    by = {s.simple_name for s in parsed.symbols}
    assert "name" in by and "version" in by
    assert j.language == "json" and ".json" in j.file_extensions


def test_json_treesitter_path(monkeypatch):
    content = '{"name": "app", "nested": {"inner": 1}}'
    k1 = tok("string", content, '"name"')
    pair1 = FakeNode(
        "pair",
        content,
        content.index('"name"'),
        content.index(', "nested"'),
        children=[k1],
        start_point=(0, 1),
        end_point=(0, 14),
    )
    # nested pair - len(current_path)>1 so skipped from symbols but recursed
    k_inner = tok("string", content, '"inner"')
    pair_inner = FakeNode(
        "pair",
        content,
        content.index('"inner"'),
        content.index("1}") + 1,
        children=[k_inner],
        start_point=(0, 26),
        end_point=(0, 36),
    )
    obj_inner = FakeNode("object", content, children=[pair_inner])
    k2 = tok("string", content, '"nested"')
    pair2 = FakeNode(
        "pair",
        content,
        content.index('"nested"'),
        len(content),
        children=[k2, obj_inner],
        start_point=(0, 16),
        end_point=(0, 38),
    )
    obj = FakeNode("object", content, children=[pair1, pair2])
    root = FakeNode("document", content, 0, len(content), children=[obj])
    install_fake_pack(monkeypatch, root)
    j = JsonParser()
    parsed = j.parse(content, "p.json")
    by = {s.simple_name for s in parsed.symbols}
    # top-level keys become symbols; the parser recurses into nested objects
    assert "name" in by and "nested" in by


# ---------------------------------------------------------------------------
# XML parser
# ---------------------------------------------------------------------------
def test_xml_parser():
    x = XmlParser()
    assert x.language == "xml" and ".xml" in x.file_extensions
    parsed = x.parse("<root><child>v</child></root>", "c.xml")
    assert parsed.symbols
    assert parsed.symbols[0].simple_name == "root"
    assert parsed.symbols[0].symbol_type == CodeSymbolType.MODULE
    # no element -> no symbols
    empty = x.parse("no tags here", "c.xml")
    assert empty.symbols == []


# ---------------------------------------------------------------------------
# Perl parser
# ---------------------------------------------------------------------------
def test_perl_regex_fallback():
    p = PerlParser()
    p._parser = None
    src = "package My::Mod;\nuse strict;\nsub greet {\n  return 1;\n}\n"
    parsed = p.parse(src, "m.pm")
    by = {s.simple_name: s for s in parsed.symbols}
    assert "greet" in by and by["greet"].symbol_type == CodeSymbolType.FUNCTION
    assert "My::Mod" in by and by["My::Mod"].symbol_type == CodeSymbolType.PACKAGE
    assert any("use" in i for i in parsed.imports)
    assert p.language == "perl" and ".pl" in p.file_extensions


def test_perl_treesitter_path(monkeypatch):
    content = "package My::Mod;\nuse strict;\nsub greet {\n  1;\n}\n"
    pkg_tok = tok("package", content, "My::Mod")
    pkg_stmt = FakeNode(
        "package_statement",
        content,
        content.index("package"),
        content.index(";\nuse") + 1,
        children=[pkg_tok],
        start_point=(0, 0),
        end_point=(0, 16),
    )
    use_stmt = tok("use_statement", content, "use strict;")
    sub_ident = tok("identifier", content, "greet")
    sub_decl = FakeNode(
        "subroutine_declaration",
        content,
        content.index("sub greet"),
        len(content),
        children=[sub_ident],
        start_point=(2, 0),
        end_point=(4, 1),
    )
    root = FakeNode(
        "source_file",
        content,
        0,
        len(content),
        children=[pkg_stmt, use_stmt, sub_decl],
    )
    install_fake_pack(monkeypatch, root)
    p = PerlParser()
    parsed = p.parse(content, "m.pm")
    by = {s.simple_name: s for s in parsed.symbols}
    assert "greet" in by and by["greet"].symbol_type == CodeSymbolType.FUNCTION
    assert "My::Mod" in by and by["My::Mod"].symbol_type == CodeSymbolType.PACKAGE
    assert any("use strict" in i for i in parsed.imports)


# ---------------------------------------------------------------------------
# Placeholder parsers (return empty symbols)
# ---------------------------------------------------------------------------
def test_placeholder_parsers():
    for parser, lang, ext in [
        (CSharpParser(), "csharp", ".cs"),
        (PhpParser(), "php", ".php"),
        (SwiftParser(), "swift", ".swift"),
        (KotlinParser(), "kotlin", ".kt"),
        (ScalaParser(), "scala", ".scala"),
        (LuaParser(), "lua", ".lua"),
        (HaskellParser(), "haskell", ".hs"),
        (ElixirParser(), "elixir", ".ex"),
    ]:
        assert parser.language == lang
        assert ext in parser.file_extensions
        parsed = parser.parse("anything", f"f{ext}")
        assert parsed.language == lang
        assert parsed.symbols == []


# ---------------------------------------------------------------------------
# CodeChunkingStrategy end-to-end
# ---------------------------------------------------------------------------
def test_chunker_python_end_to_end():
    chunker = create_code_chunker(languages=["python"])
    chunks = chunker.chunk(PY_SRC, "pkg/mod.py", {"author": "me"})
    assert chunks
    c0 = chunks[0]
    assert c0.metadata["chunking_strategy"] == "code"
    assert c0.metadata["chunk_type"] == "code"
    assert "symbol_type" in c0.metadata
    assert c0.metadata["author"] == "me"
    # chunk_id format
    assert "#" in c0.chunk_id


def test_chunker_language_via_metadata():
    chunker = create_code_chunker(languages=["python"])
    # source id has no .py extension; language supplied via metadata
    chunks = chunker.chunk(PY_SRC, "scratch_buffer", {"language": "python"})
    assert chunks


def test_chunker_detect_language():
    chunker = CodeChunkingStrategy(CodeChunkingConfig(languages=["python"]))
    assert chunker._detect_language("a.py") == "python"
    assert chunker._detect_language("a.rs") == "rust"
    assert chunker._detect_language("a.unknownext") is None


def test_chunker_fallback_to_semantic():
    # language not in parsers -> semantic fallback path
    chunker = CodeChunkingStrategy(CodeChunkingConfig(languages=["python"]))
    text = "Just some prose. " * 80
    chunks = chunker.chunk(text, "notes.txt", {})
    assert chunks
    assert all(c.metadata["chunk_type"] == "code_fallback" for c in chunks)
    assert all(c.metadata["chunking_strategy"] == "code" for c in chunks)


def test_chunker_init_all_languages():
    # Default: initialize parsers for every registered language
    chunker = CodeChunkingStrategy()
    # at least the common ones should be present
    assert "python" in chunker._parsers
    assert "rust" in chunker._parsers


def test_chunker_relations_in_metadata(monkeypatch):
    # Use the fake AST python path so relations get produced, then chunk.
    root, content = build_python_ast()
    install_fake_pack(monkeypatch, root)
    chunker = CodeChunkingStrategy(CodeChunkingConfig(languages=["python"]))
    chunks = chunker.chunk(content, "mod.py", {})
    # at least one chunk should carry relations metadata (fn -> helper)
    assert any("relations" in c.metadata for c in chunks)


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q"]))
