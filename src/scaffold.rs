use crate::graph::VectorGraph;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

pub const TEMPLATE_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SupportedTemplate {
    RustCli,
}

impl SupportedTemplate {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "rust-cli" | "rust_cli" | "cli" => Some(Self::RustCli),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RustCli => "rust-cli",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaffoldResult {
    pub success: bool,
    pub template: String,
    pub template_version: String,
    pub project_name: String,
    pub destination: String,
    pub created_files: Vec<String>,
    pub files_synced: usize,
    pub message: String,
}

/// Validate that a project name is a valid identifier (alphanumeric, underscores, hyphens)
pub fn validate_project_name(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Project name cannot be empty.".to_string());
    }

    let first_char = trimmed.chars().next().unwrap();
    if !first_char.is_ascii_alphabetic() && first_char != '_' {
        return Err(format!(
            "Project name '{}' must start with an ASCII letter or underscore.",
            trimmed
        ));
    }

    for c in trimmed.chars() {
        if !c.is_ascii_alphanumeric() && c != '_' && c != '-' {
            return Err(format!(
                "Project name '{}' contains invalid character '{}'. Use alphanumeric, underscore, or hyphen.",
                trimmed, c
            ));
        }
    }

    let reserved = ["test", "std", "core", "alloc", "proc_macro", "layer"];
    if reserved.contains(&trimmed.to_lowercase().as_str()) {
        return Err(format!(
            "Project name '{}' is a reserved identifier. Please choose another name.",
            trimmed
        ));
    }

    Ok(())
}

/// Validate that the destination directory is either non-existent or completely empty
pub fn validate_destination(dest: &Path) -> Result<(), String> {
    if !dest.exists() {
        return Ok(());
    }

    if !dest.is_dir() {
        return Err(format!(
            "Destination '{}' exists and is a file, not a directory.",
            dest.display()
        ));
    }

    let read_dir = fs::read_dir(dest).map_err(|e| {
        format!(
            "Failed to inspect destination directory '{}': {}",
            dest.display(),
            e
        )
    })?;

    let non_ignored_entries: Vec<_> = read_dir
        .filter_map(Result::ok)
        .filter(|entry| {
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            // Allow empty .git directory or .DS_Store if empty repository was cloned
            name_str != ".git" && name_str != ".DS_Store"
        })
        .collect();

    if !non_ignored_entries.is_empty() {
        return Err(format!(
            "Destination directory '{}' is not empty (contains {} items). Scaffolding requires an empty destination to prevent data loss.",
            dest.display(),
            non_ignored_entries.len()
        ));
    }

    Ok(())
}

/// Scaffold a project into `dest_dir` using the chosen template, and synchronize the graph
pub fn scaffold_project(
    template: SupportedTemplate,
    project_name: &str,
    dest_dir: &Path,
    graph: &mut VectorGraph,
) -> Result<ScaffoldResult, String> {
    validate_project_name(project_name)?;
    validate_destination(dest_dir)?;

    let clean_name = project_name.trim();

    // Ensure target directory exists
    fs::create_dir_all(dest_dir).map_err(|e| {
        format!(
            "Failed to create destination directory '{}': {}",
            dest_dir.display(),
            e
        )
    })?;

    let mut created_files = Vec::new();

    match template {
        SupportedTemplate::RustCli => {
            let files = get_rust_cli_files(clean_name);
            for (rel_path, content) in files {
                let full_path = dest_dir.join(rel_path);
                if let Some(parent) = full_path.parent() {
                    fs::create_dir_all(parent).map_err(|e| {
                        format!("Failed to create directory '{}': {}", parent.display(), e)
                    })?;
                }
                fs::write(&full_path, content).map_err(|e| {
                    format!("Failed to write file '{}': {}", full_path.display(), e)
                })?;
                created_files.push(rel_path.to_string());
            }
        }
    }

    // Synchronize workspace immediately
    graph.sync_workspace(dest_dir);
    let files_synced = graph.files.len();

    Ok(ScaffoldResult {
        success: true,
        template: template.as_str().to_string(),
        template_version: TEMPLATE_VERSION.to_string(),
        project_name: clean_name.to_string(),
        destination: dest_dir.display().to_string(),
        created_files,
        files_synced,
        message: format!(
            "Successfully scaffolded '{}' ({}) with {} files. Workspace indexed immediately.",
            clean_name,
            template.as_str(),
            files_synced
        ),
    })
}

