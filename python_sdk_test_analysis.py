#!/usr/bin/env python3
"""
Python SDK Test Analysis and Organization
"""

import os
import re
import ast
from pathlib import Path
from typing import Dict, List, Set

def analyze_test_file(file_path: str) -> Dict:
    """Analyze a Python test file to extract information"""
    
    try:
        with open(file_path, 'r') as f:
            content = f.read()
        
        info = {
            'file_path': file_path,
            'file_name': os.path.basename(file_path),
            'size_lines': len(content.splitlines()),
            'imports': [],
            'test_functions': [],
            'test_classes': [],
            'sdk_usage': [],
            'categories': set(),
            'protocols': set(),
            'coverage_areas': set()
        }
        
        # Analyze imports
        import_pattern = r'^(?:from|import)\s+([a-zA-Z0-9_.]+)'
        for match in re.finditer(import_pattern, content, re.MULTILINE):
            import_name = match.group(1)
            info['imports'].append(import_name)
            
            # Identify SDK usage patterns
            if 'proximadb' in import_name:
                info['sdk_usage'].append(import_name)
                
        # Find test functions and classes
        try:
            tree = ast.parse(content)
            for node in ast.walk(tree):
                if isinstance(node, ast.FunctionDef) and node.name.startswith('test_'):
                    info['test_functions'].append(node.name)
                elif isinstance(node, ast.ClassDef) and 'test' in node.name.lower():
                    info['test_classes'].append(node.name)
        except:
            # Fallback to regex if AST parsing fails
            test_func_pattern = r'def\s+(test_\w+)'
            info['test_functions'] = re.findall(test_func_pattern, content)
            
            test_class_pattern = r'class\s+(\w*[Tt]est\w*)'
            info['test_classes'] = re.findall(test_class_pattern, content)
        
        # Categorize based on content
        content_lower = content.lower()
        
        # Protocol detection
        if 'grpc' in content_lower or 'pb2' in content_lower:
            info['protocols'].add('gRPC')
        if 'rest' in content_lower or 'http' in content_lower or 'requests' in content_lower:
            info['protocols'].add('REST')
            
        # Category detection
        if 'collection' in content_lower:
            info['categories'].add('collection')
        if 'vector' in content_lower:
            info['categories'].add('vector')
        if 'search' in content_lower:
            info['categories'].add('search')
        if 'wal' in content_lower:
            info['categories'].add('wal')
        if 'flush' in content_lower:
            info['categories'].add('flush')
        if 'avro' in content_lower:
            info['categories'].add('avro')
        if 'benchmark' in content_lower or 'performance' in content_lower:
            info['categories'].add('performance')
        if 'persistence' in content_lower:
            info['categories'].add('persistence')
        if 'metadata' in content_lower:
            info['categories'].add('metadata')
            
        # Coverage area detection
        for sdk_module in ['unified_sdk', 'grpc_client', 'rest_client', 'unified_client', 'improved_rest_client']:
            if sdk_module in content:
                info['coverage_areas'].add(sdk_module)
                
        return info
        
    except Exception as e:
        return {
            'file_path': file_path,
            'file_name': os.path.basename(file_path),
            'error': str(e),
            'categories': set(),
            'protocols': set(),
            'coverage_areas': set()
        }

def analyze_all_tests():
    """Analyze all test files and create organization plan"""
    
    test_files = []
    
    # 1. Existing archived tests
    archive_path = Path('/workspace/archive/misc_tests')
    if archive_path.exists():
        for file_path in archive_path.glob('test_*.py'):
            test_files.append(str(file_path))
    
    # 2. Current Python SDK tests
    current_tests_path = Path('/workspace/clients/python/tests')
    if current_tests_path.exists():
        for file_path in current_tests_path.glob('test_*.py'):
            test_files.append(str(file_path))
            
    # 3. New test structure
    new_tests_path = Path('/workspace/clients/python/tests_new')
    if new_tests_path.exists():
        for file_path in new_tests_path.rglob('test_*.py'):
            test_files.append(str(file_path))
    
    print(f"📊 Found {len(test_files)} test files to analyze")
    
    # Analyze each file
    analyses = []
    for file_path in test_files:
        analysis = analyze_test_file(file_path)
        analyses.append(analysis)
    
    return analyses

