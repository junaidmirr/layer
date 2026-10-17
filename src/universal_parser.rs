use crate::models::{Symbol, SymbolKind};
use std::collections::HashSet;

/// Universal Lexical & Block Automaton (ULBA)
/// Zero-dependency, O(N) single-pass parser extracting symbols, line spans [start_line, end_line],
/// signatures, and imports across 25+ languages.
pub struct UniversalParser;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommentStyle {
    SlashSlash, // // ...
    Hash,       // # ...
    DashDash,   // -- ...
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockType {
    Brace,
    KeywordRuby,
    KeywordElixir,
    KeywordLua,
    SqlTable,
    SqlRoutine,
}

struct PendingDecl {
    name: String,
    kind: SymbolKind,
    start_line: usize,
    signature: String,
    doc_comment: Option<String>,
    block_type: BlockType,
    brace_depth_at_open: usize,
}

impl UniversalParser {
    /// Extract symbols and import statements for any supported polyglot language
    pub fn parse(content: &str, extension: &str) -> (Vec<Symbol>, Vec<String>) {
        let ext = extension.trim_start_matches('.').to_lowercase();
        let lines: Vec<&str> = content.lines().collect();
        let mut symbols = Vec::new();
        let mut imports = Vec::new();

        let comment_style = match ext.as_str() {
            "rb" | "ex" | "exs" | "sh" | "bash" | "py" | "pyi" | "tf" | "hcl" => CommentStyle::Hash,
            "sql" | "lua" => CommentStyle::DashDash,
            _ => CommentStyle::SlashSlash, // java, cs, scala, php, dart, zig, proto, graphql, swift, kt, groovy, sol, etc.
        };

        // 1. First pass: extract imports and comments
        Self::extract_polyglot_imports(&lines, &ext, &mut imports);

        // 2. Second pass: block tracking and symbol extraction
        Self::scan_symbols_and_blocks(&lines, &ext, comment_style, &mut symbols);

        symbols.sort_by_key(|s| (s.start_line, s.end_line));
        symbols.dedup_by(|a, b| a.name == b.name && a.start_line == b.start_line);

        (symbols, imports)
    }

    /// Delimiter and syntax balance check across polyglot languages
    pub fn check_syntax(content: &str, extension: &str) -> Result<(), (usize, usize, String)> {
        let ext = extension.trim_start_matches('.').to_lowercase();
        let comment_style = match ext.as_str() {
            "rb" | "ex" | "exs" | "sh" | "bash" | "py" | "pyi" | "tf" | "hcl" => CommentStyle::Hash,
            "sql" | "lua" => CommentStyle::DashDash,
            _ => CommentStyle::SlashSlash,
        };

        let mut stack: Vec<(char, usize, usize)> = Vec::new();
        let mut in_line_comment = false;
        let mut in_block_comment = false;
        let mut in_string = false;
        let mut string_quote = '"';
        let mut escaped = false;

        let mut line = 1;
        let mut col = 0;
        let chars: Vec<char> = content.chars().collect();
        let len = chars.len();
        let mut i = 0;

        while i < len {
            let ch = chars[i];
            col += 1;

            if ch == '\n' {
                line += 1;
                col = 0;
                in_line_comment = false;
                escaped = false;
                i += 1;
                continue;
            }

            if in_line_comment {
                i += 1;
                continue;
            }

            if in_block_comment {
                if ext == "lua" {
                    if ch == ']' && i + 1 < len && chars[i + 1] == ']' {
                        in_block_comment = false;
                        i += 2;
                        col += 1;
                        continue;
                    }
                } else if ch == '*' && i + 1 < len && chars[i + 1] == '/' {
                    in_block_comment = false;
                    i += 2;
                    col += 1;
                    continue;
                }
                i += 1;
                continue;
            }

            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == string_quote {
                    in_string = false;
                }
                i += 1;
                continue;
            }

            // Check comment starts
            match comment_style {
                CommentStyle::Hash => {
                    if ch == '#' {
                        in_line_comment = true;
                        i += 1;
                        continue;
                    }
                }
                CommentStyle::DashDash => {
                    if ch == '-' && i + 1 < len && chars[i + 1] == '-' {
                        if ext == "lua" && i + 3 < len && chars[i + 2] == '[' && chars[i + 3] == '['
                        {
                            in_block_comment = true;
                            i += 4;
                            col += 3;
                            continue;
                        }
                        in_line_comment = true;
                        i += 2;
                        col += 1;
                        continue;
                    }
                }
                CommentStyle::SlashSlash => {
                    if ch == '/' && i + 1 < len {
                        if chars[i + 1] == '/' {
                            in_line_comment = true;
                            i += 2;
                            col += 1;
                            continue;
                        } else if chars[i + 1] == '*' {
                            in_block_comment = true;
                            i += 2;
                            col += 1;
                            continue;
                        }
                    }
                }
            }

            // Check string starts
            if ch == '"' || ch == '\'' || ch == '`' {
                // Check for triple quotes
                if (ch == '"' || ch == '\'')
                    && i + 2 < len
                    && chars[i + 1] == ch
                    && chars[i + 2] == ch
                {
                    in_string = true;
                    string_quote = ch;
                    escaped = false;
                    i += 3;
                    col += 2;
                    continue;
                }
                in_string = true;
                string_quote = ch;
                escaped = false;
                i += 1;
                continue;
            }

            // Delimiter balancing
            match ch {
                '(' | '[' | '{' => {
                    stack.push((ch, line, col));
                }
                ')' => match stack.pop() {
                    Some(('(', _, _)) => {}
                    Some((open, o_line, o_col)) => {
                        return Err((
                            line,
                            col,
                            format!(
                                "Mismatched delimiter ')', expected closing for '{}' from line {}, col {}",
                                open, o_line, o_col
                            ),
                        ));
                    }
                    None => {
                        return Err((line, col, "Unmatched closing delimiter ')'".to_string()));
                    }
                },
                ']' => match stack.pop() {
                    Some(('[', _, _)) => {}
                    Some((open, o_line, o_col)) => {
                        return Err((
                            line,
                            col,
                            format!(
                                "Mismatched delimiter ']', expected closing for '{}' from line {}, col {}",
                                open, o_line, o_col
                            ),
                        ));
                    }
                    None => {
                        return Err((line, col, "Unmatched closing delimiter ']'".to_string()));
                    }
                },
                '}' => match stack.pop() {
                    Some(('{', _, _)) => {}
                    Some((open, o_line, o_col)) => {
                        return Err((
                            line,
                            col,
                            format!(
                                "Mismatched delimiter '}}', expected closing for '{}' from line {}, col {}",
                                open, o_line, o_col
                            ),
                        ));
                    }
                    None => {
                        return Err((line, col, "Unmatched closing delimiter '}'".to_string()));
                    }
                },
                _ => {}
            }

            i += 1;
        }

