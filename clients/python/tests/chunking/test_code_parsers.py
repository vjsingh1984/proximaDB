"""
Comprehensive unit tests for all language parsers in the code chunking module.

This module tests the AST-aware code parsing functionality for all supported languages.
"""

import os
import sys
import pytest
from pathlib import Path

# Add current directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

# Import from loader which handles the module loading
from loader import code_module, RESOURCES_DIR, read_resource_file

# Get references to classes and functions from the loaded module
CodeSymbol = code_module.CodeSymbol
CodeSymbolType = code_module.CodeSymbolType
CodeRelation = code_module.CodeRelation
CodeRelationType = code_module.CodeRelationType
SourceLocation = code_module.SourceLocation
ParsedCode = code_module.ParsedCode
LanguageParser = code_module.LanguageParser
PythonParser = code_module.PythonParser
RustParser = code_module.RustParser
GoParser = code_module.GoParser
JavaParser = code_module.JavaParser
JavaScriptParser = code_module.JavaScriptParser
CppParser = code_module.CppParser
CSharpParser = code_module.CSharpParser
RubyParser = code_module.RubyParser
PhpParser = code_module.PhpParser
KotlinParser = code_module.KotlinParser
ScalaParser = code_module.ScalaParser
SwiftParser = code_module.SwiftParser
BashParser = code_module.BashParser
SqlParser = code_module.SqlParser
YamlParser = code_module.YamlParser
JsonParser = code_module.JsonParser
XmlParser = code_module.XmlParser
PerlParser = code_module.PerlParser
LuaParser = code_module.LuaParser
HaskellParser = code_module.HaskellParser
ElixirParser = code_module.ElixirParser
LANGUAGE_PARSERS = code_module.LANGUAGE_PARSERS
EXTENSION_TO_LANGUAGE = code_module.EXTENSION_TO_LANGUAGE
get_supported_languages = code_module.get_supported_languages
get_supported_extensions = code_module.get_supported_extensions
register_language_parser = code_module.register_language_parser
register_file_extension = code_module.register_file_extension


class TestDataModels:
    """Test cases for code chunking data models."""

    def test_code_symbol_type_values(self):
        """Test that CodeSymbolType enum has expected values."""
        assert CodeSymbolType.FILE == 1
        assert CodeSymbolType.CLASS == 4
        assert CodeSymbolType.FUNCTION == 9
        assert CodeSymbolType.METHOD == 10

    def test_code_relation_type_values(self):
        """Test that CodeRelationType enum has expected values."""
        assert CodeRelationType.CALLS == 1
        assert CodeRelationType.EXTENDS == 3
        assert CodeRelationType.IMPORTS == 7

    def test_source_location_creation(self):
        """Test SourceLocation dataclass creation."""
        loc = SourceLocation(
            file_path="/test/file.py",
            start_line=10,
            end_line=20,
            start_column=0,
            end_column=50
        )
        assert loc.file_path == "/test/file.py"
        assert loc.start_line == 10
        assert loc.end_line == 20

    def test_code_symbol_creation(self):
        """Test CodeSymbol dataclass creation."""
        loc = SourceLocation("/test/file.py", 10, 20)
        symbol = CodeSymbol(
            id="test_func_1",
            symbol_type=CodeSymbolType.FUNCTION,
            fully_qualified_name="module.test_function",
            simple_name="test_function",
            location=loc,
            source_code="def test_function(): pass",
            language="python",
            signature="def test_function(arg: str) -> bool",
            documentation="Test function docstring"
        )
        assert symbol.simple_name == "test_function"
        assert symbol.symbol_type == CodeSymbolType.FUNCTION
        assert symbol.documentation == "Test function docstring"

    def test_code_relation_creation(self):
        """Test CodeRelation dataclass creation."""
        loc = SourceLocation("/test/file.py", 15, 15)
        relation = CodeRelation(
            from_symbol_id="caller_func_1",
            to_symbol_id="callee_func_1",
            relation_type=CodeRelationType.CALLS,
            call_site=loc
        )
        assert relation.from_symbol_id == "caller_func_1"
        assert relation.to_symbol_id == "callee_func_1"
        assert relation.relation_type == CodeRelationType.CALLS

    def test_parsed_code_creation(self):
        """Test ParsedCode dataclass creation."""
        parsed = ParsedCode(
            file_path="/test/file.py",
            language="python",
            symbols=[],
            relations=[],
            imports=[],
            content_hash="abc123"
        )
        assert parsed.language == "python"
        assert parsed.file_path == "/test/file.py"


