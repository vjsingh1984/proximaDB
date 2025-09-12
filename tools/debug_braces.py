#!/usr/bin/env python3
"""
Brace Counting Debug Tool for Rust Files

This tool helps debug unmatched braces, brackets, and parentheses in Rust code.
Useful for fixing compilation errors after large code removals or refactoring.

Usage:
    python3 tools/debug_braces.py src/storage/engines/impls/sst/mod.rs
    python3 tools/debug_braces.py src/ --recursive
"""

import sys
import os
from pathlib import Path

def count_braces_in_file(file_path):
    """Count braces, brackets, and parentheses in a Rust file."""
    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            lines = f.readlines()
            content = ''.join(lines)
    except Exception as e:
        print(f"Error reading {file_path}: {e}")
        return None
    
    # Track different types of delimiters
    brace_count = 0      # {}
    bracket_count = 0    # []
    paren_count = 0      # ()
    
    # Track position for error reporting
    line_num = 1
    char_pos = 0
    
    in_string = False
    in_char = False
    in_comment = False
    in_line_comment = False
    escape_next = False
    
    errors = []
    suggestions = []
    
    # Track opening delimiters for matching suggestions
    brace_stack = []     # Stack of (line_num, char_pos, char)
    bracket_stack = []
    paren_stack = []
    
    i = 0
    while i < len(content):
        char = content[i]
        
        # Track position
        if char == '\n':
            line_num += 1
            char_pos = 0
            in_line_comment = False
        else:
            char_pos += 1
        
        # Handle escape sequences
        if escape_next:
            escape_next = False
            i += 1
            continue
            
        if char == '\\' and (in_string or in_char):
            escape_next = True
            i += 1
            continue
        
        # Handle comments
        if not in_string and not in_char:
            if char == '/' and i + 1 < len(content):
                if content[i + 1] == '/':
                    in_line_comment = True
                    i += 2
                    continue
                elif content[i + 1] == '*':
                    in_comment = True
                    i += 2
                    continue
            elif char == '*' and i + 1 < len(content) and content[i + 1] == '/':
                in_comment = False
                i += 2
                continue
        
        # Skip if in comment
        if in_comment or in_line_comment:
            i += 1
            continue
        
        # Handle string literals
        if char == '"' and not in_char:
            in_string = not in_string
        elif char == "'" and not in_string:
            in_char = not in_char
        
        # Count delimiters only outside strings and comments
        if not in_string and not in_char:
            if char == '{':
                brace_count += 1
                brace_stack.append((line_num, char_pos, char))
            elif char == '}':
                brace_count -= 1
                if brace_stack:
                    opening = brace_stack.pop()
                    # Check for indentation issues
                    if len(lines) >= line_num:
                        open_indent = len(lines[opening[0]-1]) - len(lines[opening[0]-1].lstrip())
                        close_indent = len(lines[line_num-1]) - len(lines[line_num-1].lstrip())
                        if abs(open_indent - close_indent) > 4:  # Significant indentation difference
                            suggestions.append(f"Line {line_num}: Closing brace indentation ({close_indent}) differs significantly from opening brace at line {opening[0]} ({open_indent})")
                else:
                    errors.append(f"Line {line_num}:{char_pos}: Unexpected closing brace '}}' - no matching opening brace found")
                    if brace_stack:
                        last_open = brace_stack[-1]
                        suggestions.append(f"  → Consider: Missing opening brace or extra closing brace. Last unclosed opening brace at line {last_open[0]}:{last_open[1]}")
            elif char == '[':
                bracket_count += 1
                bracket_stack.append((line_num, char_pos, char))
            elif char == ']':
                bracket_count -= 1
                if bracket_stack:
                    bracket_stack.pop()
                else:
                    errors.append(f"Line {line_num}:{char_pos}: Unexpected closing bracket ']' (no matching opening bracket)")
            elif char == '(':
                paren_count += 1
                paren_stack.append((line_num, char_pos, char))
            elif char == ')':
                paren_count -= 1
                if paren_stack:
                    paren_stack.pop()
                else:
                    errors.append(f"Line {line_num}:{char_pos}: Unexpected closing parenthesis ')' (no matching opening parenthesis)")
        
        i += 1
    
    # Check for unclosed delimiters and provide suggestions
    if brace_count > 0:
        errors.append(f"End of file: {brace_count} unclosed braces '{{' remaining")
        if brace_stack:
            for line, pos, char in brace_stack[-3:]:  # Show last 3 unclosed braces
                line_content = lines[line-1].strip() if line <= len(lines) else "N/A"
                suggestions.append(f"  → Unclosed '{{' at line {line}:{pos}: {line_content}")
    if bracket_count > 0:
        errors.append(f"End of file: {bracket_count} unclosed brackets '[' remaining")
        if bracket_stack:
            for line, pos, char in bracket_stack[-3:]:
                line_content = lines[line-1].strip() if line <= len(lines) else "N/A"
                suggestions.append(f"  → Unclosed '[' at line {line}:{pos}: {line_content}")
    if paren_count > 0:
        errors.append(f"End of file: {paren_count} unclosed parentheses '(' remaining")
        if paren_stack:
            for line, pos, char in paren_stack[-3:]:
                line_content = lines[line-1].strip() if line <= len(lines) else "N/A"
                suggestions.append(f"  → Unclosed '(' at line {line}:{pos}: {line_content}")
    
    return {
        'file': file_path,
        'braces': brace_count,
        'brackets': bracket_count,
        'parens': paren_count,
        'errors': errors,
        'suggestions': suggestions,
        'balanced': len(errors) == 0 and brace_count == 0 and bracket_count == 0 and paren_count == 0
    }