        if let Some((unclosed, u_line, u_col)) = stack.pop() {
            return Err((
                u_line,
                u_col,
                format!(
                    "Unclosed delimiter '{}' starting at line {}, col {}",
                    unclosed, u_line, u_col
                ),
            ));
        }

        Ok(())
    }

    /// Extract language-specific imports and package statements
    fn extract_polyglot_imports(lines: &[&str], ext: &str, imports: &mut Vec<String>) {
        for line in lines {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            match ext {
                "java" | "kt" | "kts" | "scala" | "groovy" => {
                    if trimmed.starts_with("import ") || trimmed.starts_with("package ") {
                        imports.push(trimmed.trim_end_matches(';').to_string());
                    }
                }
                "cs" => {
                    if trimmed.starts_with("using ") || trimmed.starts_with("namespace ") {
                        imports.push(trimmed.trim_end_matches(';').to_string());
                    }
                }
                "php" => {
                    if trimmed.starts_with("use ")
                        || trimmed.starts_with("namespace ")
                        || trimmed.starts_with("require ")
                        || trimmed.starts_with("require_once ")
                        || trimmed.starts_with("include ")
                        || trimmed.starts_with("include_once ")
                    {
                        imports.push(trimmed.trim_end_matches(';').to_string());
                    }
                }
                "rb" => {
                    if trimmed.starts_with("require ")
                        || trimmed.starts_with("require_relative ")
                        || trimmed.starts_with("include ")
                        || trimmed.starts_with("extend ")
                    {
                        imports.push(trimmed.to_string());
                    }
                }
                "ex" | "exs" => {
                    if trimmed.starts_with("import ")
                        || trimmed.starts_with("alias ")
                        || trimmed.starts_with("use ")
                        || trimmed.starts_with("require ")
                    {
                        imports.push(trimmed.to_string());
                    }
                }
                "dart" => {
                    if trimmed.starts_with("import ")
                        || trimmed.starts_with("export ")
                        || trimmed.starts_with("part ")
                    {
                        imports.push(trimmed.trim_end_matches(';').to_string());
                    }
                }
                "zig" => {
                    if trimmed.contains("@import(") {
                        imports.push(trimmed.trim_end_matches(';').to_string());
                    }
                }
                "lua" => {
                    if trimmed.starts_with("require(")
                        || trimmed.starts_with("require \"")
                        || trimmed.starts_with("require '")
                    {
                        imports.push(trimmed.to_string());
                    }
                }
                "sh" | "bash" => {
                    if trimmed.starts_with("source ") || trimmed.starts_with(". ") {
                        imports.push(trimmed.to_string());
                    }
                }
                "proto" => {
                    if trimmed.starts_with("import ") || trimmed.starts_with("package ") {
                        imports.push(trimmed.trim_end_matches(';').to_string());
                    }
                }
                "graphql" | "gql" => {
                    if trimmed.starts_with("#import ") {
                        imports.push(trimmed.to_string());
                    }
                }
                "sol" => {
                    if trimmed.starts_with("import ") {
                        imports.push(trimmed.trim_end_matches(';').to_string());
                    }
                }
                _ => {
                    if trimmed.starts_with("import ")
                        || trimmed.starts_with("using ")
                        || trimmed.starts_with("include ")
                    {
                        imports.push(trimmed.trim_end_matches(';').to_string());
                    }
                }
            }
        }
    }

