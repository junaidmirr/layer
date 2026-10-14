use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

static TSC_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    // Matches patterns like:
    // src/index.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.
    // or
    // src/index.ts:12:5 - error TS2322: Type 'string' is not assignable to type 'number'.
    Regex::new(r"(?m)^(?P<file>[^\s:(]+)(?:\((?P<line1>\d+),(?P<col1>\d+)\)|:(?P<line2>\d+):(?P<col2>\d+))\s*[-:]?\s*(?P<severity>error|warning)\s+(?P<code>TS\d+):\s*(?P<msg>.+)$").unwrap()
});

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct DiagnosticItem {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub severity: String,
    pub code: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationComparison {
    pub new_errors: Vec<DiagnosticItem>,
    pub resolved_errors: Vec<DiagnosticItem>,
    pub persistent_errors: Vec<DiagnosticItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationReport {
    pub success: bool,
    pub project_type: String,
    pub error_count: usize,
    pub warning_count: usize,
    pub diagnostics: Vec<DiagnosticItem>,
    pub comparison: Option<VerificationComparison>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectType {
    Rust,
    TypeScript,
    Unknown,
}

impl ProjectType {
    pub fn detect(root: &Path) -> Self {
        if root.join("Cargo.toml").exists() {
            Self::Rust
        } else if root.join("tsconfig.json").exists() || root.join("package.json").exists() {
            Self::TypeScript
        } else {
            Self::Unknown
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Unknown => "unknown",
        }
    }
}

pub struct Verifier;

impl Verifier {
    /// Parse Rust compiler JSON output from `cargo check --message-format=json`
    pub fn parse_cargo_json(stdout: &str) -> Vec<DiagnosticItem> {
        let mut items = Vec::new();

        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let parsed: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if parsed.get("reason").and_then(|r| r.as_str()) != Some("compiler-message") {
                continue;
            }

            let msg = match parsed.get("message") {
                Some(m) => m,
                None => continue,
            };

            let level = msg
                .get("level")
                .and_then(|l| l.as_str())
                .unwrap_or("error")
                .to_string();

            // We focus on errors and warnings
            if level != "error" && level != "warning" {
                continue;
            }

            let code = msg
                .get("code")
                .and_then(|c| c.get("code"))
                .and_then(|c| c.as_str())
                .map(|s| s.to_string());

            let message_text = msg
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();

            // Extract primary span
            if let Some(spans) = msg.get("spans").and_then(|s| s.as_array()) {
                let primary = spans
                    .iter()
                    .find(|s| {
                        s.get("is_primary")
                            .and_then(|p| p.as_bool())
                            .unwrap_or(false)
                    })
                    .or_else(|| spans.first());

                if let Some(span) = primary {
                    let file = span
                        .get("file_name")
                        .and_then(|f| f.as_str())
                        .unwrap_or("")
                        .to_string();
                    let line =
                        span.get("line_start").and_then(|l| l.as_u64()).unwrap_or(1) as usize;
                    let col = span
                        .get("column_start")
                        .and_then(|c| c.as_u64())
                        .unwrap_or(1) as usize;

                    items.push(DiagnosticItem {
                        file,
                        line,
                        col,
                        severity: level,
                        code,
                        message: message_text,
                    });
                }
            }
        }

        items
    }

    /// Parse TypeScript compiler output from `tsc --noEmit`
    pub fn parse_tsc_output(output: &str) -> Vec<DiagnosticItem> {
        let mut items = Vec::new();

        for cap in TSC_REGEX.captures_iter(output) {
            let file = cap
                .name("file")
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let line: usize = cap
                .name("line1")
                .or_else(|| cap.name("line2"))
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(1);
            let col: usize = cap
                .name("col1")
                .or_else(|| cap.name("col2"))
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(1);
            let severity = cap
                .name("severity")
                .map(|m| m.as_str().to_lowercase())
                .unwrap_or_else(|| "error".to_string());
            let code = cap.name("code").map(|m| m.as_str().to_string());
            let message = cap
                .name("msg")
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();

            items.push(DiagnosticItem {
                file,
                line,
                col,
                severity,
                code,
                message,
            });
        }

        items
    }

    /// Compare current diagnostics against previous diagnostics snapshot
    pub fn compare_diagnostics(
        previous: Option<&[DiagnosticItem]>,
        current: &[DiagnosticItem],
    ) -> VerificationComparison {
        let prev_items = match previous {
            Some(p) => p,
            None => {
                return VerificationComparison {
                    new_errors: current.to_vec(),
                    resolved_errors: Vec::new(),
                    persistent_errors: Vec::new(),
                };
            }
        };

        let mut persistent = Vec::new();
        let mut new_items = Vec::new();
        let mut matched_prev_indices = HashSet::new();

        for curr in current {
            let match_idx = prev_items.iter().enumerate().position(|(idx, prev)| {
                if matched_prev_indices.contains(&idx) {
                    return false;
                }
                // Match by file and code if code is present, otherwise file and message
                let file_match = prev.file == curr.file;
                let code_match = match (&prev.code, &curr.code) {
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                };
                let msg_match = prev.message == curr.message;
                let line_proximity = prev.line.abs_diff(curr.line) <= 5;

                file_match && ((code_match && line_proximity) || msg_match)
            });

            if let Some(idx) = match_idx {
                matched_prev_indices.insert(idx);
                persistent.push(curr.clone());
            } else {
                new_items.push(curr.clone());
            }
        }

        let resolved: Vec<DiagnosticItem> = prev_items
            .iter()
            .enumerate()
            .filter(|(idx, _)| !matched_prev_indices.contains(idx))
            .map(|(_, item)| item.clone())
            .collect();

        VerificationComparison {
            new_errors: new_items,
            resolved_errors: resolved,
            persistent_errors: persistent,
        }
    }

    /// Run native verification tool on the workspace root and produce a report
    pub fn verify_workspace(
        root: &Path,
        previous_diagnostics: Option<&[DiagnosticItem]>,
    ) -> VerificationReport {
        let project_type = ProjectType::detect(root);

        match project_type {
            ProjectType::Rust => Self::verify_rust(root, previous_diagnostics),
            ProjectType::TypeScript => Self::verify_typescript(root, previous_diagnostics),
            ProjectType::Unknown => VerificationReport {
                success: false,
                project_type: "unknown".to_string(),
                error_count: 0,
                warning_count: 0,
                diagnostics: Vec::new(),
                comparison: None,
                message: "No recognized build configuration (Cargo.toml or tsconfig.json/package.json) found in workspace root.".to_string(),
            },
        }
    }

    fn verify_rust(root: &Path, previous: Option<&[DiagnosticItem]>) -> VerificationReport {
        let output = match Command::new("cargo")
            .arg("check")
            .arg("--message-format=json")
            .current_dir(root)
            .output()
        {
            Ok(out) => out,
            Err(e) => {
                return VerificationReport {
                    success: false,
                    project_type: "rust".to_string(),
                    error_count: 1,
                    warning_count: 0,
                    diagnostics: Vec::new(),
                    comparison: None,
                    message: format!("Failed to invoke `cargo check`: {}", e),
                };
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut diagnostics = Self::parse_cargo_json(&stdout);

        // If cargo itself failed before emitting json (e.g. invalid Cargo.toml)
        if diagnostics.is_empty() && !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            diagnostics.push(DiagnosticItem {
                file: "Cargo.toml".to_string(),
                line: 1,
                col: 1,
                severity: "error".to_string(),
                code: None,
                message: stderr.trim().to_string(),
            });
        }

        let error_count = diagnostics.iter().filter(|d| d.severity == "error").count();
        let warning_count = diagnostics
            .iter()
            .filter(|d| d.severity == "warning")
            .count();
        let success = output.status.success() && error_count == 0;

        let comparison = Some(Self::compare_diagnostics(previous, &diagnostics));

        let message = if success {
            format!("Rust check passed cleanly ({} warnings).", warning_count)
        } else {
            format!(
                "Rust check failed with {} error(s) and {} warning(s).",
                error_count, warning_count
            )
        };

        VerificationReport {
            success,
            project_type: "rust".to_string(),
            error_count,
            warning_count,
            diagnostics,
            comparison,
            message,
        }
    }

    fn verify_typescript(root: &Path, previous: Option<&[DiagnosticItem]>) -> VerificationReport {
        // Look for local tsc in node_modules or system tsc / npx tsc
        let local_tsc = root.join("node_modules").join(".bin").join("tsc");
        let (cmd, args): (&str, Vec<&str>) = if local_tsc.exists() {
            (local_tsc.to_str().unwrap_or("tsc"), vec!["--noEmit"])
        } else {
            ("npx", vec!["--no-install", "tsc", "--noEmit"])
        };

        let output = match Command::new(cmd).args(&args).current_dir(root).output() {
            Ok(out) => out,
            Err(e) => {
                return VerificationReport {
                    success: false,
                    project_type: "typescript".to_string(),
                    error_count: 1,
                    warning_count: 0,
                    diagnostics: Vec::new(),
                    comparison: None,
                    message: format!("Failed to invoke TypeScript compiler `{}`: {}", cmd, e),
                };
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}\n{}", stdout, stderr);

        let diagnostics = Self::parse_tsc_output(&combined);
        let error_count = diagnostics.iter().filter(|d| d.severity == "error").count();
        let warning_count = diagnostics
            .iter()
            .filter(|d| d.severity == "warning")
            .count();
        let success = output.status.success() && error_count == 0;

        let comparison = Some(Self::compare_diagnostics(previous, &diagnostics));

        let message = if success {
            "TypeScript check passed cleanly with 0 errors.".to_string()
        } else {
            format!(
                "TypeScript check failed with {} error(s) and {} warning(s).",
                error_count, warning_count
            )
        };

        VerificationReport {
            success,
            project_type: "typescript".to_string(),
            error_count,
            warning_count,
            diagnostics,
            comparison,
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tsc_standard_output() {
        let sample_output = r#"
src/physics/engine.ts(24,18): error TS2322: Type 'string' is not assignable to type 'number'.
src/components/Canvas.tsx:45:9 - error TS2304: Cannot find name 'useRef'.
src/types.ts(10,3): warning TS2564: Property 'mass' has no initializer and is not definitely assigned in the constructor.
"#;
        let diags = Verifier::parse_tsc_output(sample_output);
        assert_eq!(diags.len(), 3);

        assert_eq!(diags[0].file, "src/physics/engine.ts");
        assert_eq!(diags[0].line, 24);
        assert_eq!(diags[0].col, 18);
        assert_eq!(diags[0].severity, "error");
        assert_eq!(diags[0].code.as_deref(), Some("TS2322"));
        assert!(diags[0].message.contains("not assignable to type 'number'"));

        assert_eq!(diags[1].file, "src/components/Canvas.tsx");
        assert_eq!(diags[1].line, 45);
        assert_eq!(diags[1].col, 9);
        assert_eq!(diags[1].severity, "error");
        assert_eq!(diags[1].code.as_deref(), Some("TS2304"));

        assert_eq!(diags[2].severity, "warning");
        assert_eq!(diags[2].code.as_deref(), Some("TS2564"));
    }

    #[test]
    fn test_parse_cargo_json_diagnostics() {
        let cargo_line = r#"{"reason":"compiler-message","package_id":"demo 0.1.0","message":{"rendered":"...","children":[],"code":{"code":"E0308"},"level":"error","message":"mismatched types","spans":[{"file_name":"src/main.rs","line_start":14,"line_end":14,"column_start":9,"column_end":10,"is_primary":true}]}}"#;
        let diags = Verifier::parse_cargo_json(cargo_line);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].file, "src/main.rs");
        assert_eq!(diags[0].line, 14);
        assert_eq!(diags[0].col, 9);
        assert_eq!(diags[0].severity, "error");
        assert_eq!(diags[0].code.as_deref(), Some("E0308"));
        assert_eq!(diags[0].message, "mismatched types");
    }

    #[test]
    fn test_comparison_diagnostics_all_cases() {
        let err1 = DiagnosticItem {
            file: "src/engine.ts".to_string(),
            line: 10,
            col: 5,
            severity: "error".to_string(),
            code: Some("TS2322".to_string()),
            message: "Type mismatch".to_string(),
        };

        let err2 = DiagnosticItem {
            file: "src/canvas.tsx".to_string(),
            line: 30,
            col: 8,
            severity: "error".to_string(),
            code: Some("TS2304".to_string()),
            message: "Cannot find name 'ctx'".to_string(),
        };

        let err3_new = DiagnosticItem {
            file: "src/utils.ts".to_string(),
            line: 15,
            col: 2,
            severity: "error".to_string(),
            code: Some("TS2554".to_string()),
            message: "Expected 2 arguments, but got 1".to_string(),
        };

        // 1. First run: previous is None
        let comp1 = Verifier::compare_diagnostics(None, &[err1.clone(), err2.clone()]);
        assert_eq!(comp1.new_errors.len(), 2);
        assert!(comp1.resolved_errors.is_empty());
        assert!(comp1.persistent_errors.is_empty());

        // 2. Second run: err1 resolved, err2 persisted (shifted by 1 line), err3 is newly introduced
        let mut err2_shifted = err2.clone();
        err2_shifted.line = 31; // line shifted slightly due to code insertion

        let prev = vec![err1.clone(), err2.clone()];
        let curr = vec![err2_shifted.clone(), err3_new.clone()];

        let comp2 = Verifier::compare_diagnostics(Some(&prev), &curr);

        // err3_new must be in new_errors
        assert_eq!(comp2.new_errors.len(), 1);
        assert_eq!(comp2.new_errors[0].code.as_deref(), Some("TS2554"));

        // err1 must be in resolved_errors
        assert_eq!(comp2.resolved_errors.len(), 1);
        assert_eq!(comp2.resolved_errors[0].file, "src/engine.ts");
        assert_eq!(comp2.resolved_errors[0].code.as_deref(), Some("TS2322"));

        // err2 must be in persistent_errors
        assert_eq!(comp2.persistent_errors.len(), 1);
        assert_eq!(comp2.persistent_errors[0].file, "src/canvas.tsx");
        assert_eq!(comp2.persistent_errors[0].code.as_deref(), Some("TS2304"));
    }

    #[test]
    fn test_successful_clean_verification_comparison() {
        let err = DiagnosticItem {
            file: "src/main.rs".to_string(),
            line: 5,
            col: 1,
            severity: "error".to_string(),
            code: Some("E0425".to_string()),
            message: "cannot find value".to_string(),
        };

        let prev = vec![err];
        let curr: Vec<DiagnosticItem> = Vec::new();

        let comp = Verifier::compare_diagnostics(Some(&prev), &curr);
        assert!(comp.new_errors.is_empty());
        assert!(comp.persistent_errors.is_empty());
        assert_eq!(comp.resolved_errors.len(), 1);
        assert_eq!(comp.resolved_errors[0].code.as_deref(), Some("E0425"));
    }
}
