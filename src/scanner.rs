use crate::models::{Symbol, SymbolKind};
use regex::Regex;
use std::sync::LazyLock;

static RUST_FN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?(?:extern\s+(?:.*?\s+)?)?fn\s+([a-zA-Z0-9_]+)").unwrap()
});

static RUST_STRUCT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:pub(?:\s*\([^)]*\))?\s+)?(?:struct|enum|trait)\s+([a-zA-Z0-9_]+)").unwrap()
});

static JS_TS_FN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:export(?:\s+default)?\s+)?(?:async\s+)?function\s+([a-zA-Z0-9_]+)\s*\(").unwrap()
});

static JS_TS_METHOD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:(?:public|private|protected|static|async)\s+)+([a-zA-Z0-9_]+)\s*\(").unwrap()
});

static JS_TS_CLASS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:export(?:\s+default)?\s+)?(?:class|interface|type)\s+([a-zA-Z0-9_]+)").unwrap()
});

static JS_TS_CONST_FN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:export\s+)?(?:const|let|var)\s+([a-zA-Z0-9_]+)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[a-zA-Z0-9_]+)\s*=>").unwrap()
});

static PYTHON_DEF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:async\s+)?def\s+([a-zA-Z0-9_]+)\s*\(").unwrap()
});

static PYTHON_CLASS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*class\s+([a-zA-Z0-9_]+)").unwrap()
});

static GO_FUNC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*func\s+(?:\([^)]+\)\s+)?([a-zA-Z0-9_]+)\s*\(").unwrap()
});

static GENERIC_FN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:(?:virtual|static|inline|explicit|constexpr|friend)\s+)*(?:(?:[a-zA-Z_][a-zA-Z0-9_<>, \t*&:]*)\s+)?(~?[a-zA-Z_][a-zA-Z0-9_]*(?:::[a-zA-Z_][a-zA-Z0-9_]*)*)\s*\([^;]*\)\s*(?:const\s*)?(?:noexcept\s*)?(?:override\s*)?(?:final\s*)?(?::[^{]*)?\{?").unwrap()
});

static CPP_CLASS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:class|struct)\s+([a-zA-Z0-9_]+)\s*(?::\s*(?:public|protected|private)\s+[a-zA-Z0-9_:<>\s,]+)?\s*\{").unwrap()
});

pub struct AstScanner;

impl AstScanner {
    pub fn extract_symbols(content: &str, extension: Option<&str>) -> Vec<Symbol> {
        let lines: Vec<&str> = content.lines().collect();
        let ext = extension.unwrap_or("").to_lowercase();
        let mut symbols = Vec::new();

        match ext.as_str() {
            "rs" => Self::scan_rust(&lines, &mut symbols),
            "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => Self::scan_javascript_typescript(&lines, &mut symbols),
            "py" | "pyw" => Self::scan_python(&lines, &mut symbols),
            "go" => Self::scan_go(&lines, &mut symbols),
            _ => Self::scan_generic(&lines, &mut symbols),
        }

        // Sort by start_line
        symbols.sort_by_key(|s| s.start_line);
        symbols
    }

    pub fn extract_imports(content: &str, extension: Option<&str>) -> Vec<String> {
        let mut imports = Vec::new();
        let ext = extension.unwrap_or("").to_lowercase();
        for line in content.lines() {
            let trimmed = line.trim();
            match ext.as_str() {
                "rs" => {
                    if trimmed.starts_with("use ") || trimmed.starts_with("mod ") {
                        imports.push(trimmed.trim_end_matches(';').to_string());
                    }
                }
                "js" | "jsx" | "ts" | "tsx" => {
                    if trimmed.starts_with("import ") || (trimmed.contains("require(") && !trimmed.starts_with("//")) {
                        imports.push(trimmed.to_string());
                    }
                }
                "py" => {
                    if trimmed.starts_with("import ") || trimmed.starts_with("from ") {
                        imports.push(trimmed.to_string());
                    }
                }
                "go" => {
                    if trimmed.starts_with("import ") {
                        imports.push(trimmed.to_string());
                    }
                }
                _ => {}
            }
        }
        imports
    }