def create_organization_plan(analyses: List[Dict]):
    """Create a comprehensive organization plan"""
    
    # Group by categories
    categorized = {
        'unit_tests': [],
        'integration_tests': [],
        'performance_tests': [],
        'protocol_tests': {
            'grpc': [],
            'rest': [],
            'unified': []
        },
        'feature_tests': {
            'collections': [],
            'vectors': [],
            'search': [],
            'wal': [],
            'persistence': []
        },
        'sdk_coverage': {
            'unified_sdk': [],
            'grpc_client': [],
            'rest_client': [],
            'legacy_client': []
        }
    }
    
    for analysis in analyses:
        if 'error' in analysis:
            continue
            
        file_name = analysis['file_name']
        categories = analysis['categories']
        protocols = analysis['protocols']
        coverage_areas = analysis['coverage_areas']
        
        # Classify test type
        if any(keyword in file_name.lower() for keyword in ['unit', 'simple', 'basic']):
            categorized['unit_tests'].append(analysis)
        elif any(keyword in file_name.lower() for keyword in ['integration', 'e2e', 'comprehensive']):
            categorized['integration_tests'].append(analysis)
        elif any(keyword in file_name.lower() for keyword in ['benchmark', 'performance', '10k', '100k', '1m']):
            categorized['performance_tests'].append(analysis)
        else:
            # Default to integration for comprehensive tests
            categorized['integration_tests'].append(analysis)
        
        # Protocol classification
        if 'gRPC' in protocols and 'REST' not in protocols:
            categorized['protocol_tests']['grpc'].append(analysis)
        elif 'REST' in protocols and 'gRPC' not in protocols:
            categorized['protocol_tests']['rest'].append(analysis)
        elif 'gRPC' in protocols and 'REST' in protocols:
            categorized['protocol_tests']['unified'].append(analysis)
            
        # Feature classification
        if 'collection' in categories:
            categorized['feature_tests']['collections'].append(analysis)
        if 'vector' in categories:
            categorized['feature_tests']['vectors'].append(analysis)
        if 'search' in categories:
            categorized['feature_tests']['search'].append(analysis)
        if 'wal' in categories:
            categorized['feature_tests']['wal'].append(analysis)
        if 'persistence' in categories:
            categorized['feature_tests']['persistence'].append(analysis)
            
        # SDK coverage
        for area in coverage_areas:
            if area in categorized['sdk_coverage']:
                categorized['sdk_coverage'][area].append(analysis)
    
    return categorized

def print_organization_summary(categorized: Dict):
    """Print organization summary"""
    
    print("\n📋 Python SDK Test Organization Summary")
    print("=" * 60)
    
    total_tests = 0
    
    print(f"\n🔧 Unit Tests: {len(categorized['unit_tests'])}")
    for test in categorized['unit_tests'][:5]:
        print(f"   • {test['file_name']}")
    if len(categorized['unit_tests']) > 5:
        print(f"   ... and {len(categorized['unit_tests']) - 5} more")
    total_tests += len(categorized['unit_tests'])
    
    print(f"\n🔗 Integration Tests: {len(categorized['integration_tests'])}")
    for test in categorized['integration_tests'][:5]:
        print(f"   • {test['file_name']}")
    if len(categorized['integration_tests']) > 5:
        print(f"   ... and {len(categorized['integration_tests']) - 5} more")
    total_tests += len(categorized['integration_tests'])
    
    print(f"\n⚡ Performance Tests: {len(categorized['performance_tests'])}")
    for test in categorized['performance_tests'][:5]:
        print(f"   • {test['file_name']}")
    if len(categorized['performance_tests']) > 5:
        print(f"   ... and {len(categorized['performance_tests']) - 5} more")
    total_tests += len(categorized['performance_tests'])
    
    print(f"\n🌐 Protocol Coverage:")
    print(f"   • gRPC: {len(categorized['protocol_tests']['grpc'])}")
    print(f"   • REST: {len(categorized['protocol_tests']['rest'])}")
    print(f"   • Unified: {len(categorized['protocol_tests']['unified'])}")
    
    print(f"\n🎯 Feature Coverage:")
    for feature, tests in categorized['feature_tests'].items():
        print(f"   • {feature}: {len(tests)}")
    
    print(f"\n📦 SDK Coverage:")
    for sdk, tests in categorized['sdk_coverage'].items():
        print(f"   • {sdk}: {len(tests)}")
    
    print(f"\n📊 Total Tests: {total_tests}")

def create_new_test_structure(categorized: Dict):
    """Create the new organized test structure"""
    
    structure = {
        'clients/python/tests_organized/': {
            'unit/': {
                'test_unified_sdk.py': 'Unit tests for unified SDK interface',
                'test_grpc_client.py': 'Unit tests for gRPC client',
                'test_rest_client.py': 'Unit tests for REST client',
                'test_exceptions.py': 'Unit tests for exception handling',
                'test_models.py': 'Unit tests for data models'
            },
            'integration/': {
                'test_collection_operations.py': 'Collection CRUD integration tests',
                'test_vector_operations.py': 'Vector insert/search integration tests',
                'test_protocol_compatibility.py': 'Cross-protocol compatibility tests',
                'test_persistence.py': 'Data persistence integration tests',
                'test_search_functionality.py': 'Search functionality integration tests'
            },
            'performance/': {
                'test_benchmark_10k.py': '10K vector benchmarks',
                'test_benchmark_100k.py': '100K vector benchmarks',
                'test_throughput.py': 'Throughput performance tests',
                'test_latency.py': 'Latency performance tests'
            },
            'e2e/': {
                'test_full_workflow.py': 'End-to-end workflow tests',
                'test_error_scenarios.py': 'Error handling scenarios',
                'test_edge_cases.py': 'Edge case testing'
            }
        }
    }
    
    return structure

if __name__ == "__main__":
    print("🔍 Analyzing Python SDK Test Files...")
    analyses = analyze_all_tests()
    
    print(f"\n📊 Analysis Complete: {len(analyses)} files analyzed")
    
    categorized = create_organization_plan(analyses)
    print_organization_summary(categorized)
    
    print(f"\n🏗️ Recommended New Test Structure:")
    structure = create_new_test_structure(categorized)
    
    def print_structure(struct, indent=0):
        for name, content in struct.items():
            print("  " * indent + f"📁 {name}")
            if isinstance(content, dict):
                print_structure(content, indent + 1)
            else:
                print("  " * (indent + 1) + f"📄 {content}")
    
    print_structure(structure)
    
    print(f"\n✅ Analysis completed. Ready for test organization!")