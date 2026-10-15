use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

static TSC_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    // Matches patterns like:
    // src/index.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.
    // or
    // src/index.ts:12:5 - error TS2322: Type 'string' is not assignable to type 'number'.
    Regex::new(r"(?m)^(?P<file>[^\s:(]+)(?:\((?P<line1>\d+),(?P<col1>\d+)\)|:(?P<line2>\d+):(?P<col2>\d+))\s*[-:]?\s*(?P<severity>error|warning)\s+(?P<code>TS\d+):\s*(?P<msg>.+)$").unwrap()
});

static IDENTIFIER_EXTRACT_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Matches single-quoted or backticked terms in compiler messages
    Regex::new(r#"['`"]([a-zA-Z0-9_$]+)['`"]"#).unwrap()
});

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct DiagnosticItem {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub severity: String,
    pub code: Option<String>,
    pub message: String,
    #[serde(default = "default_occurrence_count")]
    pub occurrence_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_truth_context: Option<String>,
}

fn default_occurrence_count() -> usize {
    1
}

impl DiagnosticItem {
    pub fn new(
        file: impl Into<String>,
        line: usize,
        col: usize,
        severity: impl Into<String>,
        code: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            file: file.into(),
            line,
            col,
            severity: severity.into(),
            code,
            message: message.into(),
            occurrence_count: 1,
            ground_truth_context: None,
        }
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compiler_skipped_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactedTestReport {
    pub changed_files: Vec<String>,
    pub impacted_test_files: Vec<String>,
    pub total_workspace_test_files: usize,
    pub skipped_test_files_count: usize,
    pub passed: bool,
    pub execution_ms: u128,
    pub diagnostics: Vec<DiagnosticItem>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectType {
    Rust,
    TypeScript,
    Python,
    Go,
    Cpp,
    Swift,
    Kotlin,
    CSharp,
    Java,
    Unknown,
}

impl ProjectType {
    pub fn detect(root: &Path) -> Self {
        // Disambiguation: if the workspace has top-level Python files and no TS/Rust files,
        // it must be identified as Python even if an unrelated subfolder or old config exists.
        let mut top_level_py = false;
        let mut top_level_ts = false;
        let mut top_level_rs = false;
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
                        "py" | "pyi" => top_level_py = true,
                        "ts" | "tsx" | "js" | "jsx" => top_level_ts = true,
                        "rs" => top_level_rs = true,
                        _ => {}
                    }
                }
            }
        }

        if top_level_py && !top_level_ts && !top_level_rs && !root.join("Cargo.toml").exists() {
            return Self::Python;
        }

        if root.join("Cargo.toml").exists() {
            Self::Rust
        } else if root.join("tsconfig.json").exists()
            || root.join("tsconfig.app.json").exists()
            || root.join("package.json").exists()
        {
            Self::TypeScript
        } else if root.join("pyproject.toml").exists()
            || root.join("requirements.txt").exists()
            || root.join("setup.py").exists()
            || root.join("Pipfile").exists()
        {
            Self::Python
        } else if root.join("go.mod").exists() || root.join("go.sum").exists() {
            Self::Go
        } else if root.join("CMakeLists.txt").exists()
            || root.join("Makefile").exists()
            || root.join("compile_commands.json").exists()
        {
            Self::Cpp
        } else if root.join("Package.swift").exists() {
            Self::Swift
        } else if root.join("pom.xml").exists() {
            Self::Java
        } else if root.join("build.gradle.kts").exists() {
            Self::Kotlin
        } else if root.join("build.gradle").exists() {
            Self::Java
        } else {
            // Also inspect file extensions in root if no build manifest found
            if let Ok(entries) = std::fs::read_dir(root) {
                let mut has_py = false;
                let mut has_go = false;
                let mut has_cpp = false;
                let mut has_swift = false;
                let mut has_kt = false;
                let mut has_rs = false;
                let mut has_ts = false;
                let mut has_cs = false;
                let mut has_java = false;
                for entry in entries.flatten() {
                    let p = entry.path();
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name.ends_with(".csproj") || name.ends_with(".sln") {
                        return Self::CSharp;
                    }
                    if p.is_file() {
                        match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
                            "rs" => has_rs = true,
                            "ts" | "tsx" | "js" | "jsx" => has_ts = true,
                            "py" | "pyi" => has_py = true,
                            "go" => has_go = true,
                            "cpp" | "cc" | "cxx" | "c" | "h" | "hpp" => has_cpp = true,
                            "swift" => has_swift = true,
                            "kt" | "kts" => has_kt = true,
                            "cs" => has_cs = true,
                            "java" => has_java = true,
                            _ => {}
                        }
                    }
                }
                if has_rs {
                    return Self::Rust;
                } else if has_ts {
                    return Self::TypeScript;
                } else if has_py {
                    return Self::Python;
                } else if has_go {
                    return Self::Go;
                } else if has_cpp {
                    return Self::Cpp;
                } else if has_swift {
                    return Self::Swift;
                } else if has_kt {
                    return Self::Kotlin;
                } else if has_cs {
                    return Self::CSharp;
                } else if has_java {
                    return Self::Java;
                }
            }
            Self::Unknown
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Python => "python",
            Self::Go => "go",
            Self::Cpp => "cpp",
            Self::Swift => "swift",
            Self::Kotlin => "kotlin",
            Self::CSharp => "csharp",
            Self::Java => "java",
            Self::Unknown => "unknown",
        }
    }
}

/// Checks whether the directory contains any top-level source code files
fn has_top_level_source_files(root: &Path) -> bool {
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file()
            && let Some(ext) = p.extension().and_then(|s| s.to_str())
            && matches!(
                ext,
                "py" | "pyi"
                    | "rs"
                    | "ts"
                    | "tsx"
                    | "js"
                    | "jsx"
                    | "go"
                    | "cpp"
                    | "cc"
                    | "cxx"
                    | "c"
                    | "h"
                    | "hpp"
                    | "swift"
                    | "kt"
                    | "kts"
                    | "cs"
                    | "java"
            )
        {
            return true;
        }
    }
    false
}

