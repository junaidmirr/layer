use crate::models::{Symbol, SymbolKind};
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::{Language, Node, Parser};

// Fallback regexes only for Tier-2 languages without dedicated compiled Tree-sitter grammars (e.g. Java, Kotlin, Swift, C#, PHP, Zig)
static GENERIC_FN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:(?:public|private|protected|internal|virtual|static|inline|explicit|constexpr|friend|async|override|final|pub|fn|func|def)\s+)+(?:[a-zA-Z_][a-zA-Z0-9_<>, \t*&:\[\]]*\s+)?([a-zA-Z_][a-zA-Z0-9_]*)\s*\([^;]*\)\s*(?:const\s*)?(?:noexcept\s*)?(?:override\s*)?(?:final\s*)?(?:->\s*[^{]+)?\{").unwrap()
});

static GENERIC_CLASS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:(?:public|private|protected|internal|abstract|final|sealed|open|data|pub)\s+)*(?:class|struct|interface|enum|trait)\s+([a-zA-Z0-9_]+)").unwrap()
});

/// Represents an AST-verified symbol reference / usage site in source code
#[derive(Debug, Clone)]
pub struct AstUsage {
    pub line: usize, // 1-indexed
    pub line_text: String,
    pub is_definite_code_call_or_ref: bool,
}

pub struct AstScanner;

impl AstScanner {
    /// Returns the compiled Tree-sitter grammar for supported Tier-1 languages
    pub fn get_language(extension: Option<&str>) -> Option<Language> {
        let ext = extension?.to_lowercase();
        match ext.as_str() {
            "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
            "js" | "mjs" | "cjs" | "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),
            "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
            "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
            "py" | "pyw" | "pyi" => Some(tree_sitter_python::LANGUAGE.into()),
            "go" => Some(tree_sitter_go::LANGUAGE.into()),
            "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" | "hxx" => {
                Some(tree_sitter_cpp::LANGUAGE.into())
            }
            _ => None,
        }
    }

    /// Extract symbols using Tree-sitter Concrete Syntax Trees (CST/AST),
    /// falling back to block-scoped lexical heuristics only for unsupported extensions.
    pub fn extract_symbols(content: &str, extension: Option<&str>) -> Vec<Symbol> {
        let ext = extension.unwrap_or("").to_lowercase();
        let mut symbols = Vec::new();

        if let Some(lang) = Self::get_language(Some(&ext))
            && let Some(tree) = Self::parse_tree(content, &lang)
        {
            let lines: Vec<&str> = content.lines().collect();
            Self::walk_ast_symbols(
                tree.root_node(),
                content.as_bytes(),
                &lines,
                &ext,
                false,
                &mut symbols,
            );
            symbols.sort_by_key(|s| (s.start_line, s.end_line));
            symbols.dedup_by(|a, b| a.name == b.name && a.start_line == b.start_line);
            return symbols;
        }

        // Fallback for Tier-2 OOP languages without embedded Tree-sitter C grammars
        let lines: Vec<&str> = content.lines().collect();
        if matches!(
            ext.as_str(),
            "java" | "kt" | "kts" | "swift" | "cs" | "php" | "scala" | "dart" | "zig"
        ) {
            Self::scan_fallback_oop(&lines, &mut symbols);
        }

        symbols.sort_by_key(|s| s.start_line);
        symbols
    }