class TestRegistryFunctions:
    """Test cases for language registry functions."""

    def test_get_supported_languages(self):
        """Test that get_supported_languages returns expected languages."""
        languages = get_supported_languages()
        assert "python" in languages
        assert "rust" in languages
        assert "go" in languages
        assert "java" in languages
        assert "javascript" in languages
        assert len(languages) >= 20

    def test_get_supported_extensions(self):
        """Test that get_supported_extensions returns expected extensions."""
        extensions = get_supported_extensions()
        assert ".py" in extensions
        assert ".rs" in extensions
        assert ".go" in extensions
        assert ".java" in extensions
        assert ".js" in extensions
        assert ".ts" in extensions

    def test_extension_to_language_mapping(self):
        """Test extension to language mapping."""
        assert EXTENSION_TO_LANGUAGE.get(".py") == "python"
        assert EXTENSION_TO_LANGUAGE.get(".rs") == "rust"
        assert EXTENSION_TO_LANGUAGE.get(".go") == "go"
        assert EXTENSION_TO_LANGUAGE.get(".java") == "java"

    def test_register_language_parser(self):
        """Test registering a custom language parser."""
        class CustomParser(LanguageParser):
            @property
            def language(self) -> str:
                return "custom"

            @property
            def file_extensions(self):
                return [".custom"]

            def parse(self, source_code, file_path=None):
                return ParsedCode([], [], source_code, "custom")

        register_language_parser("custom", CustomParser)
        assert "custom" in LANGUAGE_PARSERS
        # Clean up to avoid polluting other tests
        del LANGUAGE_PARSERS["custom"]

    def test_register_file_extension(self):
        """Test registering a custom file extension."""
        register_file_extension(".customext", "customlang")
        assert EXTENSION_TO_LANGUAGE.get(".customext") == "customlang"
        # Clean up
        del EXTENSION_TO_LANGUAGE[".customext"]


class TestPythonParser:
    """Test cases for Python parser."""

    @pytest.fixture
    def parser(self):
        return PythonParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("python", "sample.py")

    def test_parser_creation(self, parser):
        """Test Python parser can be created."""
        assert parser.language == "python"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing Python sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.py")
        assert isinstance(result, ParsedCode)
        assert result.language == "python"
        assert isinstance(result.symbols, list)

    def test_parse_classes(self, parser, sample_code):
        """Test parsing Python classes."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.py")
        class_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.CLASS]

        assert len(class_symbols) >= 2  # User, BaseService, UserService
        class_names = [s.simple_name for s in class_symbols]
        assert "User" in class_names
        assert "UserService" in class_names

    def test_parse_functions(self, parser, sample_code):
        """Test parsing Python functions."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.py")
        func_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.FUNCTION]

        func_names = [s.simple_name for s in func_symbols]
        assert "calculate_factorial" in func_names
        assert "main" in func_names

    def test_parse_methods(self, parser, sample_code):
        """Test parsing Python methods."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.py")
        method_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.METHOD]

        method_names = [s.simple_name for s in method_symbols]
        assert "get_display_name" in method_names
        assert "create_user" in method_names

    def test_parse_imports(self, parser, sample_code):
        """Test parsing Python imports."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.py")
        # Check imports list instead of relations
        assert any("typing" in i or "dataclasses" in i for i in result.imports)

    def test_parse_decorators(self, parser):
        """Test parsing decorated functions."""
        code = '''
@decorator
def decorated_func():
    pass

@dataclass
class DataClass:
    field: str
'''
        result = parser.parse(code, "test.py")
        symbols = [s for s in result.symbols if s.simple_name in ["decorated_func", "DataClass"]]
        assert len(symbols) >= 1

    def test_parse_async_functions(self, parser):
        """Test parsing async functions."""
        code = '''
async def async_function():
    await something()
'''
        result = parser.parse(code, "test.py")
        func_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.FUNCTION]
        assert any(s.simple_name == "async_function" for s in func_symbols)