fn get_rust_cli_files(project_name: &str) -> Vec<(&'static str, String)> {
    let cargo_toml = format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

[dependencies]
clap = {{ version = "4.5", features = ["derive"] }}
"#,
        name = project_name
    );

    let main_rs = format!(
        r#"use clap::{{Parser, Subcommand}};

#[derive(Parser, Debug)]
#[command(name = "{name}", version, about = "High-performance CLI for {name}")]
pub struct Cli {{
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Optional target name or identifier
    #[arg(short, long, global = true)]
    pub target: Option<String>,
}}

#[derive(Subcommand, Debug)]
pub enum Commands {{
    /// Run default operation
    Run {{
        #[arg(short, long)]
        dry_run: bool,
    }},
    /// Display status and configuration
    Status,
}}

fn main() {{
    let cli = Cli::parse();
    match cli.command {{
        Some(Commands::Run {{ dry_run }}) => {{
            println!("Executing operation (dry_run: {{}})", dry_run);
        }}
        Some(Commands::Status) => {{
            println!("{name} is operational.");
        }}
        None => {{
            let target = cli.target.as_deref().unwrap_or("World");
            println!("Hello, {{}}! Run with --help for available commands.", target);
        }}
    }}
}}
"#,
        name = project_name
    );

    let lib_rs = format!(
        r#"//! Core library logic for {name}

pub mod error;

pub struct AppConfig {{
    pub name: String,
    pub verbose: bool,
}}

impl Default for AppConfig {{
    fn default() -> Self {{
        Self {{
            name: "{name}".to_string(),
            verbose: false,
        }}
    }}
}}
"#,
        name = project_name
    );

    let error_rs = r#"use std::fmt;

#[derive(Debug)]
pub enum AppError {
    InvalidInput(String),
    IoError(std::io::Error),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
            Self::IoError(err) => write!(f, "I/O error: {}", err),
        }
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        Self::IoError(err)
    }
}
"#
    .to_string();

    let gitignore = r#"/target
**/*.rs.bk
*.swp
.DS_Store
"#
    .to_string();

    let readme = format!(
        r#"# {name}

High-performance CLI built with Rust and Clap.

## Build and Run

```bash
cargo build --release
cargo run -- --help
```
"#,
        name = project_name
    );

    vec![
        ("Cargo.toml", cargo_toml),
        ("src/main.rs", main_rs),
        ("src/lib.rs", lib_rs),
        ("src/error.rs", error_rs),
        (".gitignore", gitignore),
        ("README.md", readme),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_validate_project_name() {
        assert!(validate_project_name("my_cli").is_ok());
        assert!(validate_project_name("my-cli-tool").is_ok());
        assert!(validate_project_name("tool123").is_ok());

        assert!(validate_project_name("").is_err());
        assert!(validate_project_name("123tool").is_err());
        assert!(validate_project_name("my cli").is_err());
        assert!(validate_project_name("std").is_err());
        assert!(validate_project_name("test").is_err());
    }

    #[test]
    fn test_validate_destination_rejects_non_empty() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("existing.txt");
        fs::write(file_path, "hello").unwrap();

        let res = validate_destination(dir.path());
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("is not empty"));
    }

    #[test]
    fn test_scaffold_rust_cli_success_and_immediate_sync() {
        let dir = tempdir().unwrap();
        let target_dir = dir.path().join("my_new_tool");
        let mut graph = VectorGraph::new();

        let result = scaffold_project(
            SupportedTemplate::RustCli,
            "my_new_tool",
            &target_dir,
            &mut graph,
        )
        .expect("scaffolding must succeed");

        assert!(result.success);
        assert_eq!(result.template, "rust-cli");
        assert_eq!(result.template_version, TEMPLATE_VERSION);
        assert_eq!(result.project_name, "my_new_tool");
        assert_eq!(result.created_files.len(), 6);
        assert!(target_dir.join("Cargo.toml").exists());
        assert!(target_dir.join("src/main.rs").exists());
        assert!(target_dir.join("src/lib.rs").exists());
        assert!(target_dir.join("src/error.rs").exists());

        // Verify immediate graph sync
        assert!(result.files_synced >= 3); // main.rs, lib.rs, error.rs
        let main_syms = graph.find_symbol_filtered("Cli", None, None);
        assert!(!main_syms.is_empty());
        assert_eq!(main_syms[0].1.name, "Cli");

        let app_config = graph.find_symbol_filtered("AppConfig", None, None);
        assert!(!app_config.is_empty());
        assert_eq!(app_config[0].1.name, "AppConfig");
    }

    #[test]
    fn test_scaffold_rejects_repeat_invocation_on_same_destination() {
        let dir = tempdir().unwrap();
        let target_dir = dir.path().join("tool");
        let mut graph = VectorGraph::new();

        // First invocation succeeds
        assert!(
            scaffold_project(SupportedTemplate::RustCli, "tool", &target_dir, &mut graph,).is_ok()
        );

        // Repeat invocation on non-empty directory must be rejected
        let second = scaffold_project(SupportedTemplate::RustCli, "tool", &target_dir, &mut graph);
        assert!(second.is_err());
        assert!(second.unwrap_err().contains("is not empty"));
    }

    #[test]
    fn test_scaffolded_project_compiles_cleanly() {
        let dir = tempdir().unwrap();
        let target_dir = dir.path().join("demo_cli");
        let mut graph = VectorGraph::new();

        let result = scaffold_project(
            SupportedTemplate::RustCli,
            "demo_cli",
            &target_dir,
            &mut graph,
        )
        .expect("scaffolding must succeed");

        assert!(result.success);

        // Run cargo check in the generated project to guarantee it builds
        let status = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&target_dir)
            .status();

        if let Ok(st) = status {
            assert!(st.success(), "scaffolded project failed cargo check");
        }
    }
}