    /// Single-pass lexical & block scanner
    fn scan_symbols_and_blocks(
        lines: &[&str],
        ext: &str,
        comment_style: CommentStyle,
        symbols: &mut Vec<Symbol>,
    ) {
        let is_ruby = ext == "rb";
        let is_elixir = ext == "ex" || ext == "exs";
        let is_lua = ext == "lua";
        let is_sql = ext == "sql";

        let mut in_block_comment = false;
        let mut in_string = false;
        let mut string_quote = '"';

        let mut brace_depth = 0usize;
        let mut block_stack: Vec<PendingDecl> = Vec::new();
        // Ruby / Elixir / Lua generic block depth counter to track non-symbol blocks (if, while, etc.)
        let mut control_block_depth = 0usize;

        for (idx, line) in lines.iter().enumerate() {
            let line_num = idx + 1;
            let trimmed = line.trim();

            if trimmed.is_empty() {
                continue;
            }

            // Strip strings and comments for lexical block analysis
            let mut code_buf = String::with_capacity(line.len());
            let chars: Vec<char> = line.chars().collect();
            let len = chars.len();
            let mut i = 0;
            let mut escaped = false;

            while i < len {
                let ch = chars[i];

                if in_block_comment {
                    if ext == "lua" {
                        if ch == ']' && i + 1 < len && chars[i + 1] == ']' {
                            in_block_comment = false;
                            i += 2;
                            continue;
                        }
                    } else if ch == '*' && i + 1 < len && chars[i + 1] == '/' {
                        in_block_comment = false;
                        i += 2;
                        continue;
                    }
                    i += 1;
                    continue;
                }

                if in_string {
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == string_quote {
                        in_string = false;
                    }
                    i += 1;
                    continue;
                }

                // Check comment start
                match comment_style {
                    CommentStyle::Hash => {
                        if ch == '#' {
                            break; // rest of line is comment
                        }
                    }
                    CommentStyle::DashDash => {
                        if ch == '-' && i + 1 < len && chars[i + 1] == '-' {
                            if ext == "lua"
                                && i + 3 < len
                                && chars[i + 2] == '['
                                && chars[i + 3] == '['
                            {
                                in_block_comment = true;
                                i += 4;
                                continue;
                            }
                            break; // rest of line is comment
                        }
                    }
                    CommentStyle::SlashSlash => {
                        if ch == '/' && i + 1 < len {
                            if chars[i + 1] == '/' {
                                break; // rest of line is comment
                            } else if chars[i + 1] == '*' {
                                in_block_comment = true;
                                i += 2;
                                continue;
                            }
                        }
                    }
                }

                // Check string start
                if ch == '"' || ch == '\'' || ch == '`' {
                    if (ch == '"' || ch == '\'')
                        && i + 2 < len
                        && chars[i + 1] == ch
                        && chars[i + 2] == ch
                    {
                        in_string = true;
                        string_quote = ch;
                        escaped = false;
                        i += 3;
                        continue;
                    }
                    in_string = true;
                    string_quote = ch;
                    escaped = false;
                    i += 1;
                    continue;
                }

                code_buf.push(ch);
                i += 1;
            }

            let code_clean = code_buf.trim();

            // Check if current line declares a symbol
            let doc_comment = Self::find_preceding_doc_comment(lines, idx, comment_style);
            let maybe_decl =
                Self::detect_declaration(trimmed, code_clean, line_num, ext, doc_comment);

            if let Some(decl) = maybe_decl {
                if is_sql {
                    // For SQL tables or procedures
                    if code_clean.ends_with(';') {
                        // Single-line or closed statement
                        symbols.push(Symbol {
                            name: decl.name,
                            kind: decl.kind,
                            start_line: decl.start_line,
                            end_line: line_num,
                            signature: decl.signature,
                            doc_comment: decl.doc_comment,
                        });
                    } else {
                        block_stack.push(decl);
                    }
                } else if is_ruby || is_elixir || is_lua {
                    // Check if one-liner
                    if Self::is_single_line_ruby_or_elixir(code_clean) {
                        symbols.push(Symbol {
                            name: decl.name,
                            kind: decl.kind,
                            start_line: decl.start_line,
                            end_line: line_num,
                            signature: decl.signature,
                            doc_comment: decl.doc_comment,
                        });
                    } else {
                        block_stack.push(decl);
                    }
                } else {
                    // Brace-delimited languages:
                    // Check if it's an abstract method or interface method ending with ';'
                    let has_open_brace = code_clean.contains('{');
                    let ends_with_semicolon = code_clean.ends_with(';');

                    if ends_with_semicolon && !has_open_brace {
                        // Interface method or abstract method declaration
                        symbols.push(Symbol {
                            name: decl.name,
                            kind: decl.kind,
                            start_line: decl.start_line,
                            end_line: line_num,
                            signature: decl.signature,
                            doc_comment: decl.doc_comment,
                        });
                    } else {
                        block_stack.push(decl);
                    }
                }
            }

            // Track block closures and nesting
            if is_ruby {
                Self::track_ruby_blocks(
                    code_clean,
                    line_num,
                    &mut block_stack,
                    &mut control_block_depth,
                    symbols,
                );
            } else if is_elixir {
                Self::track_elixir_blocks(
                    code_clean,
                    line_num,
                    &mut block_stack,
                    &mut control_block_depth,
                    symbols,
                );
            } else if is_lua {
                Self::track_lua_blocks(
                    code_clean,
                    line_num,
                    &mut block_stack,
                    &mut control_block_depth,
                    symbols,
                );
            } else if is_sql {
                Self::track_sql_blocks(code_clean, line_num, &mut block_stack, symbols);
            } else {
                // Brace-delimited languages
                for ch in code_clean.chars() {
                    if ch == '{' {
                        brace_depth += 1;
                        // If top pending decl hasn't recorded its brace_depth_at_open, set it now
                        if let Some(top) = block_stack.last_mut()
                            && top.brace_depth_at_open == 0
                        {
                            top.brace_depth_at_open = brace_depth;
                        }
                    } else if ch == '}' {
                        let should_close = block_stack.last().is_some_and(|top| {
                            top.brace_depth_at_open == brace_depth
                                || (top.brace_depth_at_open == 0 && brace_depth <= 1)
                        });
                        if should_close && let Some(closed) = block_stack.pop() {
                            symbols.push(Symbol {
                                name: closed.name,
                                kind: closed.kind,
                                start_line: closed.start_line,
                                end_line: line_num,
                                signature: closed.signature,
                                doc_comment: closed.doc_comment,
                            });
                        }
                        brace_depth = brace_depth.saturating_sub(1);
                    }
                }
            }
        }

        // Close any remaining unclosed declarations gracefully at end of file
        let eof_line = lines.len().max(1);
        while let Some(decl) = block_stack.pop() {
            symbols.push(Symbol {
                name: decl.name,
                kind: decl.kind,
                start_line: decl.start_line,
                end_line: eof_line,
                signature: decl.signature,
                doc_comment: decl.doc_comment,
            });
        }
    }

