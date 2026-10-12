use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Interface,
    Trait,
    Module,
    Constant,
    Variable,
    Unknown,
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SymbolKind::Function => write!(f, "fn"),
            SymbolKind::Method => write!(f, "method"),
            SymbolKind::Class => write!(f, "class"),
            SymbolKind::Struct => write!(f, "struct"),
            SymbolKind::Enum => write!(f, "enum"),
            SymbolKind::Interface => write!(f, "interface"),
            SymbolKind::Trait => write!(f, "trait"),
            SymbolKind::Module => write!(f, "mod"),
            SymbolKind::Constant => write!(f, "const"),
            SymbolKind::Variable => write!(f, "var"),
            SymbolKind::Unknown => write!(f, "symbol"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub start_line: usize, // 1-indexed
    pub end_line: usize,   // 1-indexed
    pub signature: String,
    pub doc_comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolSkeleton {
    pub name: String,
    pub kind: String,
    pub lines: [usize; 2], // [start, end]
    pub signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSkeleton {
    pub path: String,
    pub total_lines: usize,
    pub imports: Vec<String>,
    pub symbols: Vec<SymbolSkeleton>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolReference {
    pub file: String,
    pub line: usize,
    pub text: String,
    pub caller_symbol: Option<String>,
    pub is_approximate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallerInfo {
    pub caller_symbol: String,
    pub caller_file: String,
    pub caller_lines: [usize; 2],
    pub call_line: usize,
    pub call_snippet: String,
    pub is_approximate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolBodyResult {
    pub symbol_name: String,
    pub file: String,
    pub lines: [usize; 2],
    pub signature: String,
    pub body: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_truncated: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<String>, // Alternative file locations if ambiguous
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactRadius {
    pub target_symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    pub total_callers: usize,
    pub callers: Vec<CallerInfo>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub callers_truncated: bool,
    pub callers_confidence: String, // "high" (direct word match) or "approximate"
    pub callees: Vec<String>,       // Functions or methods called
    pub callees_confidence: String, // "approximate"
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<String>,    // If symbol has multiple declarations
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunk {
    pub id: String,
    pub file_path: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
    pub symbol_name: Option<String>,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    pub relative_path: PathBuf,
    pub absolute_path: PathBuf,
    pub extension: Option<String>,
    pub line_count: usize,
    pub size_bytes: u64,
    #[serde(default)]
    pub content_hash: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<std::time::SystemTime>,
    pub symbols: Vec<Symbol>,
    pub imports: Vec<String>,
    pub content: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub file_path: PathBuf,
    pub score: f32,
    pub symbol_name: Option<String>,
    pub start_line: usize,
    pub end_line: usize,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileOperation {
    Created,
    Modified,
    Deleted,
    Moved { from: PathBuf, to: PathBuf },
    Copied { from: PathBuf, to: PathBuf },
}

impl std::fmt::Display for FileOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileOperation::Created => write!(f, "CREATED"),
            FileOperation::Modified => write!(f, "MODIFIED"),
            FileOperation::Deleted => write!(f, "DELETED"),
            FileOperation::Moved { from, to } => write!(f, "MOVED ({} -> {})", from.display(), to.display()),
            FileOperation::Copied { from, to } => write!(f, "COPIED ({} -> {})", from.display(), to.display()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_line_start: usize,
    pub old_line_count: usize,
    pub new_line_start: usize,
    pub new_line_count: usize,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub id: usize,
    pub timestamp: String,
    pub file_path: PathBuf,
    pub operation: FileOperation,
    pub description: String,
    pub old_content: Option<String>,
    pub new_content: Option<String>,
    pub diff: Option<String>,
    pub affected_symbols: Vec<String>,
}