    fn scan_rust(lines: &[&str], symbols: &mut Vec<Symbol>) {
        for (idx, &line) in lines.iter().enumerate() {
            let line_num = idx + 1;
            if let Some(caps) = RUST_FN_RE.captures(line) {
                if let Some(name_match) = caps.get(1) {
                    let end_line = find_block_end(lines, idx);
                    symbols.push(Symbol {
                        name: name_match.as_str().to_string(),
                        kind: SymbolKind::Function,
                        start_line: line_num,
                        end_line,
                        signature: line.trim().to_string(),
                        doc_comment: find_doc_comment(lines, idx),
                    });
                }
            } else if let Some(caps) = RUST_STRUCT_RE.captures(line) {
                if let Some(name_match) = caps.get(1) {
                    let end_line = find_block_end(lines, idx);
                    let kind = if line.contains("struct ") {
                        SymbolKind::Struct
                    } else if line.contains("enum ") {
                        SymbolKind::Enum
                    } else {
                        SymbolKind::Trait
                    };
                    symbols.push(Symbol {
                        name: name_match.as_str().to_string(),
                        kind,
                        start_line: line_num,
                        end_line,
                        signature: line.trim().to_string(),
                        doc_comment: find_doc_comment(lines, idx),
                    });
                }
            }
        }
    }

    fn scan_javascript_typescript(lines: &[&str], symbols: &mut Vec<Symbol>) {
        for (idx, &line) in lines.iter().enumerate() {
            let line_num = idx + 1;
            if let Some(caps) = JS_TS_FN_RE.captures(line) {
                if let Some(m) = caps.get(1) {
                    let end_line = find_block_end(lines, idx);
                    symbols.push(Symbol {
                        name: m.as_str().to_string(),
                        kind: SymbolKind::Function,
                        start_line: line_num,
                        end_line,
                        signature: line.trim().to_string(),
                        doc_comment: find_doc_comment(lines, idx),
                    });
                }
            } else if let Some(caps) = JS_TS_CONST_FN_RE.captures(line) {
                if let Some(m) = caps.get(1) {
                    let end_line = find_block_end(lines, idx);
                    symbols.push(Symbol {
                        name: m.as_str().to_string(),
                        kind: SymbolKind::Function,
                        start_line: line_num,
                        end_line,
                        signature: line.trim().to_string(),
                        doc_comment: find_doc_comment(lines, idx),
                    });
                }
            } else if let Some(caps) = JS_TS_CLASS_RE.captures(line) {
                if let Some(m) = caps.get(1) {
                    let end_line = find_block_end(lines, idx);
                    symbols.push(Symbol {
                        name: m.as_str().to_string(),
                        kind: SymbolKind::Class,
                        start_line: line_num,
                        end_line,
                        signature: line.trim().to_string(),
                        doc_comment: find_doc_comment(lines, idx),
                    });
                }
            } else if let Some(caps) = JS_TS_METHOD_RE.captures(line) {
                if let Some(m) = caps.get(1) {
                    let name = m.as_str();
                    if name != "if" && name != "for" && name != "while" && name != "switch" {
                        let end_line = find_block_end(lines, idx);
                        symbols.push(Symbol {
                            name: name.to_string(),
                            kind: SymbolKind::Method,
                            start_line: line_num,
                            end_line,
                            signature: line.trim().to_string(),
                            doc_comment: find_doc_comment(lines, idx),
                        });
                    }
                }
            }
        }
    }

    fn scan_python(lines: &[&str], symbols: &mut Vec<Symbol>) {
        for (idx, &line) in lines.iter().enumerate() {
            let line_num = idx + 1;
            if let Some(caps) = PYTHON_DEF_RE.captures(line) {
                if let Some(m) = caps.get(1) {
                    let end_line = find_python_block_end(lines, idx);
                    let indent = line.len() - line.trim_start().len();
                    let kind = if indent > 0 { SymbolKind::Method } else { SymbolKind::Function };
                    symbols.push(Symbol {
                        name: m.as_str().to_string(),
                        kind,
                        start_line: line_num,
                        end_line,
                        signature: line.trim().to_string(),
                        doc_comment: find_python_docstring(lines, idx),
                    });
                }
            } else if let Some(caps) = PYTHON_CLASS_RE.captures(line) {
                if let Some(m) = caps.get(1) {
                    let end_line = find_python_block_end(lines, idx);
                    symbols.push(Symbol {
                        name: m.as_str().to_string(),
                        kind: SymbolKind::Class,
                        start_line: line_num,
                        end_line,
                        signature: line.trim().to_string(),
                        doc_comment: find_python_docstring(lines, idx),
                    });
                }
            }
        }
    }