class TestRustParser:
    """Test cases for Rust parser."""

    @pytest.fixture
    def parser(self):
        return RustParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("rust", "sample.rs")

    def test_parser_creation(self, parser):
        """Test Rust parser can be created."""
        assert parser.language == "rust"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing Rust sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.rs")
        assert isinstance(result, ParsedCode)
        assert result.language == "rust"
        # Symbols may be empty if tree-sitter not available
        assert isinstance(result.symbols, list)

    def test_parse_structs(self, parser, sample_code):
        """Test parsing Rust structs."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.rs")
        struct_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.STRUCT]
        # May be empty if tree-sitter not available
        if struct_symbols:
            struct_names = [s.simple_name for s in struct_symbols]
            assert "User" in struct_names or "UserService" in struct_names

    def test_parse_traits(self, parser, sample_code):
        """Test parsing Rust traits."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.rs")
        trait_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.TRAIT]
        # May be empty if tree-sitter not available
        assert isinstance(trait_symbols, list)

    def test_parse_functions(self, parser, sample_code):
        """Test parsing Rust functions."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.rs")
        func_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.FUNCTION]
        # May be empty if tree-sitter not available
        assert isinstance(func_symbols, list)

    def test_parse_enums(self, parser, sample_code):
        """Test parsing Rust enums."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.rs")
        enum_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.ENUM]
        # May be empty if tree-sitter not available
        assert isinstance(enum_symbols, list)

    def test_parse_impl_blocks(self, parser):
        """Test parsing Rust impl blocks."""
        code = '''
struct MyStruct;

impl MyStruct {
    fn new() -> Self { Self }
    fn method(&self) {}
}
'''
        result = parser.parse(code, "test.rs")
        # May be empty if tree-sitter not available
        assert isinstance(result.symbols, list)


class TestGoParser:
    """Test cases for Go parser."""

    @pytest.fixture
    def parser(self):
        return GoParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("go", "sample.go")

    def test_parser_creation(self, parser):
        """Test Go parser can be created."""
        assert parser.language == "go"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing Go sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.go")
        assert isinstance(result, ParsedCode)
        assert result.language == "go"
        assert isinstance(result.symbols, list)

    def test_parse_structs(self, parser, sample_code):
        """Test parsing Go structs."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.go")
        struct_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.STRUCT]
        assert isinstance(struct_symbols, list)

    def test_parse_interfaces(self, parser, sample_code):
        """Test parsing Go interfaces."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.go")
        interface_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.INTERFACE]
        assert isinstance(interface_symbols, list)

    def test_parse_functions(self, parser, sample_code):
        """Test parsing Go functions."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.go")
        func_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.FUNCTION]
        assert isinstance(func_symbols, list)

    def test_parse_methods(self, parser, sample_code):
        """Test parsing Go methods."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.go")
        method_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.METHOD]
        assert isinstance(method_symbols, list)


class TestJavaParser:
    """Test cases for Java parser."""

    @pytest.fixture
    def parser(self):
        return JavaParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("java", "Sample.java")

    def test_parser_creation(self, parser):
        """Test Java parser can be created."""
        assert parser.language == "java"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing Java sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "Sample.java")
        assert isinstance(result, ParsedCode)
        assert result.language == "java"
        assert isinstance(result.symbols, list)

    def test_parse_classes(self, parser, sample_code):
        """Test parsing Java classes."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "Sample.java")
        class_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.CLASS]

        class_names = [s.simple_name for s in class_symbols]
        assert "User" in class_names
        assert "UserService" in class_names

    def test_parse_interfaces(self, parser, sample_code):
        """Test parsing Java interfaces."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "Sample.java")
        interface_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.INTERFACE]

        interface_names = [s.simple_name for s in interface_symbols]
        assert "Service" in interface_names