    /// Extract module imports, includes, and re-exports via Tree-sitter AST nodes
    pub fn extract_imports(content: &str, extension: Option<&str>) -> Vec<String> {
        let ext = extension.unwrap_or("").to_lowercase();
        let mut imports = Vec::new();

        if let Some(lang) = Self::get_language(Some(&ext))
            && let Some(tree) = Self::parse_tree(content, &lang)
        {
            Self::walk_ast_imports(tree.root_node(), content.as_bytes(), &ext, &mut imports);
            return imports;
        }

        // Fallback import extraction for other languages
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("import ") || trimmed.starts_with("using ") {
                imports.push(trimmed.trim_end_matches(';').to_string());
            }
        }
        imports
    }

    /// AST-verified symbol reference finder.
    /// Uses Tree-sitter to locate true identifier/type/call nodes, automatically ignoring
    /// occurrences inside comments (`//`, `/* */`) and string literals (`"..."`).
    pub fn find_ast_usages(
        content: &str,
        extension: Option<&str>,
        symbol_name: &str,
    ) -> Option<Vec<AstUsage>> {
        let ext = extension?.to_lowercase();
        let lang = Self::get_language(Some(&ext))?;
        let tree = Self::parse_tree(content, &lang)?;
        let lines: Vec<&str> = content.lines().collect();

        let mut usages = Vec::new();
        let mut seen_lines = std::collections::HashSet::new();

        Self::walk_ast_usages(
            tree.root_node(),
            content.as_bytes(),
            &lines,
            symbol_name,
            &mut seen_lines,
            &mut usages,
        );

        Some(usages)
    }

    fn parse_tree(content: &str, lang: &Language) -> Option<tree_sitter::Tree> {
        let mut parser = Parser::new();
        parser.set_language(lang).ok()?;
        parser.parse(content, None)
    }

    #[allow(clippy::only_used_in_recursion)]
    fn walk_ast_symbols(
        node: Node,
        source: &[u8],
        lines: &[&str],
        ext: &str,
        in_container: bool,
        symbols: &mut Vec<Symbol>,
    ) {
        let kind_str = node.kind();
        let mut next_in_container = in_container;

        match ext {
            "rs" => match kind_str {
                "impl_item" | "trait_item" => {
                    next_in_container = true;
                    if kind_str == "trait_item"
                        && let Some(name) = Self::node_field_text(node, "name", source)
                    {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Trait, symbols);
                    }
                }
                "function_item" | "function_signature_item" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        let kind = if in_container {
                            SymbolKind::Method
                        } else {
                            SymbolKind::Function
                        };
                        Self::push_ast_symbol(node, lines, name, kind, symbols);
                    }
                }
                "struct_item" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Struct, symbols);
                    }
                }
                "enum_item" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Enum, symbols);
                    }
                }
                "type_item" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::TypeAlias, symbols);
                    }
                }
                "mod_item" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Module, symbols);
                    }
                }
                "const_item" | "static_item" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Constant, symbols);
                    }
                }
                _ => {}
            },
            "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => match kind_str {
                "class_declaration" => {
                    next_in_container = true;
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Class, symbols);
                    }
                }
                "interface_declaration" => {
                    next_in_container = true;
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Interface, symbols);
                    }
                }
                "enum_declaration" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Enum, symbols);
                    }
                }
                "type_alias_declaration" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::TypeAlias, symbols);
                    }
                }
                "function_declaration" | "generator_function_declaration" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Function, symbols);
                    }
                }
                "method_definition" | "method_signature" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Method, symbols);
                    }
                }
                "variable_declarator" => {
                    if let Some(val) = node.child_by_field_name("value")
                        && matches!(val.kind(), "arrow_function" | "function_expression")
                        && let Some(name) = Self::node_field_text(node, "name", source)
                    {
                        let span_node = node
                            .parent()
                            .and_then(|p| {
                                if p.parent().is_some_and(|gp| gp.kind() == "export_statement") {
                                    p.parent()
                                } else {
                                    Some(p)
                                }
                            })
                            .unwrap_or(node);
                        Self::push_ast_symbol(
                            span_node,
                            lines,
                            name,
                            SymbolKind::Function,
                            symbols,
                        );
                    }
                }
                _ => {}
            },
            "py" | "pyw" | "pyi" => match kind_str {
                "class_definition" => {
                    next_in_container = true;
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        let doc = Self::extract_python_ast_docstring(node, source);
                        Self::push_ast_symbol_with_doc(
                            node,
                            lines,
                            name,
                            SymbolKind::Class,
                            doc,
                            symbols,
                        );
                    }
                }
                "function_definition" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        let kind = if in_container {
                            SymbolKind::Method
                        } else {
                            SymbolKind::Function
                        };
                        let doc = Self::extract_python_ast_docstring(node, source);
                        Self::push_ast_symbol_with_doc(node, lines, name, kind, doc, symbols);
                    }
                }
                _ => {}
            },
            "go" => match kind_str {
                "function_declaration" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Function, symbols);
                    }
                }
                "method_declaration" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Method, symbols);
                    }
                }
                "type_spec" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        let kind = match node.child_by_field_name("type").map(|t| t.kind()) {
                            Some("struct_type") => SymbolKind::Struct,
                            Some("interface_type") => SymbolKind::Interface,
                            _ => SymbolKind::TypeAlias,
                        };
                        Self::push_ast_symbol(node, lines, name, kind, symbols);
                    }
                }
                _ => {}
            },
            "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" | "hxx" => match kind_str {
                "class_specifier" => {
                    next_in_container = true;
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Class, symbols);
                    }
                }
                "struct_specifier" => {
                    next_in_container = true;
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Struct, symbols);
                    }
                }
                "enum_specifier" => {
                    if let Some(name) = Self::node_field_text(node, "name", source) {
                        Self::push_ast_symbol(node, lines, name, SymbolKind::Enum, symbols);
                    }
                }
                "function_definition" => {
                    if let Some(decl) = node.child_by_field_name("declarator")
                        && let Some(name) = Self::extract_cpp_declarator_name(decl, source)
                    {
                        let kind = if in_container || name.contains("::") {
                            SymbolKind::Method
                        } else {
                            SymbolKind::Function
                        };
                        Self::push_ast_symbol(node, lines, name, kind, symbols);
                    }
                }
                _ => {}
            },
            _ => {}
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            Self::walk_ast_symbols(child, source, lines, ext, next_in_container, symbols);
        }
    }

    fn walk_ast_imports(node: Node, source: &[u8], ext: &str, imports: &mut Vec<String>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let k = child.kind();
            let is_import = match ext {
                "rs" => matches!(
                    k,
                    "use_declaration" | "extern_crate_declaration" | "mod_item"
                ),
                "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => {
                    k == "import_statement"
                        || (k == "export_statement"
                            && (child.child_by_field_name("source").is_some()
                                || child.utf8_text(source).is_ok_and(|t| {
                                    t.starts_with("export {") || t.starts_with("export *")
                                })))
                }
                "py" | "pyw" | "pyi" => matches!(k, "import_statement" | "import_from_statement"),
                "go" => k == "import_declaration",
                "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" | "hxx" => k == "preproc_include",
                _ => false,
            };

            if is_import {
                // For Rust mod_item, only treat declaration `pub mod foo;` as an import/export entry, not inline `mod tests { ... }`
                if ext == "rs" && k == "mod_item" && child.child_by_field_name("body").is_some() {
                    continue;
                }
                if let Ok(text) = child.utf8_text(source) {
                    let first_line = text.lines().next().unwrap_or("").trim();
                    if !first_line.is_empty() {
                        imports.push(first_line.trim_end_matches(';').to_string());
                    }
                }
            }
        }
    }

    fn walk_ast_usages(
        node: Node,
        source: &[u8],
        lines: &[&str],
        symbol_name: &str,
        seen_lines: &mut std::collections::HashSet<usize>,
        usages: &mut Vec<AstUsage>,
    ) {
        let kind = node.kind();

        // Explicitly skip comment and string literal subtrees so they never trigger false-positive references
        if kind.contains("comment")
            || matches!(
                kind,
                "string_literal"
                    | "raw_string_literal"
                    | "string"
                    | "string_content"
                    | "template_string"
            )
        {
            return;
        }

        // Check if this leaf node is an identifier matching the symbol
        if matches!(
            kind,
            "identifier"
                | "type_identifier"
                | "field_identifier"
                | "property_identifier"
                | "shorthand_property_identifier"
        ) && let Ok(text) = node.utf8_text(source)
            && text == symbol_name
        {
            // Check if this node is the declaration name of the symbol itself
            if !Self::is_declaration_name_node(node, source, symbol_name) {
                let line_num = node.start_position().row + 1;
                if seen_lines.insert(line_num) {
                    let line_text = lines
                        .get(line_num.saturating_sub(1))
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default();
                    let is_definite = Self::is_definite_ast_usage(node);
                    usages.push(AstUsage {
                        line: line_num,
                        line_text,
                        is_definite_code_call_or_ref: is_definite,
                    });
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            Self::walk_ast_usages(child, source, lines, symbol_name, seen_lines, usages);
        }
    }

    /// Determines if an identifier node is the name field of its own declaration
    fn is_declaration_name_node(node: Node, source: &[u8], symbol_name: &str) -> bool {
        let Some(parent) = node.parent() else {
            return false;
        };
        let pk = parent.kind();
        if matches!(
            pk,
            "function_item"
                | "function_signature_item"
                | "struct_item"
                | "enum_item"
                | "trait_item"
                | "type_item"
                | "mod_item"
                | "const_item"
                | "static_item"
                | "function_declaration"
                | "method_definition"
                | "class_declaration"
                | "interface_declaration"
                | "type_alias_declaration"
                | "enum_declaration"
                | "function_definition"
                | "class_definition"
                | "type_spec"
                | "class_specifier"
                | "struct_specifier"
                | "enum_specifier"
        ) && let Some(name_node) = parent.child_by_field_name("name")
            && name_node.id() == node.id()
        {
            return true;
        }

        // Handle C++ function_declarator name
        if pk == "function_declarator"
            && parent
                .parent()
                .is_some_and(|gp| gp.kind() == "function_definition")
            && let Some(decl_name) = Self::extract_cpp_declarator_name(parent, source)
            && (decl_name == symbol_name || decl_name.ends_with(&format!("::{}", symbol_name)))
        {
            return true;
        }

        false
    }

    /// Inspects parent AST nodes to determine if an identifier is used in a direct call, construction, type, or scoped access
    fn is_definite_ast_usage(node: Node) -> bool {
        let mut curr = node.parent();
        for _ in 0..3 {
            let Some(p) = curr else { break };
            match p.kind() {
                "call_expression"
                | "macro_invocation"
                | "struct_expression"
                | "new_expression"
                | "jsx_opening_element"
                | "jsx_self_closing_element"
                | "scoped_identifier"
                | "scoped_type_identifier"
                | "field_expression"
                | "member_expression"
                | "generic_type"
                | "type_annotation"
                | "parameter" => return true,
                _ => curr = p.parent(),
            }
        }
        node.kind() == "type_identifier"
    }

    fn node_field_text(node: Node, field: &str, source: &[u8]) -> Option<String> {
        let child = node.child_by_field_name(field)?;
        let text = child.utf8_text(source).ok()?.trim();
        if text.is_empty() {
            None
        } else {
            Some(text.to_string())
        }
    }

    fn extract_cpp_declarator_name(node: Node, source: &[u8]) -> Option<String> {
        match node.kind() {
            "identifier" | "field_identifier" | "destructor_name" | "qualified_identifier" => {
                node.utf8_text(source).ok().map(|s| s.trim().to_string())
            }
            "function_declarator"
            | "pointer_declarator"
            | "reference_declarator"
            | "parenthesized_declarator" => {
                if let Some(inner) = node.child_by_field_name("declarator") {
                    Self::extract_cpp_declarator_name(inner, source)
                } else {
                    let mut cursor = node.walk();
                    for child in node.children(&mut cursor) {
                        if let Some(name) = Self::extract_cpp_declarator_name(child, source) {
                            return Some(name);
                        }
                    }
                    None
                }
            }
            _ => None,
        }
    }

    fn push_ast_symbol(
        node: Node,
        lines: &[&str],
        name: String,
        kind: SymbolKind,
        symbols: &mut Vec<Symbol>,
    ) {
        let start_idx = node.start_position().row;
        let doc = find_preceding_doc_comment(lines, start_idx);
        Self::push_ast_symbol_with_doc(node, lines, name, kind, doc, symbols);
    }

    fn push_ast_symbol_with_doc(
        node: Node,
        lines: &[&str],
        name: String,
        kind: SymbolKind,
        doc_comment: Option<String>,
        symbols: &mut Vec<Symbol>,
    ) {
        let start_line = node.start_position().row + 1;
        let end_line = (node.end_position().row + 1).max(start_line);
        let sig_line = lines
            .get(node.start_position().row)
            .map(|l| l.trim().trim_end_matches(';').to_string())
            .unwrap_or_else(|| name.clone());

        symbols.push(Symbol {
            name,
            kind,
            start_line,
            end_line,
            signature: sig_line,
            doc_comment,
        });
    }

    fn extract_python_ast_docstring(node: Node, source: &[u8]) -> Option<String> {
        let body = node.child_by_field_name("body")?;
        let first_stmt = body.named_child(0)?;
        if first_stmt.kind() == "expression_statement"
            && let Some(str_node) = first_stmt.named_child(0)
            && str_node.kind() == "string"
            && let Ok(text) = str_node.utf8_text(source)
        {
            let cleaned = text
                .trim()
                .trim_start_matches("\"\"\"")
                .trim_end_matches("\"\"\"")
                .trim_start_matches("'''")
                .trim_end_matches("'''")
                .trim();
            if !cleaned.is_empty() {
                return Some(cleaned.to_string());
            }
        }
        None
    }

    fn scan_fallback_oop(lines: &[&str], symbols: &mut Vec<Symbol>) {
        for (idx, &line) in lines.iter().enumerate() {
            let line_num = idx + 1;
            if let Some(caps) = GENERIC_CLASS_RE.captures(line)
                && let Some(m) = caps.get(1)
            {
                let end_line = find_block_end(lines, idx);
                let kind = if line.contains("struct ") {
                    SymbolKind::Struct
                } else if line.contains("interface ") {
                    SymbolKind::Interface
                } else if line.contains("enum ") {
                    SymbolKind::Enum
                } else {
                    SymbolKind::Class
                };
                symbols.push(Symbol {
                    name: m.as_str().to_string(),
                    kind,
                    start_line: line_num,
                    end_line,
                    signature: line.trim().to_string(),
                    doc_comment: find_preceding_doc_comment(lines, idx),
                });
            } else if let Some(caps) = GENERIC_FN_RE.captures(line)
                && let Some(m) = caps.get(1)
            {
                let name = m.as_str();
                if !["if", "for", "while", "switch", "catch", "return", "sizeof"].contains(&name) {
                    let end_line = find_block_end(lines, idx);
                    symbols.push(Symbol {
                        name: name.to_string(),
                        kind: SymbolKind::Function,
                        start_line: line_num,
                        end_line,
                        signature: line.trim().to_string(),
                        doc_comment: find_preceding_doc_comment(lines, idx),
                    });
                }
            }
        }
    }
}

fn find_block_end(lines: &[&str], start_idx: usize) -> usize {
    let mut brace_count = 0;
    let mut found_open = false;

    for (i, &line) in lines.iter().enumerate().skip(start_idx) {
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
                return i + 1;
            }
        }
        if !found_open && i > start_idx + 4 {
            return start_idx + 1;
        }
    }
    start_idx + 1
}