def main():
    if len(sys.argv) < 2:
        print("Usage: python3 debug_braces.py <file_or_directory> [--recursive]")
        print("Examples:")
        print("  python3 debug_braces.py src/storage/engines/impls/sst/mod.rs")
        print("  python3 debug_braces.py src/ --recursive")
        sys.exit(1)
    
    path = sys.argv[1]
    recursive = len(sys.argv) > 2 and sys.argv[2] == '--recursive'
    
    if os.path.isfile(path):
        # Single file
        result = count_braces_in_file(path)
        if result:
            print(f"\n📁 File: {result['file']}")
            print(f"   Braces: {result['braces']} (balanced: {result['braces'] == 0})")
            print(f"   Brackets: {result['brackets']} (balanced: {result['brackets'] == 0})")
            print(f"   Parentheses: {result['parens']} (balanced: {result['parens'] == 0})")
            print(f"   Overall balanced: {result['balanced']}")
            
            if result['errors']:
                print(f"\n❌ ERRORS FOUND:")
                for error in result['errors']:
                    print(f"   {error}")
                    
                if result['suggestions']:
                    print(f"\n💡 SUGGESTIONS:")
                    for suggestion in result['suggestions']:
                        print(f"   {suggestion}")
            else:
                print(f"\n✅ No delimiter errors found")
    
    elif os.path.isdir(path) and recursive:
        # Recursive directory scan
        rust_files = list(Path(path).rglob("*.rs"))
        total_files = len(rust_files)
        error_files = []
        
        print(f"🔍 Scanning {total_files} Rust files in {path}...")
        
        for file_path in rust_files:
            result = count_braces_in_file(str(file_path))
            if result and not result['balanced']:
                error_files.append(result)
        
        print(f"\n📊 Summary:")
        print(f"   Total files: {total_files}")
        print(f"   Files with delimiter errors: {len(error_files)}")
        print(f"   Clean files: {total_files - len(error_files)}")
        
        if error_files:
            print(f"\n❌ FILES WITH ERRORS:")
            for result in error_files:
                print(f"\n📁 {result['file']}:")
                print(f"   Braces: {result['braces']}, Brackets: {result['brackets']}, Parens: {result['parens']}")
                for error in result['errors']:
                    print(f"   ❌ {error}")
        else:
            print(f"\n✅ All files have balanced delimiters!")
    
    else:
        print(f"Error: {path} is not a file, or use --recursive for directories")
        sys.exit(1)

if __name__ == "__main__":
    main()