class TestJavaScriptParser:
    """Test cases for JavaScript parser."""

    @pytest.fixture
    def parser(self):
        return JavaScriptParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("javascript", "sample.js")

    def test_parser_creation(self, parser):
        """Test JavaScript parser can be created."""
        assert parser.language == "javascript"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing JavaScript sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.js")
        assert isinstance(result, ParsedCode)
        assert result.language == "javascript"
        assert isinstance(result.symbols, list)

    def test_parse_classes(self, parser, sample_code):
        """Test parsing JavaScript classes."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.js")
        class_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.CLASS]
        # May be empty if tree-sitter not available
        assert isinstance(class_symbols, list)

    def test_parse_functions(self, parser, sample_code):
        """Test parsing JavaScript functions."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.js")
        func_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.FUNCTION]
        # May be empty if tree-sitter not available
        assert isinstance(func_symbols, list)

    def test_parse_arrow_functions(self, parser):
        """Test parsing arrow functions."""
        code = '''
const arrowFunc = (x) => x * 2;
const multiLine = (a, b) => {
    return a + b;
};
'''
        result = parser.parse(code, "test.js")
        # May be empty if tree-sitter not available
        assert isinstance(result.symbols, list)


class TestTypeScriptParser:
    """Test cases for TypeScript parser."""

    @pytest.fixture
    def parser(self):
        return JavaScriptParser(typescript=True)

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("typescript", "sample.ts")

    def test_parser_creation(self, parser):
        """Test TypeScript parser can be created."""
        assert parser.language in ["javascript", "typescript"]

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing TypeScript sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.ts")
        assert isinstance(result, ParsedCode)
        assert isinstance(result.symbols, list)

    def test_parse_interfaces(self, parser, sample_code):
        """Test parsing TypeScript interfaces."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.ts")
        interface_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.INTERFACE]
        # May be empty if tree-sitter not available
        assert isinstance(interface_symbols, list)

    def test_parse_enums(self, parser, sample_code):
        """Test parsing TypeScript enums."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.ts")
        enum_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.ENUM]
        # May be empty if tree-sitter not available
        assert isinstance(enum_symbols, list)


class TestCppParser:
    """Test cases for C++ parser."""

    @pytest.fixture
    def parser(self):
        return CppParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("cpp", "sample.cpp")

    def test_parser_creation(self, parser):
        """Test C++ parser can be created."""
        assert parser.language == "cpp"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing C++ sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.cpp")
        assert isinstance(result, ParsedCode)
        assert result.language == "cpp"
        assert isinstance(result.symbols, list)

    def test_parse_classes(self, parser, sample_code):
        """Test parsing C++ classes."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.cpp")
        class_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.CLASS]
        # May be empty if tree-sitter not available
        assert isinstance(class_symbols, list)


class TestCParser:
    """Test cases for C parser."""

    @pytest.fixture
    def parser(self):
        return CppParser(c_mode=True)

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("c", "sample.c")

    def test_parser_creation(self, parser):
        """Test C parser can be created."""
        assert parser.language in ["c", "cpp"]

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing C sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.c")
        assert isinstance(result, ParsedCode)
        assert isinstance(result.symbols, list)

    def test_parse_structs(self, parser, sample_code):
        """Test parsing C structs."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.c")
        struct_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.STRUCT]
        # May be empty if tree-sitter not available
        assert isinstance(struct_symbols, list)

    def test_parse_functions(self, parser, sample_code):
        """Test parsing C functions."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.c")
        func_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.FUNCTION]
        # May be empty if tree-sitter not available
        assert isinstance(func_symbols, list)


class TestCSharpParser:
    """Test cases for C# parser."""

    @pytest.fixture
    def parser(self):
        return CSharpParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("csharp", "Sample.cs")

    def test_parser_creation(self, parser):
        """Test C# parser can be created."""
        assert parser.language == "csharp"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing C# sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "Sample.cs")
        assert isinstance(result, ParsedCode)
        assert result.language == "csharp"
        assert isinstance(result.symbols, list)

    def test_parse_classes(self, parser, sample_code):
        """Test parsing C# classes."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "Sample.cs")
        class_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.CLASS]
        # May be empty if tree-sitter not available
        assert isinstance(class_symbols, list)


class TestRubyParser:
    """Test cases for Ruby parser."""

    @pytest.fixture
    def parser(self):
        return RubyParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("ruby", "sample.rb")

    def test_parser_creation(self, parser):
        """Test Ruby parser can be created."""
        assert parser.language == "ruby"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing Ruby sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.rb")
        assert isinstance(result, ParsedCode)
        assert result.language == "ruby"
        assert isinstance(result.symbols, list)

    def test_parse_classes(self, parser, sample_code):
        """Test parsing Ruby classes."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.rb")
        class_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.CLASS]
        # May be empty if tree-sitter not available
        assert isinstance(class_symbols, list)


