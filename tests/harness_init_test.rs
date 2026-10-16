use layer::harness::{HarnessManager, HarnessType, LAYER_DIRECTIVE_HEADER};
use std::fs;

#[test]
fn test_harness_init_codex_creates_agents_md() {
    let temp_dir =
        std::env::temp_dir().join(format!("layer_test_codex_init_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let report = HarnessManager::init_workspace(&temp_dir, HarnessType::Codex, false);
    assert!(report.success);
    assert_eq!(report.files_created, vec!["AGENTS.md"]);
    assert!(report.files_updated.is_empty());

    let agents_content = fs::read_to_string(temp_dir.join("AGENTS.md")).unwrap();
    assert!(agents_content.contains(LAYER_DIRECTIVE_HEADER));
    assert!(agents_content.contains("bootstrap_project"));
    assert!(agents_content.contains("inspect_package_api"));
    assert!(agents_content.contains("Tool Permissions & Execution"));

    // Ensure Claude and Cursor files were NOT created
    assert!(!temp_dir.join("CLAUDE.md").exists());
    assert!(!temp_dir.join(".cursorrules").exists());

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_harness_init_claude_creates_claude_md() {
    let temp_dir =
        std::env::temp_dir().join(format!("layer_test_claude_init_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let report = HarnessManager::init_workspace(&temp_dir, HarnessType::Claude, false);
    assert!(report.success);
    assert_eq!(report.files_created, vec!["CLAUDE.md"]);

    let content = fs::read_to_string(temp_dir.join("CLAUDE.md")).unwrap();
    assert!(content.contains(LAYER_DIRECTIVE_HEADER));
    assert!(content.contains("Tool Permissions & Execution"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_harness_init_all_creates_all_steering_files_and_cursor_mcp() {
    let temp_dir = std::env::temp_dir().join(format!("layer_test_all_init_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let report = HarnessManager::init_workspace(&temp_dir, HarnessType::All, false);
    assert!(report.success);
    assert!(temp_dir.join("AGENTS.md").exists());
    assert!(temp_dir.join("CLAUDE.md").exists());
    assert!(temp_dir.join(".cursorrules").exists());
    assert!(temp_dir.join(".cursor/mcp.json").exists());

    let cursor_json = fs::read_to_string(temp_dir.join(".cursor/mcp.json")).unwrap();
    assert!(cursor_json.contains("mcpServers"));
    assert!(cursor_json.contains("layer"));
    assert!(cursor_json.contains("autoApprove"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_harness_init_idempotency() {
    let temp_dir =
        std::env::temp_dir().join(format!("layer_test_idempotent_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    // First run
    let rep1 = HarnessManager::init_workspace(&temp_dir, HarnessType::Codex, false);
    assert_eq!(rep1.files_created.len(), 1);

    // Second run
    let rep2 = HarnessManager::init_workspace(&temp_dir, HarnessType::Codex, false);
    assert!(rep2.files_created.is_empty());
    assert!(rep2.files_updated.is_empty());
    assert!(rep2.message.contains("already active and up-to-date"));

    let content = fs::read_to_string(temp_dir.join("AGENTS.md")).unwrap();
    assert_eq!(content.matches(LAYER_DIRECTIVE_HEADER).count(), 1);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_harness_init_appends_to_existing_file() {
    let temp_dir = std::env::temp_dir().join(format!("layer_test_append_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    // Create existing AGENTS.md with user instructions
    let existing_instructions = "# My Custom Project\nAlways run with node v22.\n";
    fs::write(temp_dir.join("AGENTS.md"), existing_instructions).unwrap();

    let report = HarnessManager::init_workspace(&temp_dir, HarnessType::Codex, false);
    assert!(report.files_created.is_empty());
    assert_eq!(report.files_updated, vec!["AGENTS.md"]);

    let merged_content = fs::read_to_string(temp_dir.join("AGENTS.md")).unwrap();
    assert!(merged_content.starts_with("# My Custom Project"));
    assert!(merged_content.contains(LAYER_DIRECTIVE_HEADER));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_generate_shell_aliases_contains_codex_and_claude() {
    let aliases = HarnessManager::generate_shell_aliases();
    assert!(aliases.contains("codex()"));
    assert!(aliases.contains("claude()"));
    assert!(aliases.contains("--layer"));
}