    fn scan_go(lines: &[&str], symbols: &mut Vec<Symbol>) {
        for (idx, &line) in lines.iter().enumerate() {
            let line_num = idx + 1;
            if let Some(caps) = GO_FUNC_RE.captures(line) {
                if let Some(m) = caps.get(1) {
                    let end_line = find_block_end(lines, idx);
                    symbols.push(Symbol {
                        name: m.as_str().to_string(),
                        kind: SymbolKind::Function,
                        start_line: line_num,
                        end_line,
                        signature: line.trim().to_string(),
                        doc_comment: find_doc_comment(lines, idx),
                    });
                }
            }
        }
    }

    fn scan_generic(lines: &[&str], symbols: &mut Vec<Symbol>) {
        for (idx, &line) in lines.iter().enumerate() {
            let line_num = idx + 1;
            if let Some(caps) = CPP_CLASS_RE.captures(line) {
                if let Some(m) = caps.get(1) {
                    let end_line = find_block_end(lines, idx);
                    let kind = if line.trim_start().starts_with("struct") {
                        SymbolKind::Struct
                    } else {
                        SymbolKind::Class
                    };
                    symbols.push(Symbol {
                        name: m.as_str().to_string(),
                        kind,
                        start_line: line_num,
                        end_line,
                        signature: line.trim().to_string(),
                        doc_comment: find_doc_comment(lines, idx),
                    });
                }
            } else if let Some(caps) = GENERIC_FN_RE.captures(line) {
                if let Some(m) = caps.get(1) {
                    let name = m.as_str();
                    if !["if", "for", "while", "switch", "catch", "return", "sizeof"].contains(&name) {
                        let end_line = find_block_end(lines, idx);
                        symbols.push(Symbol {
                            name: name.to_string(),
                            kind: SymbolKind::Function,
                            start_line: line_num,
                            end_line,
                            signature: line.trim().to_string(),
                            doc_comment: find_doc_comment(lines, idx),
                        });
                    }
                }
            }
        }
    }
}

fn find_block_end(lines: &[&str], start_idx: usize) -> usize {
    let mut brace_count = 0;
    let mut found_open = false;

    for i in start_idx..lines.len() {
        let line = lines[i];
        for ch in line.chars() {
            if ch == '{' {
                brace_count += 1;
                found_open = true;
            } else if ch == '}' {
                brace_count -= 1;
                if found_open && brace_count <= 0 {
                    return i + 1;
                }
            } else if ch == ';' && !found_open {
                // Semicolon before any opening brace => declaration without body
                return i + 1;
            }
        }
        if !found_open && i > start_idx + 4 {
            return start_idx + 1;
        }
    }
    start_idx + 1
}

fn find_python_block_end(lines: &[&str], start_idx: usize) -> usize {
    let base_line = lines[start_idx];
    let base_indent = base_line.len() - base_line.trim_start().len();

    let mut last_content_line = start_idx + 1;
    for i in (start_idx + 1)..lines.len() {
        let line = lines[i];
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let current_indent = line.len() - line.trim_start().len();
        if current_indent <= base_indent {
            return last_content_line;
        }
        last_content_line = i + 1;
    }
    lines.len()
}

fn find_doc_comment(lines: &[&str], start_idx: usize) -> Option<String> {
    if start_idx == 0 {
        return None;
    }
    let mut comments = Vec::new();
    let mut i = start_idx;
    while i > 0 {
        i -= 1;
        let line = lines[i].trim();
        if line.starts_with("///") || line.starts_with("//!") || line.starts_with("/**") || line.starts_with("*") {
            comments.push(line.to_string());
        } else if line.is_empty() {
            continue;
        } else {
            break;
        }
    }
    if comments.is_empty() {
        None
    } else {
        comments.reverse();
        Some(comments.join("\n"))
    }
}

fn find_python_docstring(lines: &[&str], start_idx: usize) -> Option<String> {
    if start_idx + 1 >= lines.len() {
        return None;
    }
    let next_line = lines[start_idx + 1].trim();
    if next_line.starts_with("\"\"\"") || next_line.starts_with("'''") {
        Some(next_line.to_string())
    } else {
        None
    }
}