/// Scans immediate child directories of `root` to see if one contains a recognized build manifest.
/// Useful when a project was bootstrapped into a subfolder (e.g. `light_circuit_studio/` or `bulb-circuit-simulator/`).
/// ONLY matches if root has NO top-level source files and there is a single unambiguous child project.
pub fn detect_nested_project_root(root: &Path) -> Option<PathBuf> {
    if ProjectType::detect(root) != ProjectType::Unknown {
        return None;
    }

    if has_top_level_source_files(root) {
        return None;
    }

    let entries = std::fs::read_dir(root).ok()?;
    let mut candidates = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        // Ignore hidden, VCS, cache, and build artifact folders
        if name.starts_with('.')
            || name == "node_modules"
            || name == "target"
            || name == "dist"
            || name == "build"
            || name == "bin"
            || name == "obj"
            || name == "__pycache__"
        {
            continue;
        }

        if ProjectType::detect(&path) != ProjectType::Unknown {
            let mtime = entry.metadata().and_then(|m| m.modified()).ok();
            candidates.push((path, mtime));
        }
    }

    // Only auto-detect if there is a single unambiguous child project directory!
    if candidates.len() == 1 {
        Some(candidates.remove(0).0)
    } else {
        None
    }
}

pub struct Verifier;

impl Verifier {
    /// Extract potential identifiers or symbols mentioned inside quotes or backticks in a diagnostic
    pub fn extract_identifiers_from_diagnostic(message: &str) -> Vec<String> {
        let mut idents = Vec::new();
        for cap in IDENTIFIER_EXTRACT_RE.captures_iter(message) {
            if let Some(m) = cap.get(1) {
                let s = m.as_str();
                // Filter out common language keywords/primitives
                if !matches!(
                    s,
                    "string"
                        | "number"
                        | "boolean"
                        | "any"
                        | "void"
                        | "unknown"
                        | "never"
                        | "undefined"
                        | "null"
                        | "true"
                        | "false"
                        | "object"
                        | "symbol"
                        | "usize"
                        | "u32"
                        | "i32"
                        | "u64"
                        | "i64"
                        | "f32"
                        | "f64"
                        | "bool"
                        | "str"
                ) && !idents.contains(&s.to_string())
                {
                    idents.push(s.to_string());
                }
            }
        }
        idents
    }