fn find_preceding_doc_comment(lines: &[&str], start_idx: usize) -> Option<String> {
    if start_idx == 0 {
        return None;
    }
    let mut comments = Vec::new();
    let mut i = start_idx;
    while i > 0 {
        i -= 1;
        let line = lines[i].trim();
        if line.starts_with("///")
            || line.starts_with("//!")
            || line.starts_with("/**")
            || line.starts_with('*')
        {
            comments.push(line.to_string());
        } else if line.starts_with("#[") || line.is_empty() {
            // Skip Rust attributes (#[derive(...)], #[cfg(...)]) between doc comment and item
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_sitter_rust_ast_extraction() {
        let code = r#"
use std::collections::HashMap;
pub use crate::models::Symbol;

/// Processes an incoming IPC frame.
#[inline]
pub async fn handle_frame(payload: &[u8]) -> Result<(), String> {
    let raw_str = "fn fake_function_inside_string() {}";
    // fn fake_function_inside_comment() {}
    Ok(())
}

pub struct FrameRouter {
    pub routes: HashMap<String, usize>,
}

impl FrameRouter {
    pub fn dispatch(&self, name: &str) -> bool {
        let _ = handle_frame(b"test");
        true
    }
}
"#;
        let symbols = AstScanner::extract_symbols(code, Some("rs"));
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["handle_frame", "FrameRouter", "dispatch"]);
        assert_eq!(symbols[0].kind, SymbolKind::Function);
        assert!(
            symbols[0]
                .doc_comment
                .as_deref()
                .unwrap_or("")
                .contains("Processes an incoming IPC frame")
        );
        assert_eq!(symbols[1].kind, SymbolKind::Struct);
        assert_eq!(symbols[2].kind, SymbolKind::Method);

        let imports = AstScanner::extract_imports(code, Some("rs"));
        assert_eq!(imports.len(), 2);

        // Verify AST usage tracking ignores comments and string literals
        let usages = AstScanner::find_ast_usages(code, Some("rs"), "handle_frame").unwrap();
        assert_eq!(usages.len(), 1);
        assert!(usages[0].line_text.contains("handle_frame(b\"test\")"));
        assert!(usages[0].is_definite_code_call_or_ref);
    }

    #[test]
    fn test_tree_sitter_typescript_and_python_ast() {
        let ts_code = r#"
import { useState } from 'react';

export interface UserProps {
    id: string;
}

export const UserCard = ({ id }: UserProps) => {
    // UserProps in comment should be ignored
    const label = "UserProps in string should be ignored";
    return null;
};
"#;
        let ts_syms = AstScanner::extract_symbols(ts_code, Some("tsx"));
        let ts_names: Vec<&str> = ts_syms.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(ts_names, vec!["UserProps", "UserCard"]);

        let usages = AstScanner::find_ast_usages(ts_code, Some("tsx"), "UserProps").unwrap();
        // Only the type annotation `: UserProps` on line 8 is matched; comment & string are ignored!
        assert_eq!(usages.len(), 1);
        assert_eq!(usages[0].line, 8);

        let py_code = r#"
import os

class DataPipeline:
    """Pipeline for batch records."""
    def run_batch(self, items):
        """Executes batch."""
        return len(items)
"#;
        let py_syms = AstScanner::extract_symbols(py_code, Some("py"));
        assert_eq!(py_syms.len(), 2);
        assert_eq!(py_syms[0].name, "DataPipeline");
        assert_eq!(py_syms[0].kind, SymbolKind::Class);
        assert_eq!(
            py_syms[0].doc_comment.as_deref(),
            Some("Pipeline for batch records.")
        );
        assert_eq!(py_syms[1].name, "run_batch");
        assert_eq!(py_syms[1].kind, SymbolKind::Method);
    }
}