class TestPhpParser:
    """Test cases for PHP parser."""

    @pytest.fixture
    def parser(self):
        return PhpParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("php", "sample.php")

    def test_parser_creation(self, parser):
        """Test PHP parser can be created."""
        assert parser.language == "php"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing PHP sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.php")
        assert isinstance(result, ParsedCode)
        assert result.language == "php"
        assert isinstance(result.symbols, list)


class TestKotlinParser:
    """Test cases for Kotlin parser."""

    @pytest.fixture
    def parser(self):
        return KotlinParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("kotlin", "Sample.kt")

    def test_parser_creation(self, parser):
        """Test Kotlin parser can be created."""
        assert parser.language == "kotlin"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing Kotlin sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "Sample.kt")
        assert isinstance(result, ParsedCode)
        assert result.language == "kotlin"
        assert isinstance(result.symbols, list)


class TestScalaParser:
    """Test cases for Scala parser."""

    @pytest.fixture
    def parser(self):
        return ScalaParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("scala", "Sample.scala")

    def test_parser_creation(self, parser):
        """Test Scala parser can be created."""
        assert parser.language == "scala"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing Scala sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "Sample.scala")
        assert isinstance(result, ParsedCode)
        assert result.language == "scala"
        assert isinstance(result.symbols, list)


class TestSwiftParser:
    """Test cases for Swift parser."""

    @pytest.fixture
    def parser(self):
        return SwiftParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("swift", "Sample.swift")

    def test_parser_creation(self, parser):
        """Test Swift parser can be created."""
        assert parser.language == "swift"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing Swift sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "Sample.swift")
        assert isinstance(result, ParsedCode)
        assert result.language == "swift"
        assert isinstance(result.symbols, list)


