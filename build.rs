//! Build script to generate compliance documentation for rustdoc.
//!
//! This reads the requirements and coverage data from spec-data/ and generates
//! a markdown file that is included in rustdoc via `include_str!()`.
//!
//! Automatically runs `scripts/extract_coverage.py` to extract test coverage
//! annotations before generating the documentation.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Get the current git commit hash, or "main" as fallback.
fn get_git_commit() -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "main".to_string())
}

/// Run the Python script to extract coverage annotations from test files.
fn run_extract_coverage(project_root: &Path) {
    let script_path = project_root.join("scripts/extract_coverage.py");
    
    if !script_path.exists() {
        eprintln!("Warning: extract_coverage.py not found, skipping coverage extraction");
        return;
    }

    let output = Command::new("python3")
        .arg(&script_path)
        .current_dir(project_root)
        .output();

    match output {
        Ok(result) => {
            if !result.status.success() {
                eprintln!(
                    "Warning: extract_coverage.py failed: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
            }
        }
        Err(e) => {
            eprintln!("Warning: Could not run extract_coverage.py: {}", e);
        }
    }
}

fn main() {
    // Re-run if test files change (coverage annotations)
    println!("cargo:rerun-if-changed=tests/compliance/");
    // Re-run if spec data changes
    println!("cargo:rerun-if-changed=spec-data/requirements.json");
    // Re-run if git HEAD changes (new commits)
    println!("cargo:rerun-if-changed=.git/HEAD");
    // Re-run if the extraction script changes
    println!("cargo:rerun-if-changed=scripts/extract_coverage.py");

    let project_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let project_root = Path::new(&project_root);

    // Run Python script to extract coverage from test files
    run_extract_coverage(project_root);

    let requirements_path = project_root.join("spec-data/requirements.json");
    let coverage_path = project_root.join("spec-data/coverage.json");
    let output_path = project_root.join("docs/generated/compliance.md");

    // Get git commit for stable links
    let git_commit = get_git_commit();

    // Ensure output directory exists
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).ok();
    }

    // Load requirements
    let requirements: Vec<Requirement> = match fs::read_to_string(&requirements_path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => {
            eprintln!("Warning: Could not read requirements.json, generating stub");
            write_stub(&output_path);
            return;
        }
    };

    // Load coverage
    let coverage: Coverage = match fs::read_to_string(&coverage_path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Coverage::default(),
    };

    // Generate markdown
    let markdown = generate_compliance_doc(&requirements, &coverage, &git_commit);
    fs::write(&output_path, markdown).expect("Failed to write compliance.md");
}

fn write_stub(path: &Path) {
    let stub = r#"# Specification Compliance

> ⚠️ Compliance data not available. Run `python scripts/extract_coverage.py` to generate.
"#;
    fs::write(path, stub).ok();
}

#[derive(Debug, Default, serde::Deserialize)]
struct Requirement {
    id: String,
    reqtype: String,
    source_file: String,
    #[allow(dead_code)]
    line_number: u32,
    text: String,
    section: String,
    #[allow(dead_code)]
    status: String,
}

#[derive(Debug, Default, serde::Deserialize)]
struct Coverage {
    #[allow(dead_code)]
    total_tests_with_coverage: u32,
    #[allow(dead_code)]
    total_requirements_covered: u32,
    tests: Vec<TestCoverage>,
    requirements_to_tests: HashMap<String, Vec<String>>,
}

#[derive(Debug, serde::Deserialize)]
struct TestCoverage {
    test_name: String,
    file_path: String,
    line_number: u32,
    requirements: Vec<String>,
    #[allow(dead_code)]
    is_ignored: bool,
    #[allow(dead_code)]
    ignore_reason: Option<String>,
}