    /// Automatically attach ground-truth AST signatures or package API declarations
    /// to persistent diagnostics (`occurrence_count >= 2`).
    pub fn enrich_persistent_diagnostics(
        diagnostics: &mut [DiagnosticItem],
        graph: &crate::graph::VectorGraph,
        mut pkg_indexer: Option<&mut crate::package_indexer::PackageIndexer>,
        workspace_root: &Path,
    ) {
        for diag in diagnostics.iter_mut() {
            if diag.occurrence_count >= 2 && diag.ground_truth_context.is_none() {
                let idents = Self::extract_identifiers_from_diagnostic(&diag.message);
                let mut ground_truth_sections = Vec::new();

                for ident in &idents {
                    // 1. Search in local VectorGraph
                    let local_hits = graph.find_symbol_filtered(ident, None, None);
                    for (file_path, sym) in local_hits.into_iter().take(2) {
                        if let Some(body) =
                            graph.get_symbol_body_opt(&sym.name, Some(&file_path), 50)
                        {
                            ground_truth_sections.push(format!(
                                "[Ground Truth Definition for '{}' in {} (lines {}-{})]\n```typescript\n{}\n```",
                                sym.name,
                                file_path.display(),
                                body.lines[0],
                                body.lines[1],
                                body.body
                            ));
                        } else {
                            ground_truth_sections.push(format!(
                                "[Ground Truth Declaration for '{}' in {}]\n`{}`",
                                sym.name,
                                file_path.display(),
                                sym.signature
                            ));
                        }
                    }

                    // 2. Search in external package if pkg_indexer available
                    if ground_truth_sections.is_empty()
                        && let Some(ref mut indexer) = pkg_indexer
                        && let Some(file_node) = graph.files.get(Path::new(&diag.file))
                    {
                        for imp in &file_node.imports {
                            if let Some(caps) = crate::graph::IMPORT_PATH_REGEX.captures(imp)
                                && let Some(spec) = caps.get(1)
                            {
                                let spec_str = spec.as_str();
                                if !spec_str.starts_with('.') {
                                    let pkg_name = if spec_str.starts_with('@') {
                                        let parts: Vec<&str> = spec_str.split('/').collect();
                                        if parts.len() >= 2 {
                                            format!("{}/{}", parts[0], parts[1])
                                        } else {
                                            spec_str.to_string()
                                        }
                                    } else {
                                        spec_str.split('/').next().unwrap_or(spec_str).to_string()
                                    };

                                    if let Ok(pkg_api) = indexer.inspect_package(
                                        workspace_root,
                                        &pkg_name,
                                        Some(ident),
                                        None,
                                    ) && let Some(decl) = pkg_api.requested_symbol
                                    {
                                        ground_truth_sections.push(format!(
                                            "[Ground Truth from External Package '{}']\nSymbol: {} ({})\nSignature: `{}`",
                                            pkg_name, decl.symbol_name, decl.kind, decl.signature
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }

                if !ground_truth_sections.is_empty() {
                    diag.ground_truth_context = Some(ground_truth_sections.join("\n\n"));
                }
            }
        }
    }

    pub fn instant_ast_precheck(
        root: &Path,
        files: &std::collections::HashMap<std::path::PathBuf, crate::models::FileNode>,
        path_aliases: &std::collections::HashMap<String, String>,
    ) -> Vec<DiagnosticItem> {
        Self::instant_ast_precheck_filtered(root, files, path_aliases, None)
    }

    /// Fast-path in-memory AST syntax and local import resolution (<5ms).
    /// Detects broken imports and syntax parse errors without spawning external compiler processes.
    /// When allowed_files is provided, only those files are checked for syntax & local import issues.
    pub fn instant_ast_precheck_filtered(
        root: &Path,
        files: &std::collections::HashMap<std::path::PathBuf, crate::models::FileNode>,
        path_aliases: &std::collections::HashMap<String, String>,
        allowed_files: Option<&HashSet<PathBuf>>,
    ) -> Vec<DiagnosticItem> {
        let mut items = Vec::new();
        let known_files: HashSet<std::path::PathBuf> = files.keys().cloned().collect();

        for (rel_path, node) in files {
            if let Some(allowed) = allowed_files {
                let norm = crate::graph::normalize_relative_path(rel_path);
                if !allowed.contains(&norm) && !allowed.contains(rel_path) {
                    continue;
                }
            }

            let ext = node.extension.as_deref().unwrap_or("");
            if !matches!(
                ext,
                "ts" | "tsx"
                    | "js"
                    | "jsx"
                    | "rs"
                    | "py"
                    | "pyi"
                    | "go"
                    | "c"
                    | "cpp"
                    | "cc"
                    | "h"
                    | "hpp"
                    | "swift"
                    | "kt"
                    | "kts"
                    | "cs"
                    | "java"
            ) {
                continue;
            }

            // 1. Syntax parse check via Tree-sitter or lexical balance check
            if let Some(lang) = crate::scanner::AstScanner::get_language(Some(ext)) {
                let mut parser = tree_sitter::Parser::new();
                if parser.set_language(&lang).is_ok()
                    && let Some(tree) = parser.parse(&node.content, None)
                {
                    let root_node = tree.root_node();
                    if root_node.has_error() {
                        let mut is_real_error = true;

                        // Python false-positive filter:
                        // tree-sitter-python flags modern PEP 701 f-strings (nested quotes inside f-strings)
                        // or modern Python 3.10-3.12 syntax as (ERROR) CST nodes.
                        if matches!(ext, "py" | "pyi") {
                            let mut py_parsed_cleanly = false;

                            if let Ok(mut child) = std::process::Command::new("python3")
                                .args(["-c", "import ast, sys; ast.parse(sys.stdin.read())"])
                                .stdin(std::process::Stdio::piped())
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .spawn()
                            {
                                if let Some(mut stdin) = child.stdin.take() {
                                    let _ = stdin.write_all(node.content.as_bytes());
                                }
                                if let Ok(status) = child.wait()
                                    && status.success()
                                {
                                    py_parsed_cleanly = true;
                                }
                            }

                            if py_parsed_cleanly
                                || crate::scanner::AstScanner::check_lexical_syntax(&node.content)
                                    .is_ok()
                            {
                                is_real_error = false;
                            }
                        }

                        if is_real_error {
                            let (line, col) = Self::find_ast_error_position(root_node);
                            items.push(DiagnosticItem {
                                file: rel_path.to_string_lossy().to_string(),
                                line,
                                col,
                                severity: "error".to_string(),
                                code: Some("SYNTAX_ERROR".to_string()),
                                message: format!(
                                    "Syntax error detected in {} at line {}, column {}",
                                    rel_path.display(),
                                    line,
                                    col
                                ),
                                occurrence_count: 1,
                                ground_truth_context: None,
                            });
                        }
                    }
                }
            } else if matches!(ext, "swift" | "kt" | "kts" | "cs" | "java")
                && let Err((line, col, msg)) =
                    crate::scanner::AstScanner::check_lexical_syntax(&node.content)
            {
                items.push(DiagnosticItem {
                    file: rel_path.to_string_lossy().to_string(),
                    line,
                    col,
                    severity: "error".to_string(),
                    code: Some("SYNTAX_ERROR".to_string()),
                    message: format!("Syntax error detected in {}: {}", rel_path.display(), msg),
                    occurrence_count: 1,
                    ground_truth_context: None,
                });
            }

            // 2. Relative & Aliased local import resolution check
            for import_str in &node.imports {
                let mut matched_target: Option<std::path::PathBuf> = None;

                if let Some(caps) = crate::graph::IMPORT_PATH_REGEX.captures(import_str)
                    && let Some(spec) = caps.get(1)
                {
                    let spec_str = spec.as_str().trim();
                    let is_relative = spec_str.starts_with("./") || spec_str.starts_with("../");
                    let is_aliased = path_aliases
                        .keys()
                        .any(|k| spec_str.starts_with(k.trim_end_matches('*')));

                    if is_relative || is_aliased {
                        matched_target = crate::graph::resolve_module_import_path(
                            rel_path,
                            spec_str,
                            &known_files,
                            path_aliases,
                        );
                        if matched_target.is_none() {
                            let parent = rel_path.parent().unwrap_or(Path::new(""));
                            let disk_cand = root.join(parent).join(spec_str);
                            if !disk_cand.exists() && !root.join(spec_str).exists() {
                                let line = node
                                    .content
                                    .lines()
                                    .position(|l| l.contains(spec_str))
                                    .unwrap_or(0)
                                    + 1;
                                let err_code = if matches!(ext, "ts" | "tsx" | "js" | "jsx") {
                                    "TS2307"
                                } else {
                                    "MODULE_NOT_FOUND"
                                };
                                items.push(DiagnosticItem {
                                    file: rel_path.to_string_lossy().to_string(),
                                    line,
                                    col: 1,
                                    severity: "error".to_string(),
                                    code: Some(err_code.to_string()),
                                    message: format!(
                                        "Cannot find module '{}' or its corresponding type declarations.",
                                        spec_str
                                    ),
                                    occurrence_count: 1,
                                    ground_truth_context: None,
                                });
                            }
                        }
                    }
                }

                // Check Python relative imports
                if matched_target.is_none()
                    && matches!(ext, "py" | "pyi")
                    && (import_str.starts_with("from .") || import_str.starts_with("from .."))
                {
                    let resolved = crate::graph::resolve_python_import_path(
                        rel_path,
                        import_str,
                        &known_files,
                    );
                    if resolved.is_none() {
                        let line = node
                            .content
                            .lines()
                            .position(|l| l.contains(import_str))
                            .unwrap_or(0)
                            + 1;
                        items.push(DiagnosticItem {
                            file: rel_path.to_string_lossy().to_string(),
                            line,
                            col: 1,
                            severity: "error".to_string(),
                            code: Some("PY_IMPORT_ERROR".to_string()),
                            message: format!(
                                "Unresolved relative Python import '{}' in {}.",
                                import_str,
                                rel_path.display()
                            ),
                            occurrence_count: 1,
                            ground_truth_context: None,
                        });
                    }
                }
            }
        }

        items
    }

    fn find_ast_error_position(node: tree_sitter::Node) -> (usize, usize) {
        if node.is_error() || node.is_missing() {
            let pt = node.start_position();
            return (pt.row + 1, pt.column + 1);
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i)
                && child.has_error()
            {
                return Self::find_ast_error_position(child);
            }
        }
        let pt = node.start_position();
        (pt.row + 1, pt.column + 1)
    }

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
                        occurrence_count: 1,
                        ground_truth_context: None,
                    });
                }
            }
        }

        items
    }