    /// Check for single-line Ruby/Elixir constructs like `def foo; 42; end` or `def foo, do: bar`
    fn is_single_line_ruby_or_elixir(code: &str) -> bool {
        (code.contains("def ") && code.contains(" end"))
            || (code.contains("def ") && code.contains(", do:"))
            || (code.contains("defp ") && code.contains(", do:"))
    }

    /// Track Ruby `end` keywords matching `def`, `class`, `module`, or control flow
    fn track_ruby_blocks(
        code: &str,
        line_num: usize,
        stack: &mut Vec<PendingDecl>,
        control_depth: &mut usize,
        symbols: &mut Vec<Symbol>,
    ) {
        let words: Vec<&str> = code.split_whitespace().collect();
        for &w in &words {
            match w {
                "do" | "begin" | "if" | "unless" | "case" | "while" | "until" | "for" => {
                    *control_depth += 1;
                }
                "end" => {
                    if *control_depth > 0 {
                        *control_depth -= 1;
                    } else if let Some(decl) = stack.pop() {
                        symbols.push(Symbol {
                            name: decl.name,
                            kind: decl.kind,
                            start_line: decl.start_line,
                            end_line: line_num,
                            signature: decl.signature,
                            doc_comment: decl.doc_comment,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    /// Track Elixir `end` keywords
    fn track_elixir_blocks(
        code: &str,
        line_num: usize,
        stack: &mut Vec<PendingDecl>,
        control_depth: &mut usize,
        symbols: &mut Vec<Symbol>,
    ) {
        let words: Vec<&str> = code.split_whitespace().collect();
        for &w in &words {
            match w {
                "do" | "case" | "cond" | "receive" | "try" => {
                    *control_depth += 1;
                }
                "end" => {
                    if *control_depth > 0 {
                        *control_depth -= 1;
                    } else if let Some(decl) = stack.pop() {
                        symbols.push(Symbol {
                            name: decl.name,
                            kind: decl.kind,
                            start_line: decl.start_line,
                            end_line: line_num,
                            signature: decl.signature,
                            doc_comment: decl.doc_comment,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    /// Track Lua `end` or `until` keywords
    fn track_lua_blocks(
        code: &str,
        line_num: usize,
        stack: &mut Vec<PendingDecl>,
        control_depth: &mut usize,
        symbols: &mut Vec<Symbol>,
    ) {
        let words: Vec<&str> = code.split_whitespace().collect();
        for &w in &words {
            match w {
                "then" | "do" | "repeat" => {
                    *control_depth += 1;
                }
                "end" | "until" => {
                    if *control_depth > 0 {
                        *control_depth -= 1;
                    } else if let Some(decl) = stack.pop() {
                        symbols.push(Symbol {
                            name: decl.name,
                            kind: decl.kind,
                            start_line: decl.start_line,
                            end_line: line_num,
                            signature: decl.signature,
                            doc_comment: decl.doc_comment,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    /// Track SQL statements ending with `;` or `END;`
    fn track_sql_blocks(
        code: &str,
        line_num: usize,
        stack: &mut Vec<PendingDecl>,
        symbols: &mut Vec<Symbol>,
    ) {
        let upper = code.to_uppercase();
        if let Some(top) = stack.last() {
            let is_closed = match top.block_type {
                BlockType::SqlRoutine => {
                    if top.signature.contains("$$") {
                        upper.contains("$$ LANGUAGE")
                            || upper.ends_with("$$;")
                            || (code.contains("$$") && code.ends_with(';'))
                    } else {
                        upper.contains("END;") || upper == "END" || code.eq_ignore_ascii_case("go")
                    }
                }
                _ => code.ends_with(';') || code.eq_ignore_ascii_case("go"),
            };

            if is_closed {
                let decl = stack.pop().unwrap();
                symbols.push(Symbol {
                    name: decl.name,
                    kind: decl.kind,
                    start_line: decl.start_line,
                    end_line: line_num,
                    signature: decl.signature,
                    doc_comment: decl.doc_comment,
                });
            }
        }
    }

    /// Detect if a line starts a symbol declaration across 25+ polyglot languages
    fn detect_declaration(
        raw_line: &str,
        code_clean: &str,
        line_num: usize,
        ext: &str,
        doc_comment: Option<String>,
    ) -> Option<PendingDecl> {
        let sig = raw_line.trim().to_string();
        let tokens: Vec<&str> = code_clean
            .split(|c: char| c.is_whitespace() || c == '(' || c == '<' || c == '{' || c == ':')
            .filter(|s| !s.is_empty())
            .collect();

        if tokens.is_empty() {
            return None;
        }

        // Language-specific detection
        match ext {
            "sql" => {
                let upper = code_clean.to_uppercase();
                if upper.starts_with("CREATE TABLE") || upper.starts_with("CREATE OR REPLACE TABLE")
                {
                    let name = extract_after_keyword(&upper, &tokens, "TABLE")?;
                    return Some(PendingDecl {
                        name: clean_ident(name),
                        kind: SymbolKind::Struct, // Table represented as Struct
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::SqlTable,
                        brace_depth_at_open: 0,
                    });
                }
                if upper.starts_with("CREATE VIEW") || upper.starts_with("CREATE OR REPLACE VIEW") {
                    let name = extract_after_keyword(&upper, &tokens, "VIEW")?;
                    return Some(PendingDecl {
                        name: clean_ident(name),
                        kind: SymbolKind::Interface,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::SqlTable,
                        brace_depth_at_open: 0,
                    });
                }
                if upper.starts_with("CREATE PROCEDURE")
                    || upper.starts_with("CREATE OR REPLACE PROCEDURE")
                {
                    let name = extract_after_keyword(&upper, &tokens, "PROCEDURE")?;
                    return Some(PendingDecl {
                        name: clean_ident(name),
                        kind: SymbolKind::Function,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::SqlRoutine,
                        brace_depth_at_open: 0,
                    });
                }
                if upper.starts_with("CREATE FUNCTION")
                    || upper.starts_with("CREATE OR REPLACE FUNCTION")
                {
                    let name = extract_after_keyword(&upper, &tokens, "FUNCTION")?;
                    return Some(PendingDecl {
                        name: clean_ident(name),
                        kind: SymbolKind::Function,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::SqlRoutine,
                        brace_depth_at_open: 0,
                    });
                }
                return None;
            }

            "rb" => {
                if tokens[0] == "class" && tokens.len() >= 2 {
                    return Some(PendingDecl {
                        name: clean_ident(tokens[1]),
                        kind: SymbolKind::Class,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::KeywordRuby,
                        brace_depth_at_open: 0,
                    });
                }
                if tokens[0] == "module" && tokens.len() >= 2 {
                    return Some(PendingDecl {
                        name: clean_ident(tokens[1]),
                        kind: SymbolKind::Class,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::KeywordRuby,
                        brace_depth_at_open: 0,
                    });
                }
                if tokens[0] == "def" && tokens.len() >= 2 {
                    let raw_name = tokens[1].trim_start_matches("self.");
                    return Some(PendingDecl {
                        name: clean_ident(raw_name),
                        kind: SymbolKind::Method,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::KeywordRuby,
                        brace_depth_at_open: 0,
                    });
                }
                return None;
            }

            "ex" | "exs" => {
                if (tokens[0] == "defmodule"
                    || tokens[0] == "defprotocol"
                    || tokens[0] == "defimpl")
                    && tokens.len() >= 2
                {
                    return Some(PendingDecl {
                        name: clean_ident(tokens[1]),
                        kind: SymbolKind::Class,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::KeywordElixir,
                        brace_depth_at_open: 0,
                    });
                }
                if (tokens[0] == "def" || tokens[0] == "defp" || tokens[0] == "defmacro")
                    && tokens.len() >= 2
                {
                    return Some(PendingDecl {
                        name: clean_ident(tokens[1]),
                        kind: SymbolKind::Function,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::KeywordElixir,
                        brace_depth_at_open: 0,
                    });
                }
                return None;
            }

            "lua" => {
                if tokens[0] == "function" && tokens.len() >= 2 {
                    let fn_name = tokens[1].split(['.', ':']).next_back().unwrap_or(tokens[1]);
                    return Some(PendingDecl {
                        name: clean_ident(fn_name),
                        kind: SymbolKind::Function,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::KeywordLua,
                        brace_depth_at_open: 0,
                    });
                }
                if tokens.len() >= 3 && tokens[0] == "local" && tokens[1] == "function" {
                    return Some(PendingDecl {
                        name: clean_ident(tokens[2]),
                        kind: SymbolKind::Function,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::KeywordLua,
                        brace_depth_at_open: 0,
                    });
                }
                return None;
            }

            "sh" | "bash" => {
                if (tokens[0] == "function" && tokens.len() >= 2)
                    || (tokens.len() >= 2 && code_clean.contains("()"))
                {
                    let name = if tokens[0] == "function" {
                        tokens[1]
                    } else {
                        tokens[0]
                    };
                    if !["if", "for", "while", "case"].contains(&name) {
                        return Some(PendingDecl {
                            name: clean_ident(name),
                            kind: SymbolKind::Function,
                            start_line: line_num,
                            signature: sig,
                            doc_comment,
                            block_type: BlockType::Brace,
                            brace_depth_at_open: 0,
                        });
                    }
                }
                return None;
            }

            "proto" => {
                if (tokens[0] == "message" || tokens[0] == "enum" || tokens[0] == "service")
                    && tokens.len() >= 2
                {
                    let kind = match tokens[0] {
                        "message" => SymbolKind::Struct,
                        "enum" => SymbolKind::Enum,
                        _ => SymbolKind::Interface,
                    };
                    return Some(PendingDecl {
                        name: clean_ident(tokens[1]),
                        kind,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::Brace,
                        brace_depth_at_open: 0,
                    });
                }
                if tokens[0] == "rpc" && tokens.len() >= 2 {
                    return Some(PendingDecl {
                        name: clean_ident(tokens[1]),
                        kind: SymbolKind::Method,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::Brace,
                        brace_depth_at_open: 0,
                    });
                }
                return None;
            }

            "graphql" | "gql" => {
                if (tokens[0] == "type"
                    || tokens[0] == "interface"
                    || tokens[0] == "input"
                    || tokens[0] == "enum"
                    || tokens[0] == "union")
                    && tokens.len() >= 2
                {
                    let kind = match tokens[0] {
                        "interface" => SymbolKind::Interface,
                        "enum" => SymbolKind::Enum,
                        "type" | "input" => SymbolKind::Struct,
                        _ => SymbolKind::TypeAlias,
                    };
                    return Some(PendingDecl {
                        name: clean_ident(tokens[1]),
                        kind,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::Brace,
                        brace_depth_at_open: 0,
                    });
                }
                return None;
            }

            "tf" | "hcl" => {
                if (tokens[0] == "resource" || tokens[0] == "data") && tokens.len() >= 3 {
                    let label = format!("{}.{}", clean_ident(tokens[1]), clean_ident(tokens[2]));
                    return Some(PendingDecl {
                        name: label,
                        kind: SymbolKind::Struct,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::Brace,
                        brace_depth_at_open: 0,
                    });
                }
                if (tokens[0] == "module" || tokens[0] == "variable" || tokens[0] == "output")
                    && tokens.len() >= 2
                {
                    return Some(PendingDecl {
                        name: clean_ident(tokens[1]),
                        kind: SymbolKind::Function,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::Brace,
                        brace_depth_at_open: 0,
                    });
                }
                return None;
            }

            _ => {}
        }

        // Generic OOP and modern typed languages:
        // Java, C#, Scala, Kotlin, Swift, Dart, PHP, Zig, Groovy, Solidity
        let keywords_type: HashSet<&str> = [
            "class",
            "interface",
            "struct",
            "enum",
            "trait",
            "record",
            "contract",
            "mixin",
            "extension",
        ]
        .into_iter()
        .collect();

        // 1. Look for type declarations
        for (i, &tok) in tokens.iter().enumerate() {
            if keywords_type.contains(tok) && i + 1 < tokens.len() {
                let name = tokens[i + 1];
                if is_valid_ident(name) {
                    let kind = match tok {
                        "interface" => SymbolKind::Interface,
                        "enum" => SymbolKind::Enum,
                        "struct" => SymbolKind::Struct,
                        "trait" => SymbolKind::Trait,
                        _ => SymbolKind::Class,
                    };
                    return Some(PendingDecl {
                        name: clean_ident(name),
                        kind,
                        start_line: line_num,
                        signature: sig,
                        doc_comment,
                        block_type: BlockType::Brace,
                        brace_depth_at_open: 0,
                    });
                }
            }
        }

        // 2. Zig functions: `pub fn name(...)` or `fn name(...)`
        if ext == "zig" {
            for (i, &tok) in tokens.iter().enumerate() {
                if tok == "fn" && i + 1 < tokens.len() {
                    let name = tokens[i + 1];
                    if is_valid_ident(name) {
                        return Some(PendingDecl {
                            name: clean_ident(name),
                            kind: SymbolKind::Function,
                            start_line: line_num,
                            signature: sig,
                            doc_comment,
                            block_type: BlockType::Brace,
                            brace_depth_at_open: 0,
                        });
                    }
                }
            }
        }

        // 3. Kotlin functions: `fun name(...)`
        if ext == "kt" || ext == "kts" {
            for (i, &tok) in tokens.iter().enumerate() {
                if tok == "fun" && i + 1 < tokens.len() {
                    let name = tokens[i + 1];
                    if is_valid_ident(name) {
                        return Some(PendingDecl {
                            name: clean_ident(name),
                            kind: SymbolKind::Method,
                            start_line: line_num,
                            signature: sig,
                            doc_comment,
                            block_type: BlockType::Brace,
                            brace_depth_at_open: 0,
                        });
                    }
                }
            }
        }

        // 4. Swift functions: `func name(...)`
        if ext == "swift" {
            for (i, &tok) in tokens.iter().enumerate() {
                if tok == "func" && i + 1 < tokens.len() {
                    let name = tokens[i + 1];
                    if is_valid_ident(name) {
                        return Some(PendingDecl {
                            name: clean_ident(name),
                            kind: SymbolKind::Method,
                            start_line: line_num,
                            signature: sig,
                            doc_comment,
                            block_type: BlockType::Brace,
                            brace_depth_at_open: 0,
                        });
                    }
                }
            }
        }

        // 5. Scala functions: `def name(...)`
        if ext == "scala" {
            for (i, &tok) in tokens.iter().enumerate() {
                if tok == "def" && i + 1 < tokens.len() {
                    let name = tokens[i + 1];
                    if is_valid_ident(name) {
                        return Some(PendingDecl {
                            name: clean_ident(name),
                            kind: SymbolKind::Method,
                            start_line: line_num,
                            signature: sig,
                            doc_comment,
                            block_type: BlockType::Brace,
                            brace_depth_at_open: 0,
                        });
                    }
                }
            }
        }

        // 6. PHP functions: `function name(...)`
        if ext == "php" {
            for (i, &tok) in tokens.iter().enumerate() {
                if tok == "function" && i + 1 < tokens.len() {
                    let name = tokens[i + 1];
                    if is_valid_ident(name) {
                        return Some(PendingDecl {
                            name: clean_ident(name),
                            kind: SymbolKind::Method,
                            start_line: line_num,
                            signature: sig,
                            doc_comment,
                            block_type: BlockType::Brace,
                            brace_depth_at_open: 0,
                        });
                    }
                }
            }
        }

        // 7. General OOP method detection:
        // Must have '(' in the line, and must not be a control flow keyword
        if code_clean.contains('(') {
            let paren_idx = code_clean.find('(').unwrap();
            let before_paren = &code_clean[..paren_idx].trim();
            let sub_tokens: Vec<&str> = before_paren.split_whitespace().collect();

            if let Some(&last_tok) = sub_tokens.last() {
                let clean_name = clean_ident(last_tok);
                let control_words = [
                    "if",
                    "for",
                    "while",
                    "switch",
                    "catch",
                    "synchronized",
                    "sizeof",
                    "return",
                    "throw",
                    "new",
                    "super",
                    "this",
                    "lock",
                    "using",
                    "assert",
                ];

                if is_valid_ident(&clean_name) && !control_words.contains(&clean_name.as_str()) {
                    let modifiers = [
                        "public",
                        "private",
                        "protected",
                        "internal",
                        "static",
                        "final",
                        "abstract",
                        "virtual",
                        "override",
                        "async",
                        "synchronized",
                        "native",
                        "default",
                        "export",
                        "pub",
                        "fun",
                        "func",
                        "def",
                        "function",
                    ];
                    let has_modifier = sub_tokens.iter().any(|t| modifiers.contains(t));

                    // If it has a modifier or at least 2 tokens before paren (ReturnType Name)
                    if has_modifier || sub_tokens.len() >= 2 {
                        return Some(PendingDecl {
                            name: clean_name,
                            kind: SymbolKind::Method,
                            start_line: line_num,
                            signature: sig,
                            doc_comment,
                            block_type: BlockType::Brace,
                            brace_depth_at_open: 0,
                        });
                    }
                }
            }
        }

        None
    }

    /// Extract doc comments preceding a declaration line
    fn find_preceding_doc_comment(
        lines: &[&str],
        start_idx: usize,
        style: CommentStyle,
    ) -> Option<String> {
        if start_idx == 0 {
            return None;
        }

        let mut doc_lines = Vec::new();
        let mut curr = start_idx - 1;

        loop {
            let line = lines[curr].trim();
            if line.is_empty() {
                break;
            }

            match style {
                CommentStyle::SlashSlash => {
                    if line.starts_with("///") || line.starts_with("//") {
                        doc_lines.push(line.trim_start_matches('/').trim());
                    } else if line.ends_with("*/") {
                        // Multi-line block comment
                        doc_lines.push(
                            line.trim_end_matches("*/")
                                .trim_start_matches("/*")
                                .trim_start_matches('*')
                                .trim(),
                        );
                        while curr > 0 {
                            curr -= 1;
                            let prev = lines[curr].trim();
                            if prev.starts_with("/*") {
                                doc_lines.push(
                                    prev.trim_start_matches("/*").trim_start_matches('*').trim(),
                                );
                                break;
                            } else {
                                doc_lines.push(prev.trim_start_matches('*').trim());
                            }
                        }
                        break;
                    } else {
                        break;
                    }
                }
                CommentStyle::Hash => {
                    if line.starts_with('#') {
                        doc_lines.push(line.trim_start_matches('#').trim());
                    } else {
                        break;
                    }
                }
                CommentStyle::DashDash => {
                    if line.starts_with("--") {
                        doc_lines.push(line.trim_start_matches('-').trim());
                    } else {
                        break;
                    }
                }
            }

            if curr == 0 {
                break;
            }
            curr -= 1;
        }

        if doc_lines.is_empty() {
            None
        } else {
            doc_lines.reverse();
            let joined = doc_lines.join(" ").trim().to_string();
            if joined.is_empty() {
                None
            } else {
                Some(joined)
            }
        }
    }
}

fn extract_after_keyword<'a>(_upper: &str, tokens: &[&'a str], kw: &str) -> Option<&'a str> {
    for (i, &tok) in tokens.iter().enumerate() {
        if tok.eq_ignore_ascii_case(kw) && i + 1 < tokens.len() {
            // Check if next token is IF NOT EXISTS
            if tokens[i + 1].eq_ignore_ascii_case("IF") && i + 4 < tokens.len() {
                return Some(tokens[i + 4]);
            }
            return Some(tokens[i + 1]);
        }
    }
    None
}

fn clean_ident(s: &str) -> String {
    s.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '$')
        .split('.')
        .next_back()
        .unwrap_or(s)
        .to_string()
}

fn is_valid_ident(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let first = s.chars().next().unwrap();
    (first.is_alphabetic() || first == '_' || first == '$')
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_java_parsing_with_comments_and_strings() {
        let java_code = r#"package org.apache.hadoop.hdfs;

import java.io.IOException;
import org.apache.hadoop.conf.Configuration;

/**
 * Router RPC Client documentation.
 */
public class RouterRpcClient {
    private final Configuration conf;

    public RouterRpcClient(Configuration conf) {
        this.conf = conf;
        String fake = "Brace inside string { } not a block"; // comment { }
    }

    public Object invokeMethod(String method) throws IOException {
        /* block comment { } */
        return null;
    }
}
"#;

        let (symbols, imports) = UniversalParser::parse(java_code, "java");
        assert_eq!(imports.len(), 3);
        assert!(imports.contains(&"package org.apache.hadoop.hdfs".to_string()));
        assert!(imports.contains(&"import java.io.IOException".to_string()));

        let class_sym = symbols
            .iter()
            .find(|s| s.name == "RouterRpcClient" && s.kind == SymbolKind::Class);
        assert!(
            class_sym.is_some(),
            "RouterRpcClient class symbol must be found"
        );
        let class_sym = class_sym.unwrap();
        assert_eq!(class_sym.start_line, 9);
        assert_eq!(class_sym.end_line, 21);
        assert!(class_sym.doc_comment.is_some());

        let invoke_sym = symbols.iter().find(|s| s.name == "invokeMethod");
        assert!(invoke_sym.is_some(), "invokeMethod symbol must be found");
        let invoke_sym = invoke_sym.unwrap();
        assert_eq!(invoke_sym.start_line, 17);
        assert_eq!(invoke_sym.end_line, 20);
    }

    #[test]
    fn test_ruby_and_sql_parsing() {
        let ruby_code = r#"class OrderProcessor
  def process(order_id)
    if order_id > 0
      puts "valid"
    end
  end
end
"#;
        let (symbols, _) = UniversalParser::parse(ruby_code, "rb");
        let class_sym = symbols.iter().find(|s| s.name == "OrderProcessor").unwrap();
        assert_eq!(class_sym.start_line, 1);
        assert_eq!(class_sym.end_line, 7);

        let method_sym = symbols.iter().find(|s| s.name == "process").unwrap();
        assert_eq!(method_sym.start_line, 2);
        assert_eq!(method_sym.end_line, 6);

        let sql_code = r#"CREATE TABLE users (
    id INT PRIMARY KEY,
    name VARCHAR(255)
);

CREATE OR REPLACE PROCEDURE update_user(user_id INT) AS $$
BEGIN
    UPDATE users SET name = 'test';
END;
$$ LANGUAGE plpgsql;
"#;
        let (sql_symbols, _) = UniversalParser::parse(sql_code, "sql");
        let table_sym = sql_symbols.iter().find(|s| s.name == "users").unwrap();
        assert_eq!(table_sym.start_line, 1);
        assert_eq!(table_sym.end_line, 4);

        let proc_sym = sql_symbols
            .iter()
            .find(|s| s.name == "update_user")
            .unwrap();
        assert_eq!(proc_sym.start_line, 6);
        assert_eq!(proc_sym.end_line, 10);
    }
}