fn generate_compliance_doc(requirements: &[Requirement], coverage: &Coverage, git_commit: &str) -> String {
    let mut out = String::new();

    // Header
    out.push_str("# Specification Compliance\n\n");
    out.push_str("This document provides traceability between the SOME/IP specification requirements\n");
    out.push_str("and the compliance test suite. **Auto-generated at build time.**\n\n");
    out.push_str(&format!("*Git commit: [`{}`](https://github.com/daniel-freiermuth/recentip/commit/{})*\n\n", 
        &git_commit[..git_commit.len().min(8)], git_commit));

    // Build test lookup: test_name -> TestCoverage
    let test_lookup: HashMap<&str, &TestCoverage> = coverage
        .tests
        .iter()
        .map(|t| (t.test_name.as_str(), t))
        .collect();

    // Build set of requirement IDs by type
    let all_req_ids: std::collections::HashSet<&str> = requirements
        .iter()
        .map(|r| r.id.as_str())
        .collect();
    let testable_req_ids: std::collections::HashSet<&str> = requirements
        .iter()
        .filter(|r| r.reqtype == "Requirement")
        .map(|r| r.id.as_str())
        .collect();

    // Stats
    let total_reqs = requirements.len();
    let testable_count = testable_req_ids.len();
    let info_count = total_reqs - testable_count;
    
    // Covered = requirement IDs that exist AND have tests
    let covered_testable = coverage
        .requirements_to_tests
        .keys()
        .filter(|id| testable_req_ids.contains(id.as_str()))
        .count();
    // Info items that also have tests
    let covered_info = coverage
        .requirements_to_tests
        .keys()
        .filter(|id| all_req_ids.contains(id.as_str()) && !testable_req_ids.contains(id.as_str()))
        .count();
    let covered_all = covered_testable + covered_info;
    
    // Coverage % only based on testable requirements
    let coverage_pct = if testable_count > 0 {
        (covered_testable as f64 / testable_count as f64) * 100.0
    } else {
        0.0
    };

    out.push_str("## Summary\n\n");
    out.push_str("| Metric | Count |\n");
    out.push_str("|--------|-------|\n");
    out.push_str(&format!("| Total Requirements | {} |\n", total_reqs));
    out.push_str(&format!("| Requirements (testable) | {} |\n", testable_count));
    out.push_str(&format!("| Information (non-testable) | {} |\n", info_count));
    out.push_str(&format!("| Covered (testable) | {} |\n", covered_testable));
    out.push_str(&format!("| Covered (info) | {} |\n", covered_info));
    out.push_str(&format!("| **Total Covered** | **{}** |\n", covered_all));
    out.push_str(&format!("| Not Yet Covered | {} |\n", testable_count.saturating_sub(covered_testable)));
    out.push_str(&format!("| **Coverage** | **{:.1}%** |\n\n", coverage_pct));

    // Group requirements by source file
    let mut by_source: HashMap<&str, Vec<&Requirement>> = HashMap::new();
    for req in requirements {
        by_source.entry(&req.source_file).or_default().push(req);
    }

    // Coverage by document (before the huge table)
    out.push_str("## Coverage by Document\n\n");
    out.push_str("| Document | Requirements | Covered | Coverage |\n");
    out.push_str("|----------|-------------|---------|----------|\n");

    for (source, reqs) in by_source.iter() {
        let testable: Vec<_> = reqs.iter().filter(|r| r.reqtype == "Requirement").collect();
        let covered_count = testable
            .iter()
            .filter(|r| coverage.requirements_to_tests.contains_key(&r.id))
            .count();
        let pct = if testable.is_empty() {
            0.0
        } else {
            (covered_count as f64 / testable.len() as f64) * 100.0
        };
        // Create anchor ID from source file name (remove .rst extension)
        let doc_anchor = source.replace(".rst", "").replace('.', "-");
        out.push_str(&format!(
            "| [{}](#doc-{}) | {} | {} | {:.0}% |\n",
            source,
            doc_anchor,
            testable.len(),
            covered_count,
            pct
        ));
    }
    out.push('\n');

    // Full requirements overview table
    out.push_str("## All Requirements\n\n");
    out.push_str("| Status | ID | Summary | Type | Tests | Details |\n");
    out.push_str("|:------:|----|---------| -----|:-----:|--------|\n");

    // Collect all requirements sorted by ID
    let mut all_reqs: Vec<&Requirement> = requirements.iter().collect();
    all_reqs.sort_by(|a, b| a.id.cmp(&b.id));

    for req in &all_reqs {
        let test_count = coverage
            .requirements_to_tests
            .get(&req.id)
            .map(|t| t.len())
            .unwrap_or(0);
        
        let is_info = req.reqtype == "Information";
        let is_covered = test_count > 0;
        
        // Green if info or covered, red otherwise
        let status = if is_info || is_covered { "✅" } else { "❌" };
        
        // Truncate summary to ~60 chars
        let summary = if req.text.len() > 60 {
            format!("{}...", &req.text.chars().take(60).collect::<String>())
        } else {
            req.text.clone()
        };
        // Escape pipes and clean up for table
        let safe_summary = summary.replace('|', "\\|").replace('\n', " ");
        
        let req_type = if is_info { "Info" } else { "Req" };
        
        let test_display = if test_count > 0 {
            format!("{}", test_count)
        } else {
            "—".to_string()
        };
        
        // Link to detailed section
        let details_link = format!("[→](#{})", req.id);
        
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            status, req.id, safe_summary, req_type, test_display, details_link
        ));
    }
    out.push('\n');

    // Covered requirements with linked tests
    out.push_str("## Covered Requirements\n\n");
    out.push_str("Each requirement below includes the full specification text and links to verifying tests.\n\n");

    for (source, reqs) in by_source.iter() {
        // Include any requirement (Requirement or Information) that has tests
        let covered_reqs: Vec<_> = reqs
            .iter()
            .filter(|r| coverage.requirements_to_tests.contains_key(&r.id))
            .collect();

        if covered_reqs.is_empty() {
            continue;
        }

        let doc_anchor = source.replace(".rst", "").replace('.', "-");
        out.push_str(&format!("<a id=\"doc-{}\"></a>\n\n", doc_anchor));
        out.push_str(&format!("### {}\n\n", source));

        // Group requirements by section
        let mut by_section: HashMap<&str, Vec<&&Requirement>> = HashMap::new();
        for req in &covered_reqs {
            by_section.entry(&req.section).or_default().push(req);
        }

        // Sort sections alphabetically
        let mut sections: Vec<_> = by_section.keys().collect();
        sections.sort();

        for section in sections {
            let section_reqs = &by_section[section];
            out.push_str(&format!("#### {}\n\n", section));

            for req in section_reqs.iter() {
                // Escape pipes for markdown compatibility
                let safe_text = req.text.replace('|', "\\|");

                let type_badge = if req.reqtype == "Information" { " *(Info)*" } else { "" };
                out.push_str(&format!("<a id=\"{}\"></a>\n\n", req.id));
                out.push_str(&format!("##### {}{}\n\n", req.id, type_badge));
                out.push_str(&format!("> {}\n\n", safe_text));
                out.push_str("**Tests:**\n\n");

                if let Some(test_names) = coverage.requirements_to_tests.get(&req.id) {
                    for test_name in test_names {
                        if let Some(test) = test_lookup.get(test_name.as_str()) {
                            // Link to GitHub repository at specific commit
                            out.push_str(&format!(
                                "- [`{}`](https://github.com/daniel-freiermuth/recentip/blob/{}/{}#L{})\n",
                                test_name, git_commit, test.file_path, test.line_number
                            ));
                        } else {
                            out.push_str(&format!("- `{}`\n", test_name));
                        }
                    }
                }
                out.push('\n');
            }
        }
    }

    // Not covered requirements
    out.push_str("## Not Yet Covered\n\n");
    out.push_str("Requirements without test coverage. Contributions welcome!\n\n");

    for (source, reqs) in by_source.iter() {
        let uncovered: Vec<_> = reqs
            .iter()
            .filter(|r| r.reqtype == "Requirement" && !coverage.requirements_to_tests.contains_key(&r.id))
            .collect();

        if uncovered.is_empty() {
            continue;
        }

        out.push_str(&format!("### {} ({} uncovered)\n\n", source, uncovered.len()));
        out.push_str("<details>\n<summary>Click to expand</summary>\n\n");

        // Group by section
        let mut by_section: HashMap<&str, Vec<&&Requirement>> = HashMap::new();
        for req in &uncovered {
            by_section.entry(&req.section).or_default().push(req);
        }

        let mut sections: Vec<_> = by_section.keys().collect();
        sections.sort();

        for section in sections {
            let section_reqs = &by_section[section];
            out.push_str(&format!("#### {}\n\n", section));

            for req in section_reqs.iter() {
                let safe_text = req.text.replace('|', "\\|");
                out.push_str(&format!("<a id=\"{}\"></a>\n\n", req.id));
                out.push_str(&format!("##### {}\n\n", req.id));
                out.push_str(&format!("> {}\n\n", safe_text));
            }
        }

        out.push_str("\n</details>\n\n");
    }

    out
}