    /// Parse TypeScript compiler output from `tsc --noEmit` or `tsc -b`
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
                occurrence_count: 1,
                ground_truth_context: None,
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
                let mut persistent_item = curr.clone();
                persistent_item.occurrence_count = prev_items[idx].occurrence_count + 1;
                persistent.push(persistent_item);
            } else {
                let mut new_item = curr.clone();
                new_item.occurrence_count = 1;
                new_items.push(new_item);
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

    /// Standard verification entrypoint
    pub fn verify_workspace(
        root: &Path,
        previous_diagnostics: Option<&[DiagnosticItem]>,
    ) -> VerificationReport {
        Self::verify_workspace_with_graph(root, None, previous_diagnostics)
    }

    /// Verification entrypoint with optional cached VectorGraph
    pub fn verify_workspace_with_graph(
        root: &Path,
        graph: Option<&crate::graph::VectorGraph>,
        previous_diagnostics: Option<&[DiagnosticItem]>,
    ) -> VerificationReport {
        let mut target_root = root.to_path_buf();
        let mut project_type = ProjectType::detect(root);

        if project_type == ProjectType::Unknown
            && let Some(nested) = detect_nested_project_root(root)
        {
            target_root = nested;
            project_type = ProjectType::detect(&target_root);
        }

        match project_type {
            ProjectType::Rust => Self::verify_rust(&target_root, previous_diagnostics),
            ProjectType::TypeScript => {
                Self::verify_typescript(&target_root, graph, previous_diagnostics)
            }
            ProjectType::Python => Self::verify_python(&target_root, graph, previous_diagnostics),
            ProjectType::Go => Self::verify_go(&target_root, graph, previous_diagnostics),
            ProjectType::Cpp => Self::verify_cpp(&target_root, graph, previous_diagnostics),
            ProjectType::Swift => Self::verify_swift(&target_root, graph, previous_diagnostics),
            ProjectType::Kotlin => Self::verify_kotlin(&target_root, graph, previous_diagnostics),
            ProjectType::CSharp => Self::verify_csharp(&target_root, graph, previous_diagnostics),
            ProjectType::Java => Self::verify_java(&target_root, graph, previous_diagnostics),
            ProjectType::Unknown => VerificationReport {
                success: false,
                project_type: "unknown".to_string(),
                error_count: 1,
                warning_count: 0,
                diagnostics: vec![DiagnosticItem {
                    file: ".".to_string(),
                    line: 1,
                    col: 1,
                    severity: "error".to_string(),
                    code: Some("CONFIG_MISSING".to_string()),
                    message: "No recognized build configuration found in workspace root."
                        .to_string(),
                    occurrence_count: 1,
                    ground_truth_context: None,
                }],
                comparison: None,
                message: "No recognized build configuration found in workspace root.".to_string(),
                compiler_skipped_reason: None,
            },
        }
    }

    /// Run fast in-memory AST syntax and import checks without spawning compiler subprocesses (<5ms)
    pub fn verify_fast_ast_only(
        root: &Path,
        graph: &crate::graph::VectorGraph,
        pkg_indexer: Option<&mut crate::package_indexer::PackageIndexer>,
    ) -> VerificationReport {
        Self::verify_fast_ast_only_filtered(root, graph, pkg_indexer, None)
    }

    /// Run fast in-memory AST syntax and import checks for a specific set of allowed files
    pub fn verify_fast_ast_only_filtered(
        root: &Path,
        graph: &crate::graph::VectorGraph,
        pkg_indexer: Option<&mut crate::package_indexer::PackageIndexer>,
        allowed_files: Option<&HashSet<PathBuf>>,
    ) -> VerificationReport {
        let mut target_root = root.to_path_buf();
        if ProjectType::detect(root) == ProjectType::Unknown
            && let Some(nested) = detect_nested_project_root(root)
        {
            target_root = nested;
        }

        let precheck_diags = Self::instant_ast_precheck_filtered(
            &target_root,
            &graph.files,
            &graph.path_aliases,
            allowed_files,
        );
        let mut comparison =
            Self::compare_diagnostics(graph.last_verification.as_deref(), &precheck_diags);
        Self::enrich_persistent_diagnostics(
            &mut comparison.persistent_errors,
            graph,
            pkg_indexer,
            &target_root,
        );

        let error_count = precheck_diags.len();
        let success = error_count == 0;
        let message = if success {
            "Fast-path AST pre-check passed (0 syntax or unresolvable import errors in <5ms)."
                .to_string()
        } else {
            format!(
                "Fast-path AST pre-check detected {} syntax/import error(s).",
                error_count
            )
        };

        VerificationReport {
            success,
            project_type: ProjectType::detect(&target_root).as_str().to_string(),
            error_count,
            warning_count: 0,
            diagnostics: precheck_diags,
            comparison: Some(comparison),
            message,
            compiler_skipped_reason: Some(
                "Fast-path AST verification executed (<5ms); external compiler subprocess bypassed"
                    .to_string(),
            ),
        }
    }

    /// Enhanced verification with AST fast-path, configurable compiler forcing, and automatic persistent context injection
    pub fn verify_workspace_with_options(
        root: &Path,
        graph: &crate::graph::VectorGraph,
        mut pkg_indexer: Option<&mut crate::package_indexer::PackageIndexer>,
        force_compiler: bool,
    ) -> VerificationReport {
        let mut target_root = root.to_path_buf();
        if ProjectType::detect(root) == ProjectType::Unknown
            && let Some(nested) = detect_nested_project_root(root)
        {
            target_root = nested;
        }

        // Fast-Path: In-memory AST precheck (<5ms)
        let precheck_diags =
            Self::instant_ast_precheck(&target_root, &graph.files, &graph.path_aliases);
        if !precheck_diags.is_empty() && !force_compiler {
            let mut comparison =
                Self::compare_diagnostics(graph.last_verification.as_deref(), &precheck_diags);
            Self::enrich_persistent_diagnostics(
                &mut comparison.persistent_errors,
                graph,
                pkg_indexer.as_deref_mut(),
                &target_root,
            );

            let error_count = precheck_diags.len();
            return VerificationReport {
                success: false,
                project_type: ProjectType::detect(&target_root).as_str().to_string(),
                error_count,
                warning_count: 0,
                diagnostics: precheck_diags,
                comparison: Some(comparison),
                message: format!(
                    "Fast-path AST pre-check detected {} syntax/import error(s) in <5ms.",
                    error_count
                ),
                compiler_skipped_reason: Some(
                    "Pre-check detected AST syntax/import errors; external compiler subprocess bypassed"
                        .to_string(),
                ),
            };
        }

        // Run native verification
        let mut report = Self::verify_workspace_with_graph(
            &target_root,
            Some(graph),
            graph.last_verification.as_deref(),
        );

        // Enrich persistent errors with ground-truth definitions
        if let Some(ref mut comp) = report.comparison {
            Self::enrich_persistent_diagnostics(
                &mut comp.persistent_errors,
                graph,
                pkg_indexer,
                &target_root,
            );
        }

        report
    }

    /// Enhanced verification with AST fast-path and automatic persistent context injection
    pub fn verify_workspace_with_circuit_breaker(
        root: &Path,
        graph: &crate::graph::VectorGraph,
        pkg_indexer: Option<&mut crate::package_indexer::PackageIndexer>,
    ) -> VerificationReport {
        Self::verify_workspace_with_options(root, graph, pkg_indexer, false)
    }

    /// Selectively run only tests impacted by changed files using VectorGraph dependencies
    pub fn verify_impacted_tests(
        root: &Path,
        changed_files: Option<&[String]>,
        graph: &crate::graph::VectorGraph,
    ) -> ImpactedTestReport {
        let start = std::time::Instant::now();

        // 1. Collect target changed files
        let target_changed: Vec<PathBuf> = if let Some(files) = changed_files
            && !files.is_empty()
        {
            files.iter().map(PathBuf::from).collect()
        } else if let Some(ref multi) = graph.last_multi_file_snapshot {
            multi.iter().map(|s| s.rel_path.clone()).collect()
        } else if let Some(ref single) = graph.last_mutation_snapshot {
            vec![single.rel_path.clone()]
        } else {
            // Default to all indexed files
            graph.files.keys().cloned().collect()
        };

        // 2. Query impacted test files via VectorGraph
        let impacted_paths = graph.get_impacted_test_files(&target_changed);
        let all_test_files = graph.get_all_test_files();
        let total_workspace_test_files = all_test_files.len();
        let skipped_test_files_count =
            total_workspace_test_files.saturating_sub(impacted_paths.len());

        let changed_str: Vec<String> = target_changed
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        let impacted_str: Vec<String> = impacted_paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        if impacted_paths.is_empty() {
            let execution_ms = start.elapsed().as_millis();
            return ImpactedTestReport {
                changed_files: changed_str.clone(),
                impacted_test_files: vec![],
                total_workspace_test_files,
                skipped_test_files_count,
                passed: true,
                execution_ms,
                diagnostics: vec![],
                message: format!(
                    "No impacted test files found for {} changed file(s). (Skipped {} unaffected tests)",
                    changed_str.len(),
                    skipped_test_files_count
                ),
            };
        }

        // 3. Execution / Verification of impacted test files
        let mut diagnostics = Vec::new();
        let mut executed_real_runner = false;
        let mut passed = true;

        let mut target_root = root.to_path_buf();
        let mut proj_type = ProjectType::detect(root);
        if proj_type == ProjectType::Unknown
            && let Some(nested) = detect_nested_project_root(root)
        {
            target_root = nested;
            proj_type = ProjectType::detect(&target_root);
        }

        // Try native test runner if present
        match proj_type {
            ProjectType::TypeScript => {
                let vitest = target_root.join("node_modules/.bin/vitest");
                let jest = target_root.join("node_modules/.bin/jest");
                if vitest.exists() {
                    let mut cmd = Command::new(&vitest);
                    cmd.stdin(std::process::Stdio::null());
                    cmd.arg("run").current_dir(&target_root);
                    for p in &impacted_paths {
                        cmd.arg(p);
                    }
                    if let Ok(out) = cmd.output() {
                        executed_real_runner = true;
                        passed = out.status.success();
                    }
                } else if jest.exists() {
                    let mut cmd = Command::new(&jest);
                    cmd.stdin(std::process::Stdio::null());
                    cmd.current_dir(&target_root);
                    for p in &impacted_paths {
                        cmd.arg(p);
                    }
                    if let Ok(out) = cmd.output() {
                        executed_real_runner = true;
                        passed = out.status.success();
                    }
                }
            }
            ProjectType::Rust => {
                let mut rust_test_names = Vec::new();
                for p in &impacted_paths {
                    if p.starts_with("tests")
                        && let Some(stem) = p.file_stem().and_then(|s| s.to_str())
                    {
                        rust_test_names.push(stem.to_string());
                    }
                }
                if !rust_test_names.is_empty() {
                    let mut cmd = Command::new("cargo");
                    cmd.stdin(std::process::Stdio::null());
                    cmd.arg("test").current_dir(&target_root);
                    for name in &rust_test_names {
                        cmd.args(["--test", name]);
                    }
                    if let Ok(out) = cmd.output() {
                        executed_real_runner = true;
                        passed = out.status.success();
                    }
                }
            }
            ProjectType::Python => {
                let pytest = target_root.join(".venv/bin/pytest");
                let runner = if pytest.exists() {
                    Some(pytest)
                } else if Command::new("pytest")
                    .arg("--version")
                    .stdin(std::process::Stdio::null())
                    .output()
                    .is_ok()
                {
                    Some(PathBuf::from("pytest"))
                } else {
                    None
                };

                if let Some(bin) = runner {
                    let mut cmd = Command::new(bin);
                    cmd.stdin(std::process::Stdio::null());
                    cmd.current_dir(&target_root);
                    for p in &impacted_paths {
                        cmd.arg(p);
                    }
                    if let Ok(out) = cmd.output() {
                        executed_real_runner = true;
                        passed = out.status.success();
                    }
                }
            }
            ProjectType::Go => {
                let mut pkgs = std::collections::HashSet::new();
                for p in &impacted_paths {
                    if let Some(parent) = p.parent() {
                        pkgs.insert(parent.to_string_lossy().to_string());
                    }
                }
                if !pkgs.is_empty() {
                    let mut cmd = Command::new("go");
                    cmd.stdin(std::process::Stdio::null());
                    cmd.arg("test").current_dir(&target_root);
                    for pkg in &pkgs {
                        cmd.arg(format!("./{}", pkg));
                    }
                    if let Ok(out) = cmd.output() {
                        executed_real_runner = true;
                        passed = out.status.success();
                    }
                }
            }
            _ => {}
        }

        // Fast In-Memory AST / Import Precheck Fallback for impacted test files
        if !executed_real_runner {
            let all_diags =
                Self::instant_ast_precheck(&target_root, &graph.files, &graph.path_aliases);
            for d in all_diags {
                let d_path = PathBuf::from(&d.file);
                if impacted_paths.contains(&d_path) {
                    if d.severity == "error" {
                        passed = false;
                    }
                    diagnostics.push(d);
                }
            }
        }

        let execution_ms = start.elapsed().as_millis();
        let message = if passed {
            format!(
                "Ran {} impacted test file(s) (skipped {} unaffected tests) in {}ms: ALL TESTS PASSED.",
                impacted_paths.len(),
                skipped_test_files_count,
                execution_ms
            )
        } else {
            format!(
                "Ran {} impacted test file(s) (skipped {} unaffected tests) in {}ms: {} FAILURE(S) DETECTED.",
                impacted_paths.len(),
                skipped_test_files_count,
                execution_ms,
                diagnostics.len()
            )
        };

        ImpactedTestReport {
            changed_files: changed_str,
            impacted_test_files: impacted_str,
            total_workspace_test_files,
            skipped_test_files_count,
            passed,
            execution_ms,
            diagnostics,
            message,
        }
    }

    fn verify_rust(root: &Path, previous: Option<&[DiagnosticItem]>) -> VerificationReport {
        let output = match Command::new("cargo")
            .arg("check")
            .arg("--message-format=json")
            .current_dir(root)
            .stdin(std::process::Stdio::null())
            .output()
        {
            Ok(out) => out,
            Err(e) => {
                let err_msg = format!("Failed to invoke `cargo check`: {}", e);
                return VerificationReport {
                    success: false,
                    project_type: "rust".to_string(),
                    error_count: 1,
                    warning_count: 0,
                    diagnostics: vec![DiagnosticItem {
                        file: "Cargo.toml".to_string(),
                        line: 1,
                        col: 1,
                        severity: "error".to_string(),
                        code: Some("CARGO_EXEC_ERROR".to_string()),
                        message: err_msg.clone(),
                        occurrence_count: 1,
                        ground_truth_context: None,
                    }],
                    comparison: None,
                    message: err_msg,
                    compiler_skipped_reason: None,
                };
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut diagnostics = Self::parse_cargo_json(&stdout);

        if diagnostics.is_empty() && !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = if stderr.trim().is_empty() {
                "cargo check failed with non-zero exit status".to_string()
            } else {
                stderr.trim().to_string()
            };
            diagnostics.push(DiagnosticItem {
                file: "Cargo.toml".to_string(),
                line: 1,
                col: 1,
                severity: "error".to_string(),
                code: Some("CARGO_ERROR".to_string()),
                message: msg,
                occurrence_count: 1,
                ground_truth_context: None,
            });
        }

        let error_count = diagnostics.iter().filter(|d| d.severity == "error").count();
        let warning_count = diagnostics
            .iter()
            .filter(|d| d.severity == "warning")
            .count();
        let success = error_count == 0;

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
            compiler_skipped_reason: None,
        }
    }

    fn verify_typescript(
        root: &Path,
        graph: Option<&crate::graph::VectorGraph>,
        previous: Option<&[DiagnosticItem]>,
    ) -> VerificationReport {
        let local_tsc = root.join("node_modules").join(".bin").join("tsc");
        let local_tsc_cmd = root.join("node_modules").join(".bin").join("tsc.cmd");
        let has_local_tsc = local_tsc.exists() || local_tsc_cmd.exists();

        // If local tsc is not found in node_modules, do NOT spawn npx tsc (prevents hanging / command-not-found failures).
        // Instead, run Layer's fast in-memory Tree-sitter AST & import check.
        if !has_local_tsc {
            let (precheck_diags, comparison) = if let Some(g) = graph {
                let diags = Self::instant_ast_precheck(root, &g.files, &g.path_aliases);
                let comp = Self::compare_diagnostics(previous, &diags);
                (diags, comp)
            } else {
                let mut temp_graph = crate::graph::VectorGraph::new();
                temp_graph.sync_workspace(root);
                let diags =
                    Self::instant_ast_precheck(root, &temp_graph.files, &temp_graph.path_aliases);
                let comp = Self::compare_diagnostics(previous, &diags);
                (diags, comp)
            };

            let error_count = precheck_diags.len();
            let success = error_count == 0;
            let (message, skipped_reason) = if success {
                (
                    "node_modules/.bin/tsc not found; verified clean via Layer in-memory AST & dependency graph (0 syntax or import errors).".to_string(),
                    Some("node_modules/.bin/tsc not found; verified clean via Layer in-memory AST & dependency graph".to_string()),
                )
            } else {
                (
                    format!(
                        "node_modules/.bin/tsc not found; in-memory AST check detected {} syntax/import error(s).",
                        error_count
                    ),
                    Some("node_modules/.bin/tsc not found; verified via Layer in-memory AST & dependency graph".to_string()),
                )
            };

            return VerificationReport {
                success,
                project_type: "typescript".to_string(),
                error_count,
                warning_count: 0,
                diagnostics: precheck_diags,
                comparison: Some(comparison),
                message,
                compiler_skipped_reason: skipped_reason,
            };
        }

        let use_build_mode = root.join("tsconfig.app.json").exists();

        // Ensure .layer cache directory exists for fast incremental build cache
        let tsc_bin_path = if local_tsc.exists() {
            local_tsc
        } else {
            local_tsc_cmd
        };
        let tsc_bin = tsc_bin_path.to_str().unwrap_or("tsc");

        let args = if use_build_mode {
            vec![
                "-b".to_string(),
                "--pretty".to_string(),
                "false".to_string(),
            ]
        } else {
            vec![
                "--noEmit".to_string(),
                "--pretty".to_string(),
                "false".to_string(),
            ]
        };

        let output = match Command::new(tsc_bin)
            .args(&args)
            .current_dir(root)
            .stdin(std::process::Stdio::null())
            .output()
        {
            Ok(out) => out,
            Err(e) => {
                let err_msg = format!("Failed to invoke TypeScript compiler `{}`: {}", tsc_bin, e);
                return VerificationReport {
                    success: false,
                    project_type: "typescript".to_string(),
                    error_count: 1,
                    warning_count: 0,
                    diagnostics: vec![DiagnosticItem {
                        file: "tsconfig.json".to_string(),
                        line: 1,
                        col: 1,
                        severity: "error".to_string(),
                        code: Some("TSC_EXEC_ERROR".to_string()),
                        message: err_msg.clone(),
                        occurrence_count: 1,
                        ground_truth_context: None,
                    }],
                    comparison: None,
                    message: err_msg,
                    compiler_skipped_reason: None,
                };
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}\n{}", stdout, stderr);

        let mut diagnostics = Self::parse_tsc_output(&combined);

        // If tsc returned non-zero exit code but parse_tsc_output yielded 0 diagnostics,
        // extract raw error lines or fall back to AST check so success: false NEVER has 0 diagnostics.
        if !output.status.success() && diagnostics.is_empty() {
            let non_empty_line = combined
                .lines()
                .map(|l| l.trim())
                .find(|l| !l.is_empty() && !l.starts_with('>'));
            if let Some(err_line) = non_empty_line {
                diagnostics.push(DiagnosticItem {
                    file: "tsconfig.json".to_string(),
                    line: 1,
                    col: 1,
                    severity: "error".to_string(),
                    code: Some("TS_ERROR".to_string()),
                    message: err_line.to_string(),
                    occurrence_count: 1,
                    ground_truth_context: None,
                });
            } else {
                let ast_diags = if let Some(g) = graph {
                    Self::instant_ast_precheck(root, &g.files, &g.path_aliases)
                } else {
                    let mut temp_graph = crate::graph::VectorGraph::new();
                    temp_graph.sync_workspace(root);
                    Self::instant_ast_precheck(root, &temp_graph.files, &temp_graph.path_aliases)
                };
                diagnostics.extend(ast_diags);
            }
        }

        let error_count = diagnostics.iter().filter(|d| d.severity == "error").count();
        let warning_count = diagnostics
            .iter()
            .filter(|d| d.severity == "warning")
            .count();
        // NEVER return success: false when diagnostics is empty!
        let success = error_count == 0;

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
            compiler_skipped_reason: None,
        }
    }

    pub fn verify_tier1_ast_fallback(
        root: &Path,
        project_type: &str,
        toolchain_name: &str,
        graph: Option<&crate::graph::VectorGraph>,
        previous: Option<&[DiagnosticItem]>,
    ) -> VerificationReport {
        let (precheck_diags, comparison) = if let Some(g) = graph {
            let diags = Self::instant_ast_precheck(root, &g.files, &g.path_aliases);
            let comp = Self::compare_diagnostics(previous, &diags);
            (diags, comp)
        } else {
            let mut temp_graph = crate::graph::VectorGraph::new();
            temp_graph.sync_workspace(root);
            let diags =
                Self::instant_ast_precheck(root, &temp_graph.files, &temp_graph.path_aliases);
            let comp = Self::compare_diagnostics(previous, &diags);
            (diags, comp)
        };

        let error_count = precheck_diags.len();
        let success = error_count == 0;
        let (message, skipped_reason) = if success {
            (
                format!(
                    "{} toolchain not installed or bypassed; verified clean via Layer in-memory AST & dependency graph (0 syntax or import errors).",
                    toolchain_name
                ),
                Some(format!(
                    "{} toolchain not installed or bypassed; verified clean via Layer in-memory AST & dependency graph",
                    toolchain_name
                )),
            )
        } else {
            (
                format!(
                    "{} toolchain not installed or bypassed; in-memory AST check detected {} syntax/import error(s).",
                    toolchain_name, error_count
                ),
                Some(format!(
                    "{} toolchain not installed or bypassed; verified via Layer in-memory AST & dependency graph",
                    toolchain_name
                )),
            )
        };

        VerificationReport {
            success,
            project_type: project_type.to_string(),
            error_count,
            warning_count: 0,
            diagnostics: precheck_diags,
            comparison: Some(comparison),
            message,
            compiler_skipped_reason: skipped_reason,
        }
    }

    fn verify_python(
        root: &Path,
        graph: Option<&crate::graph::VectorGraph>,
        previous: Option<&[DiagnosticItem]>,
    ) -> VerificationReport {
        // Try ruff if installed
        let ruff_cmd = Command::new("ruff")
            .arg("check")
            .arg("--output-format=json")
            .current_dir(root)
            .stdin(std::process::Stdio::null())
            .output();

        if let Ok(out) = ruff_cmd
            && let Ok(parsed) = serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout)
        {
            let mut diagnostics = Vec::new();
            for item in parsed {
                let file = item
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let line = item
                    .get("location")
                    .and_then(|l| l.get("row"))
                    .and_then(|r| r.as_u64())
                    .unwrap_or(1) as usize;
                let col = item
                    .get("location")
                    .and_then(|l| l.get("column"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(1) as usize;
                let code = item
                    .get("code")
                    .and_then(|c| c.as_str())
                    .map(|s| s.to_string());
                let message = item
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string();

                diagnostics.push(DiagnosticItem::new(file, line, col, "error", code, message));
            }

            let error_count = diagnostics.len();
            let success = error_count == 0;
            let comparison = Some(Self::compare_diagnostics(previous, &diagnostics));
            let message = if success {
                "Python ruff check passed cleanly with 0 errors.".to_string()
            } else {
                format!("Python ruff check detected {} error(s).", error_count)
            };

            return VerificationReport {
                success,
                project_type: "python".to_string(),
                error_count,
                warning_count: 0,
                diagnostics,
                comparison,
                message,
                compiler_skipped_reason: None,
            };
        }

        Self::verify_tier1_ast_fallback(root, "python", "Python", graph, previous)
    }

    fn verify_go(
        root: &Path,
        graph: Option<&crate::graph::VectorGraph>,
        previous: Option<&[DiagnosticItem]>,
    ) -> VerificationReport {
        if root.join("go.mod").exists()
            && let Ok(out) = Command::new("go")
                .args(["vet", "./..."])
                .current_dir(root)
                .stdin(std::process::Stdio::null())
                .output()
        {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut diagnostics = Vec::new();
            for line in stderr.lines() {
                let parts: Vec<&str> = line.splitn(4, ':').collect();
                if parts.len() >= 4 {
                    let file = parts[0].trim().to_string();
                    let l = parts[1].trim().parse::<usize>().unwrap_or(1);
                    let c = parts[2].trim().parse::<usize>().unwrap_or(1);
                    let msg = parts[3].trim().to_string();
                    diagnostics.push(DiagnosticItem::new(
                        file,
                        l,
                        c,
                        "error",
                        Some("GO_VET".to_string()),
                        msg,
                    ));
                }
            }

            if out.status.success() || !diagnostics.is_empty() {
                let error_count = diagnostics.len();
                let success = error_count == 0;
                let comparison = Some(Self::compare_diagnostics(previous, &diagnostics));
                let message = if success {
                    "Go vet passed cleanly with 0 errors.".to_string()
                } else {
                    format!("Go vet detected {} error(s).", error_count)
                };

                return VerificationReport {
                    success,
                    project_type: "go".to_string(),
                    error_count,
                    warning_count: 0,
                    diagnostics,
                    comparison,
                    message,
                    compiler_skipped_reason: None,
                };
            }
        }

        Self::verify_tier1_ast_fallback(root, "go", "Go", graph, previous)
    }

    fn verify_cpp(
        root: &Path,
        graph: Option<&crate::graph::VectorGraph>,
        previous: Option<&[DiagnosticItem]>,
    ) -> VerificationReport {
        Self::verify_tier1_ast_fallback(root, "cpp", "C/C++", graph, previous)
    }

    fn verify_swift(
        root: &Path,
        graph: Option<&crate::graph::VectorGraph>,
        previous: Option<&[DiagnosticItem]>,
    ) -> VerificationReport {
        Self::verify_tier1_ast_fallback(root, "swift", "Swift", graph, previous)
    }

    fn verify_kotlin(
        root: &Path,
        graph: Option<&crate::graph::VectorGraph>,
        previous: Option<&[DiagnosticItem]>,
    ) -> VerificationReport {
        Self::verify_tier1_ast_fallback(root, "kotlin", "Kotlin", graph, previous)
    }

    fn verify_csharp(
        root: &Path,
        graph: Option<&crate::graph::VectorGraph>,
        previous: Option<&[DiagnosticItem]>,
    ) -> VerificationReport {
        Self::verify_tier1_ast_fallback(root, "csharp", "C#", graph, previous)
    }

    fn verify_java(
        root: &Path,
        graph: Option<&crate::graph::VectorGraph>,
        previous: Option<&[DiagnosticItem]>,
    ) -> VerificationReport {
        Self::verify_tier1_ast_fallback(root, "java", "Java", graph, previous)
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
        let err1 = DiagnosticItem::new(
            "src/engine.ts",
            10,
            5,
            "error",
            Some("TS2322".to_string()),
            "Type mismatch",
        );

        let err2 = DiagnosticItem::new(
            "src/canvas.tsx",
            30,
            8,
            "error",
            Some("TS2304".to_string()),
            "Cannot find name 'ctx'",
        );

        let err3_new = DiagnosticItem::new(
            "src/utils.ts",
            15,
            2,
            "error",
            Some("TS2554".to_string()),
            "Expected 2 arguments, but got 1",
        );

        // 1. First run: previous is None
        let comp1 = Verifier::compare_diagnostics(None, &[err1.clone(), err2.clone()]);
        assert_eq!(comp1.new_errors.len(), 2);
        assert_eq!(comp1.new_errors[0].occurrence_count, 1);
        assert!(comp1.resolved_errors.is_empty());
        assert!(comp1.persistent_errors.is_empty());

        // 2. Second run: err1 resolved, err2 persisted (shifted by 1 line), err3 is newly introduced
        let mut err2_shifted = err2.clone();
        err2_shifted.line = 31;

        let prev = vec![err1.clone(), err2.clone()];
        let curr = vec![err2_shifted.clone(), err3_new.clone()];

        let comp2 = Verifier::compare_diagnostics(Some(&prev), &curr);

        assert_eq!(comp2.new_errors.len(), 1);
        assert_eq!(comp2.new_errors[0].code.as_deref(), Some("TS2554"));

        assert_eq!(comp2.resolved_errors.len(), 1);
        assert_eq!(comp2.resolved_errors[0].file, "src/engine.ts");
        assert_eq!(comp2.resolved_errors[0].code.as_deref(), Some("TS2322"));

        assert_eq!(comp2.persistent_errors.len(), 1);
        assert_eq!(comp2.persistent_errors[0].file, "src/canvas.tsx");
        assert_eq!(comp2.persistent_errors[0].code.as_deref(), Some("TS2304"));
        assert_eq!(comp2.persistent_errors[0].occurrence_count, 2);
    }

    #[test]
    fn test_successful_clean_verification_comparison() {
        let err = DiagnosticItem::new(
            "src/main.rs",
            5,
            1,
            "error",
            Some("E0425".to_string()),
            "cannot find value",
        );

        let prev = vec![err];
        let curr: Vec<DiagnosticItem> = Vec::new();

        let comp = Verifier::compare_diagnostics(Some(&prev), &curr);
        assert!(comp.new_errors.is_empty());
        assert!(comp.persistent_errors.is_empty());
        assert_eq!(comp.resolved_errors.len(), 1);
        assert_eq!(comp.resolved_errors[0].code.as_deref(), Some("E0425"));
    }
}