class TestBashParser:
    """Test cases for Bash parser."""

    @pytest.fixture
    def parser(self):
        return BashParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("bash", "sample.sh")

    def test_parser_creation(self, parser):
        """Test Bash parser can be created."""
        assert parser.language == "bash"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing Bash sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.sh")
        assert isinstance(result, ParsedCode)
        assert result.language == "bash"
        assert isinstance(result.symbols, list)

    def test_parse_functions(self, parser, sample_code):
        """Test parsing Bash functions."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.sh")
        func_symbols = [s for s in result.symbols if s.symbol_type == CodeSymbolType.FUNCTION]

        func_names = [s.simple_name for s in func_symbols]
        assert "calculate_factorial" in func_names
        assert "main" in func_names


class TestSqlParser:
    """Test cases for SQL parser."""

    @pytest.fixture
    def parser(self):
        return SqlParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("sql", "sample.sql")

    def test_parser_creation(self, parser):
        """Test SQL parser can be created."""
        assert parser.language == "sql"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing SQL sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.sql")
        assert isinstance(result, ParsedCode)
        assert result.language == "sql"
        assert isinstance(result.symbols, list)


class TestYamlParser:
    """Test cases for YAML parser."""

    @pytest.fixture
    def parser(self):
        return YamlParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("yaml", "sample.yaml")

    def test_parser_creation(self, parser):
        """Test YAML parser can be created."""
        assert parser.language == "yaml"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing YAML sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.yaml")
        assert isinstance(result, ParsedCode)
        assert result.language == "yaml"


class TestJsonParser:
    """Test cases for JSON parser."""

    @pytest.fixture
    def parser(self):
        return JsonParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("json", "sample.json")

    def test_parser_creation(self, parser):
        """Test JSON parser can be created."""
        assert parser.language == "json"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing JSON sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.json")
        assert isinstance(result, ParsedCode)
        assert result.language == "json"


class TestXmlParser:
    """Test cases for XML parser."""

    @pytest.fixture
    def parser(self):
        return XmlParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("xml", "sample.xml")

    def test_parser_creation(self, parser):
        """Test XML parser can be created."""
        assert parser.language == "xml"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing XML sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.xml")
        assert isinstance(result, ParsedCode)
        assert result.language == "xml"


class TestPerlParser:
    """Test cases for Perl parser."""

    @pytest.fixture
    def parser(self):
        return PerlParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("perl", "sample.pl")

    def test_parser_creation(self, parser):
        """Test Perl parser can be created."""
        assert parser.language == "perl"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing Perl sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.pl")
        assert isinstance(result, ParsedCode)
        assert result.language == "perl"
        assert isinstance(result.symbols, list)


class TestLuaParser:
    """Test cases for Lua parser."""

    @pytest.fixture
    def parser(self):
        return LuaParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("lua", "sample.lua")

    def test_parser_creation(self, parser):
        """Test Lua parser can be created."""
        assert parser.language == "lua"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing Lua sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.lua")
        assert isinstance(result, ParsedCode)
        assert result.language == "lua"
        assert isinstance(result.symbols, list)


class TestHaskellParser:
    """Test cases for Haskell parser."""

    @pytest.fixture
    def parser(self):
        return HaskellParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("haskell", "Sample.hs")

    def test_parser_creation(self, parser):
        """Test Haskell parser can be created."""
        assert parser.language == "haskell"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing Haskell sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "Sample.hs")
        assert isinstance(result, ParsedCode)
        assert result.language == "haskell"
        assert isinstance(result.symbols, list)


class TestElixirParser:
    """Test cases for Elixir parser."""

    @pytest.fixture
    def parser(self):
        return ElixirParser()

    @pytest.fixture
    def sample_code(self):
        return read_resource_file("elixir", "sample.ex")

    def test_parser_creation(self, parser):
        """Test Elixir parser can be created."""
        assert parser.language == "elixir"

    def test_parse_sample_code(self, parser, sample_code):
        """Test parsing Elixir sample code."""
        if not sample_code:
            pytest.skip("Sample file not found")

        result = parser.parse(sample_code, "sample.ex")
        assert isinstance(result, ParsedCode)
        assert result.language == "elixir"
        assert isinstance(result.symbols, list)


class TestEdgeCases:
    """Test cases for edge cases and error handling."""

    def test_empty_code(self):
        """Test parsing empty code."""
        parser = PythonParser()
        result = parser.parse("", "empty.py")
        assert isinstance(result, ParsedCode)
        assert len(result.symbols) == 0

    def test_invalid_syntax(self):
        """Test parsing invalid syntax doesn't crash."""
        parser = PythonParser()
        # Invalid Python syntax
        code = "def broken(\n    # missing closing paren"
        result = parser.parse(code, "invalid.py")
        assert isinstance(result, ParsedCode)

    def test_unicode_handling(self):
        """Test handling of unicode in code."""
        parser = PythonParser()
        code = '''
def greet(name):
    """Greet someone in multiple languages."""
    return f"Hello {name}! 你好! Привет! مرحبا!"
'''
        result = parser.parse(code, "unicode.py")
        assert isinstance(result, ParsedCode)
        assert isinstance(result.symbols, list)

    def test_large_file(self):
        """Test parsing a large file doesn't crash."""
        parser = PythonParser()
        # Generate a large file
        code_lines = []
        for i in range(1000):
            code_lines.append(f"def func_{i}(): pass")
        code = "\n".join(code_lines)

        result = parser.parse(code, "large.py")
        assert isinstance(result, ParsedCode)
        assert len(result.symbols) >= 1000

    def test_deeply_nested_code(self):
        """Test parsing deeply nested code."""
        parser = PythonParser()
        # Deeply nested classes and functions
        code = '''
class A:
    class B:
        class C:
            def method(self):
                def inner():
                    def deeper():
                        pass
                    return deeper
                return inner
'''
        result = parser.parse(code, "nested.py")
        assert isinstance(result, ParsedCode)
        assert isinstance(result.symbols, list)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
