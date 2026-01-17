#!/usr/bin/env python3
"""
Extract requirements from RECENT/IP spec RST files.

Generates a JSON database and markdown summary of all requirements.
"""

import re
import json
from pathlib import Path
from dataclasses import dataclass, asdict
from typing import Optional
import sys

@dataclass
class Requirement:
    id: str
    reqtype: str  # "Requirement" or "Information"
    source_file: str
    line_number: int
    text: str
    section: str = ""
    status: str = "valid"
    
def extract_requirements(rst_path: Path) -> list[Requirement]:
    """Extract all requirements from an RST file."""
    requirements = []
    content = rst_path.read_text()
    lines = content.split('\n')
    
    current_section = ""
    i = 0
    while i < len(lines):
        line = lines[i]
        
        # Track sections (headings)
        if i + 1 < len(lines) and lines[i+1].strip():
            next_line = lines[i+1].strip()
            if next_line and len(set(next_line)) == 1 and next_line[0] in '#*=-^"':
                if len(next_line) >= len(line.strip()) * 0.8:
                    current_section = line.strip()
        
        # Look for feat_req directive
        if line.strip().startswith('.. feat_req::'):
            req_id = None
            reqtype = "Unknown"
            status = "valid"
            text_lines = []
            start_line = i + 1
            
            # Parse the directive fields (lines starting with :field: value pattern)
            j = i + 1
            while j < len(lines):
                field_line = lines[j]
                stripped = field_line.strip()
                
                # Empty lines are okay within directive fields
                if stripped == '':
                    j += 1
                    continue
                
                # Check if this is a directive field (:fieldname: value) vs RST role (:role:`...`)
                is_directive_field = False
                if stripped.startswith(':'):
                    second_colon = stripped.find(':', 1)
                    if second_colon > 0:
                        after_field = stripped[second_colon+1:]
                        # Directive fields have space or empty after second colon
                        # RST roles have backtick after second colon
                        if not after_field.startswith('`'):
                            is_directive_field = True
                
                if is_directive_field:
                    if stripped.startswith(':id:'):
                        req_id = stripped.split(':id:')[1].strip()
                    elif stripped.startswith(':reqtype:'):
                        reqtype = stripped.split(':reqtype:')[1].strip()
                    elif stripped.startswith(':status:'):
                        status = stripped.split(':status:')[1].strip()
                    j += 1
                else:
                    # Non-field line - end of directive fields, start of content
                    break
            
            # Collect the requirement text (content after fields until next feat_req directive)
            while j < len(lines):
                current_line = lines[j]
                stripped = current_line.strip()
                
                # Stop at next feat_req directive
                if stripped.startswith('.. feat_req::'):
                    break
                # Skip rst-class styling directives (they're not content separators)
                if stripped.startswith('.. rst-class::'):
                    j += 1
                    continue
                # Stop at other directives that indicate new content
                if stripped.startswith('.. ') and not stripped.startswith('.. rst-class::'):
                    break
                # Stop at section headings
                if j + 1 < len(lines) and lines[j+1].strip():
                    next_stripped = lines[j+1].strip()
                    if next_stripped and len(set(next_stripped)) == 1 and next_stripped[0] in '#*=-^"':
                        break
                
                # Collect non-empty text
                # Skip directive fields (:field: value) but include RST roles (:role:`...`)
                if stripped:
                    # Check if it looks like a directive field: starts with :word: followed by space/value
                    # RST roles look like :word:`...` - they have backtick after the second colon
                    is_directive_field = False
                    if stripped.startswith(':'):
                        # Find second colon
                        second_colon = stripped.find(':', 1)
                        if second_colon > 0:
                            after_field = stripped[second_colon+1:]
                            # Directive fields have space or empty after, roles have backtick
                            if not after_field.startswith('`'):
                                is_directive_field = True
                    
                    if not is_directive_field:
                        text_lines.append(stripped)
                j += 1
            
            if req_id:
                text = ' '.join(text_lines)[:500]  # Truncate long texts
                requirements.append(Requirement(
                    id=req_id,
                    reqtype=reqtype,
                    source_file=rst_path.name,
                    line_number=start_line,
                    text=text,
                    section=current_section,
                    status=status,
                ))
            
            i = j
        else:
            i += 1
    
    return requirements

def main():
    spec_dir = Path(__file__).parent.parent.parent / 'src'
    output_dir = Path(__file__).parent.parent / 'spec-data'
    output_dir.mkdir(exist_ok=True)
    
    if not spec_dir.exists():
        print(f"Spec directory not found: {spec_dir}")
        print("Looking for RST files...")
        # Try alternative path
        spec_dir = Path('/mnt/fedora/home/daniel/side-projects/recentIpSpec/src')
    
    rst_files = list(spec_dir.glob('*.rst'))
    print(f"Found {len(rst_files)} RST files in {spec_dir}")
    
    all_requirements = []
    for rst_file in rst_files:
        reqs = extract_requirements(rst_file)
        print(f"  {rst_file.name}: {len(reqs)} requirements")
        all_requirements.extend(reqs)
    
    # Save as JSON
    json_path = output_dir / 'requirements.json'
    with open(json_path, 'w') as f:
        json.dump([asdict(r) for r in all_requirements], f, indent=2)
    print(f"\nSaved {len(all_requirements)} requirements to {json_path}")
    
    # Generate summary by type
    by_type = {}
    by_file = {}
    for req in all_requirements:
        by_type.setdefault(req.reqtype, []).append(req)
        by_file.setdefault(req.source_file, {'Requirement': 0, 'Information': 0})
        if req.reqtype in by_file[req.source_file]:
            by_file[req.source_file][req.reqtype] += 1
    
    print("\n=== Summary by Type ===")
    for reqtype, reqs in sorted(by_type.items()):
        print(f"  {reqtype}: {len(reqs)}")
    
    print("\n=== Summary by File ===")
    for filename, counts in sorted(by_file.items()):
        print(f"  {filename}: {counts['Requirement']} reqs, {counts['Information']} info")
    
    # Generate markdown summary
    md_path = output_dir / 'requirements-summary.md'
    with open(md_path, 'w') as f:
        f.write("# RECENT/IP Requirements Summary\n\n")
        f.write(f"Total: {len(all_requirements)} requirements\n\n")
        
        f.write("## By File\n\n")
        f.write("| File | Requirements | Information | Total |\n")
        f.write("|------|--------------|-------------|-------|\n")
        for filename, counts in sorted(by_file.items()):
            total = counts['Requirement'] + counts['Information']
            f.write(f"| {filename} | {counts['Requirement']} | {counts['Information']} | {total} |\n")
        
        f.write("\n## Requirements (Testable)\n\n")
        testable = [r for r in all_requirements if r.reqtype == 'Requirement']
        for req in sorted(testable, key=lambda r: r.id):
            text_preview = req.text[:100] + "..." if len(req.text) > 100 else req.text
            f.write(f"- **{req.id}** ({req.source_file}): {text_preview}\n")
    
    print(f"Saved summary to {md_path}")

if __name__ == '__main__':
    main()
