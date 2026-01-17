#!/usr/bin/env python3
"""
Extract covers!() annotations from Rust test files and generate a coverage report.

This script parses the compliance test files and extracts which requirements
are covered by which tests, creating a JSON mapping for COMPLIANCE.md generation.
"""

import json
import re
import sys
from pathlib import Path
from dataclasses import dataclass, field, asdict
from typing import Optional


@dataclass
class TestCoverage:
    """A single test and the requirements it covers."""
    test_name: str
    file_path: str
    line_number: int
    requirements: list[str]
    is_ignored: bool = False
    ignore_reason: Optional[str] = None


@dataclass 
class CoverageReport:
    """Full coverage report."""
    tests: list[TestCoverage] = field(default_factory=list)
    requirements_covered: dict[str, list[str]] = field(default_factory=dict)  # req_id -> [test_names]
    total_tests: int = 0
    total_requirements_covered: int = 0


def extract_covers_from_file(file_path: Path) -> list[TestCoverage]:
    """Extract all covers!() annotations and /// [feat_req_...] doc comments from a Rust test file."""
    content = file_path.read_text()
    coverages = []
    
    # Pattern to find test functions with optional doc comment and ignore attribute
    # Supports: #[test], #[test_log::test], #[tokio::test], etc.
    test_pattern = re.compile(
        r'(?:///\s*\[([^\]]+)\][^\n]*\n\s*)?'  # Optional /// [feat_req_xxx] doc comment
        r'(?:'
        r'#\[(?:test_log::test|tokio::test|test)\]\s*\n\s*#\[ignore(?:\s*=\s*"([^"]+)")?\]\s*\n|'  # test then ignore
        r'#\[ignore(?:\s*=\s*"([^"]+)")?\]\s*\n\s*#\[(?:test_log::test|tokio::test|test)\]\s*\n|'  # ignore then test
        r'#\[(?:test_log::test|tokio::test|test)\]\s*\n'  # Just test
        r')'
        r'\s*(?:async\s+)?fn\s+(\w+)\s*\(',
        re.MULTILINE
    )
    
    # Pattern to find covers!() macro calls
    covers_pattern = re.compile(
        r'covers!\s*\(\s*([^)]+)\s*\)',
        re.MULTILINE | re.DOTALL
    )
    
    # Pattern to find requirement IDs in doc comments (feat_req_xxx)
    doc_req_pattern = re.compile(r'feat_req_\w+')
    
    # Find all test functions with their positions
    tests = []
    for match in test_pattern.finditer(content):
        doc_comment = match.group(1)
        # Ignore reason can be in group 2 (test-then-ignore) or group 3 (ignore-then-test)
        ignore_reason = match.group(2) or match.group(3)
        test_name = match.group(4)
        start_pos = match.end()
        line_number = content[:match.start()].count('\n') + 1
        
        # Extract requirement refs from doc comment
        doc_reqs = []
        if doc_comment:
            doc_reqs = doc_req_pattern.findall(doc_comment)
        
        tests.append({
            'name': test_name,
            'start': start_pos,
            'line': line_number,
            'ignored': ignore_reason is not None,
            'ignore_reason': ignore_reason,
            'doc_reqs': doc_reqs,
        })
    
    # Find the end of each test function (next test or end of file)
    for i, test in enumerate(tests):
        if i + 1 < len(tests):
            test['end'] = tests[i + 1]['start']
        else:
            test['end'] = len(content)
    
    # Extract covers!() from each test function
    for test in tests:
        test_body = content[test['start']:test['end']]
        requirements = list(test['doc_reqs'])  # Start with doc comment reqs
        
        for covers_match in covers_pattern.finditer(test_body):
            # Parse the requirement IDs from covers!(id1, id2, ...)
            req_str = covers_match.group(1)
            # Split by comma and clean up
            reqs = [r.strip() for r in req_str.replace('\n', ' ').split(',')]
            reqs = [r for r in reqs if r and not r.startswith('//')]
            requirements.extend(reqs)
        
        if requirements:
            coverages.append(TestCoverage(
                test_name=test['name'],
                file_path=str(file_path.relative_to(file_path.parent.parent.parent)),
                line_number=test['line'],
                requirements=requirements,
                is_ignored=test['ignored'],
                ignore_reason=test['ignore_reason']
            ))
    
    return coverages


def build_coverage_report(test_dir: Path) -> CoverageReport:
    """Build a complete coverage report from all test files."""
    report = CoverageReport()
    
    # Find all .rs files in compliance directory recursively
    for rs_file in sorted(test_dir.rglob("*.rs")):
        if rs_file.name == "mod.rs":
            continue
        
        coverages = extract_covers_from_file(rs_file)
        report.tests.extend(coverages)
    
    # Build reverse mapping: requirement -> tests
    for test in report.tests:
        for req in test.requirements:
            if req not in report.requirements_covered:
                report.requirements_covered[req] = []
            report.requirements_covered[req].append(test.test_name)
    
    report.total_tests = len(report.tests)
    report.total_requirements_covered = len(report.requirements_covered)
    
    return report


def main():
    # Find the compliance test directory
    script_dir = Path(__file__).parent
    project_root = script_dir.parent
    test_dir = project_root / "tests" / "compliance"
    
    if not test_dir.exists():
        print(f"Error: Test directory not found: {test_dir}", file=sys.stderr)
        sys.exit(1)
    
    # Build the coverage report
    report = build_coverage_report(test_dir)
    
    # Output as JSON
    output = {
        "total_tests_with_coverage": report.total_tests,
        "total_requirements_covered": report.total_requirements_covered,
        "tests": [asdict(t) for t in report.tests],
        "requirements_to_tests": report.requirements_covered
    }
    
    # Write to spec-data directory
    output_path = project_root / "spec-data" / "coverage.json"
    output_path.write_text(json.dumps(output, indent=2))
    print(f"Coverage report written to: {output_path}")
    
    # Print summary
    print(f"\nSummary:")
    print(f"  Tests with covers!(): {report.total_tests}")
    print(f"  Unique requirements covered: {report.total_requirements_covered}")
    
    # Print requirements by count
    print(f"\nRequirements by coverage count:")
    for req, tests in sorted(report.requirements_covered.items(), key=lambda x: -len(x[1])):
        print(f"  {req}: {len(tests)} test(s)")


if __name__ == "__main__":
    main()
