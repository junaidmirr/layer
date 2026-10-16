use layer::graph::VectorGraph;
use layer::mutation::{MutationManager, VerifyMode};
use layer::package_indexer::PackageIndexer;
use layer::telemetry::TelemetryTracker;
use layer::verifier::Verifier;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn test_rename_symbol_atomic_multi_file_and_comment_string_safety() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();

    // 1. Create types.ts
    let types_content = r#"export interface ParticleState {
    id: string;
    mass: number;
    velocity: [number, number, number];
}
"#;
    fs::write(root.join("types.ts"), types_content).unwrap();

    // 2. Create engine.ts containing code usage, plus an explicit string literal and comment
    let engine_content = r#"import { ParticleState } from './types';

// ParticleState comment must never be corrupted
const LABEL = "ParticleState"; // string literal must never be corrupted

export function stepSimulation(state: ParticleState, dt: number): ParticleState {
    return state;
}
"#;
    fs::write(root.join("engine.ts"), engine_content).unwrap();

    // 3. Create Canvas.tsx consuming ParticleState
    let canvas_content = r#"import React from 'react';
import { ParticleState } from './types';

export const ParticleCanvas: React.FC<{ particle: ParticleState }> = ({ particle }) => {
    return <div>{particle.id}</div>;
};
"#;
    fs::write(root.join("Canvas.tsx"), canvas_content).unwrap();

    // Index workspace into VectorGraph
    graph.sync_workspace(root);
    assert!(
        graph
            .files
            .contains_key(&std::path::PathBuf::from("types.ts"))
    );
    assert!(
        graph
            .files
            .contains_key(&std::path::PathBuf::from("engine.ts"))
    );
    assert!(
        graph
            .files
            .contains_key(&std::path::PathBuf::from("Canvas.tsx"))
    );

    // 4. Perform atomic multi-file rename: ParticleState -> OrbitalBodyState
    let mut pkg_indexer = PackageIndexer::new();
    let rename_res = MutationManager::rename_symbol(
        root,
        std::path::Path::new("types.ts"),
        "ParticleState",
        "OrbitalBodyState",
        VerifyMode::Fast,
        &mut graph,
        Some(&mut pkg_indexer),
    )
    .expect("rename_symbol must succeed");

    assert!(rename_res.success);
    assert_eq!(rename_res.files_modified.len(), 3);
    assert!(rename_res.total_occurrences_replaced >= 5);
    assert_eq!(rename_res.llm_turns_saved, 2);

    // Verify types.ts was updated
    let updated_types = fs::read_to_string(root.join("types.ts")).unwrap();
    assert!(updated_types.contains("export interface OrbitalBodyState"));
    assert!(!updated_types.contains("ParticleState"));

    // Verify engine.ts was updated in code, BUT comments and string literals remain intact!
    let updated_engine = fs::read_to_string(root.join("engine.ts")).unwrap();
    assert!(updated_engine.contains("import { OrbitalBodyState } from './types'"));
    assert!(updated_engine.contains("state: OrbitalBodyState"));
    assert!(updated_engine.contains("): OrbitalBodyState"));
    // Comment preserved exactly:
    assert!(updated_engine.contains("// ParticleState comment must never be corrupted"));
    // String literal preserved exactly:
    assert!(updated_engine.contains("const LABEL = \"ParticleState\";"));

    // Verify Canvas.tsx was updated
    let updated_canvas = fs::read_to_string(root.join("Canvas.tsx")).unwrap();
    assert!(updated_canvas.contains("import { OrbitalBodyState } from './types'"));
    assert!(updated_canvas.contains("particle: OrbitalBodyState"));

    // 5. Test Transactional Multi-File Rollback
    let rollback_res = MutationManager::rollback_last_edit(root, &mut graph)
        .expect("multi-file rollback must succeed");
    assert!(rollback_res.success);
    assert!(rollback_res.message.contains("multi-file operation"));

    // Verify all 3 files reverted cleanly
    let reverted_types = fs::read_to_string(root.join("types.ts")).unwrap();
    assert!(reverted_types.contains("export interface ParticleState"));
    assert!(!reverted_types.contains("OrbitalBodyState"));

    let reverted_engine = fs::read_to_string(root.join("engine.ts")).unwrap();
    assert!(reverted_engine.contains("import { ParticleState } from './types'"));

    let reverted_canvas = fs::read_to_string(root.join("Canvas.tsx")).unwrap();
    assert!(reverted_canvas.contains("import { ParticleState } from './types'"));
}

#[test]
fn test_verify_impacted_tests_selective_discovery() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();

    // 1. Create module files:
    // math.ts
    fs::write(
        root.join("math.ts"),
        "export function add(a: number, b: number): number { return a + b; }\n",
    )
    .unwrap();

    // physics.ts imports math.ts
    fs::write(root.join("physics.ts"), "import { add } from './math';\nexport function computeVelocity(v: number, dv: number): number { return add(v, dv); }\n").unwrap();

    // ui.ts is completely independent
    fs::write(
        root.join("ui.ts"),
        "export function renderButton(): string { return '<button/>'; }\n",
    )
    .unwrap();

    // physics.test.ts imports physics.ts
    fs::write(
        root.join("physics.test.ts"),
        "import { computeVelocity } from './physics';\n// test\n",
    )
    .unwrap();

    // ui.test.ts imports ui.ts
    fs::write(
        root.join("ui.test.ts"),
        "import { renderButton } from './ui';\n// test\n",
    )
    .unwrap();

    graph.sync_workspace(root);
    assert_eq!(graph.get_all_test_files().len(), 2);

    // 2. Change math.ts: physics.test.ts must be impacted, ui.test.ts must be skipped!
    let report = Verifier::verify_impacted_tests(root, Some(&["math.ts".to_string()]), &graph);

    assert!(report.passed);
    assert_eq!(report.impacted_test_files, vec!["physics.test.ts"]);
    assert_eq!(report.total_workspace_test_files, 2);
    assert_eq!(report.skipped_test_files_count, 1);
    assert!(report.message.contains("skipped 1 unaffected tests"));

    // 3. Change ui.ts: only ui.test.ts impacted, physics.test.ts skipped!
    let report_ui = Verifier::verify_impacted_tests(root, Some(&["ui.ts".to_string()]), &graph);
    assert_eq!(report_ui.impacted_test_files, vec!["ui.test.ts"]);
    assert_eq!(report_ui.skipped_test_files_count, 1);
}

#[test]
fn test_telemetry_roi_tracking_and_stats() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    let mut tracker = TelemetryTracker::load(root);
    assert_eq!(tracker.total_tool_calls, 0);

    // Simulate surgical reads, API inspection, AST syntax catch, and cross-file rename
    tracker.record_tool_call("find_symbol", 4, 800, 0);
    tracker.record_tool_call("get_symbol_body", 6, 1200, 0);
    tracker.record_tool_call("inspect_package_api", 15, 2500, 1);
    tracker.record_ast_catch();
    tracker.record_circuit_breaker_save();
    tracker.record_tool_call("rename_symbol", 25, 7000, 2);
    tracker.record_tool_call("verify_impacted_tests", 10, 2400, 1);

    assert_eq!(tracker.total_tool_calls, 5);
    assert_eq!(tracker.surgical_ast_reads, 2);
    assert_eq!(tracker.package_apis_inspected, 1);
    assert_eq!(tracker.cross_file_renames, 1);
    assert_eq!(tracker.selective_test_runs, 1);
    assert_eq!(tracker.ast_precheck_catches, 1);
    assert_eq!(tracker.circuit_breaker_saves, 1);
    assert_eq!(tracker.llm_turns_saved, 6);
    assert!(tracker.estimated_tokens_saved >= 20000);

    // Test persistence
    tracker.save(root).unwrap();
    assert!(root.join(".layer/telemetry.json").exists());

    let loaded = TelemetryTracker::load(root);
    assert_eq!(loaded, tracker);

    // Test dashboard formatting
    let dashboard = tracker.format_dashboard();
    assert!(dashboard.contains("LAYER v1.1 — ROI & SESSION TELEMETRY"));
    assert!(dashboard.contains("find_symbol"));
    assert!(dashboard.contains("inspect_package_api"));
    assert!(dashboard.contains("rename_symbol"));
    assert!(dashboard.contains("verify_impacted_tests"));
}

#[test]
fn test_invariant_1_top_down_coding_never_rolls_back_on_missing_imports() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();
    let mut pkg_indexer = PackageIndexer::new();

    // 1. Agent writes app/layout.tsx importing uncreated sibling ./globals.css and ./components/BulbScene
    let layout_content = r#"import React from 'react';
import './globals.css';
import { BulbScene } from './components/BulbScene';

export default function RootLayout({ children }: { children: React.ReactNode }) {
    return <html><body><BulbScene />{children}</body></html>;
}
"#;
    let res = MutationManager::write_file(
        root,
        std::path::Path::new("app/layout.tsx"),
        layout_content,
        VerifyMode::Fast,
        true, // auto_rollback_on_cascade = true
        &mut graph,
        Some(&mut pkg_indexer),
    )
    .expect("write_file must succeed");

    // Invariant 1 Assertions:
    // File was preserved on disk (rolled_back: false, success: true)
    assert!(res.success);
    assert!(root.join("app/layout.tsx").exists());
    assert!(res.circuit_breaker.is_none() || !res.circuit_breaker.unwrap().rolled_back);
    // Missing imports were extracted for top-down workflow
    assert!(!res.missing_imports_to_create_next.is_empty());
    assert!(
        res.missing_imports_to_create_next
            .contains(&"./globals.css".to_string())
    );
    assert!(
        res.missing_imports_to_create_next
            .contains(&"./components/BulbScene".to_string())
    );

    // 2. An edit introducing an actual Tree-sitter AST syntax error MUST still roll back!
    let syntax_err_content = r#"export function broken() { const x = ; return x; }"#;
    let err_res = MutationManager::write_file(
        root,
        std::path::Path::new("app/syntax_err.ts"),
        syntax_err_content,
        VerifyMode::Fast,
        true,
        &mut graph,
        Some(&mut pkg_indexer),
    )
    .expect("write_file returns mutation result");

    assert!(!err_res.success);
    assert!(err_res.circuit_breaker.is_some());
    let cb = err_res.circuit_breaker.unwrap();
    assert!(cb.rolled_back);
    assert!(!root.join("app/syntax_err.ts").exists());
}

#[test]
fn test_invariant_2_instant_native_scaffolding_nextjs_and_react_and_custom() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();

    // 1. Next.js instant native scaffolding (<10ms)
    let start_next = std::time::Instant::now();
    let next_res = layer::scaffold::bootstrap_project(
        layer::scaffold::GeneratorPreset::NextJs,
        "my_next_app",
        root,
        Some(vec!["lucide-react".to_string()]),
        &mut graph,
    )
    .expect("bootstrap_project nextjs must succeed");
    let next_elapsed = start_next.elapsed();

    assert!(next_res.success);
    assert!(next_elapsed.as_millis() < 3500); // instant generation
    let next_dir = root.join("my_next_app");
    assert!(next_dir.join("package.json").exists());
    assert!(next_dir.join("tsconfig.json").exists());
    assert!(next_dir.join("next.config.ts").exists());
    assert!(next_dir.join("postcss.config.mjs").exists());
    assert!(next_dir.join("app/globals.css").exists());
    assert!(next_dir.join("app/layout.tsx").exists());
    assert!(next_dir.join("app/page.tsx").exists());

    // Check dependency injection
    let pkg_json = fs::read_to_string(next_dir.join("package.json")).unwrap();
    assert!(pkg_json.contains("lucide-react"));
    assert!(pkg_json.contains("next"));
    assert!(pkg_json.contains("tailwindcss"));

    // 2. React-TS instant native scaffolding
    let react_res = layer::scaffold::bootstrap_project(
        layer::scaffold::GeneratorPreset::ReactTs,
        "my_react_app",
        root,
        None,
        &mut graph,
    )
    .expect("bootstrap_project react-ts must succeed");
    assert!(react_res.success);
    let react_dir = root.join("my_react_app");
    assert!(react_dir.join("package.json").exists());
    assert!(react_dir.join("vite.config.ts").exists());
    assert!(react_dir.join("src/App.tsx").exists());
    assert!(react_dir.join("src/main.tsx").exists());

    // 3. Custom / fallback preset
    let custom_preset = layer::scaffold::GeneratorPreset::parse_or_custom("svelte");
    let custom_res =
        layer::scaffold::bootstrap_project(custom_preset, "my_svelte_app", root, None, &mut graph)
            .expect("bootstrap_project custom preset must succeed");
    assert!(custom_res.success);
    let custom_dir = root.join("my_svelte_app");
    assert!(custom_dir.join("README.md").exists());
    assert!(custom_dir.join("package.json").exists());
}

#[test]
fn test_invariant_3_idempotent_path_normalizer() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("my-awesome-app");
    fs::create_dir_all(&root).unwrap();

    // 1. Path starting with redundant folder name "my-awesome-app/app/page.tsx"
    // when root is already ".../my-awesome-app"
    let resolved = layer::mcp::resolve_safe_path(&root, "my-awesome-app/app/page.tsx")
        .expect("must resolve safely");
    assert_eq!(resolved, std::path::PathBuf::from("app/page.tsx"));

    // 2. Redundant leading name with ./
    let resolved_dot = layer::mcp::resolve_safe_path(&root, "./my-awesome-app/src/index.ts")
        .expect("must resolve safely");
    assert_eq!(resolved_dot, std::path::PathBuf::from("src/index.ts"));

    // 3. Exact folder name "my-awesome-app" maps to "."
    let resolved_self =
        layer::mcp::resolve_safe_path(&root, "my-awesome-app").expect("must resolve safely");
    assert_eq!(resolved_self, std::path::PathBuf::from(""));

    // 4. Normal path without redundant prefix is untouched
    let resolved_normal =
        layer::mcp::resolve_safe_path(&root, "app/layout.tsx").expect("must resolve safely");
    assert_eq!(resolved_normal, std::path::PathBuf::from("app/layout.tsx"));
}

#[test]
fn test_invariant_4_graceful_package_inspection_and_honest_telemetry() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut pkg_indexer = PackageIndexer::new();

    // 1. Inspect Next.js fallbacks
    let next_summary = pkg_indexer
        .inspect_package(root, "next/link", Some("Link"), None)
        .expect("next/link must resolve from built-in fallbacks");
    assert_eq!(next_summary.package, "next/link");
    assert!(next_summary.requested_symbol.is_some());

    // 2. Inspect TailwindCSS fallback
    let tailwind_summary = pkg_indexer
        .inspect_package(root, "tailwindcss", Some("Config"), None)
        .expect("tailwindcss must resolve");
    assert_eq!(tailwind_summary.package, "tailwindcss");

    // 3. Inspect Framer Motion fallback
    let motion_summary = pkg_indexer
        .inspect_package(root, "framer-motion", Some("motion"), None)
        .expect("framer-motion must resolve");
    assert_eq!(motion_summary.package, "framer-motion");

    // 4. Inspect Zustand fallback
    let zustand_summary = pkg_indexer
        .inspect_package(root, "zustand", Some("create"), None)
        .expect("zustand must resolve");
    assert_eq!(zustand_summary.package, "zustand");

    // 5. Test MCP inspect_package_api with unindexed/offline package
    let mut graph = VectorGraph::new();
    let mut root_buf = root.to_path_buf();
    let inspect_args = serde_json::json!({
        "package_name": "some-obscure-uninstalled-pkg-xyz",
    });
    let result_json = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "inspect_package_api",
        Some(&inspect_args),
        &mut pkg_indexer,
    );

    // Must return structured JSON with "status": "unindexed_offline" instead of MCP error string
    assert!(!result_json.starts_with("Error:"));
    let parsed: serde_json::Value = serde_json::from_str(&result_json).unwrap();
    assert_eq!(parsed["status"], "unindexed_offline");
    assert_eq!(parsed["package"], "some-obscure-uninstalled-pkg-xyz");

    // 6. Test Honest Telemetry: unindexed_offline does NOT increment package_apis_inspected or credit tokens
    let mut tracker = TelemetryTracker::new();
    let initial_calls = tracker.total_tool_calls;
    let initial_apis = tracker.package_apis_inspected;
    let initial_tokens = tracker.estimated_tokens_saved;

    // Simulate resolved inspection:
    tracker.record_tool_call("inspect_package_api", 10, 2500, 1);
    assert_eq!(tracker.package_apis_inspected, initial_apis + 1);
    assert_eq!(tracker.estimated_tokens_saved, initial_tokens + 2500);

    // Simulate unindexed_offline inspection (0 tokens saved):
    tracker.record_tool_call("inspect_package_api", 5, 0, 0);
    assert_eq!(tracker.package_apis_inspected, initial_apis + 1); // Not incremented!
    assert_eq!(tracker.estimated_tokens_saved, initial_tokens + 2500); // Not credited!
    assert_eq!(tracker.total_tool_calls, initial_calls + 2);
}

#[test]
fn test_bootstrap_project_nested_folder_and_transport_resilience() {
    let dir = tempdir().unwrap();
    let container_root = dir.path();
    let mut graph = VectorGraph::new();
    let mut root_buf = container_root.to_path_buf();
    let mut pkg_indexer = PackageIndexer::new();

    // 1. Call bootstrap_project to create a nested subfolder 'light_circuit_studio'
    let bootstrap_args = serde_json::json!({
        "preset": "nextjs",
        "project_name": "light_circuit_studio",
        "destination": "."
    });

    let res_str = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "bootstrap_project",
        Some(&bootstrap_args),
        &mut pkg_indexer,
    );

    // Verify response is valid JSON and contains created files
    assert!(
        !res_str.starts_with("Error:"),
        "Bootstrap failed: {}",
        res_str
    );
    let parsed: serde_json::Value = serde_json::from_str(&res_str).unwrap();
    assert_eq!(parsed["success"], true);
    assert_eq!(parsed["project_name"], "light_circuit_studio");

    // Verify workspace root auto-switched to nested project
    let nested_dir = container_root.join("light_circuit_studio");
    assert_eq!(
        root_buf.canonicalize().unwrap(),
        nested_dir.canonicalize().unwrap(),
        "Workspace root must auto-switch to bootstrapped nested directory"
    );

    // Verify project files exist
    assert!(nested_dir.join("package.json").exists());
    assert!(nested_dir.join("tsconfig.json").exists());
    assert!(nested_dir.join("app/page.tsx").exists());

    // 2. Subsequent write_file tool call on the active workspace succeeds without panic or error
    let write_args = serde_json::json!({
        "path": "app/BulbScene.tsx",
        "content": "export function BulbScene() { return <div id=\"scene\">Bulb Active</div>; }",
        "verify_mode": "fast"
    });

    let write_res_str = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "write_file",
        Some(&write_args),
        &mut pkg_indexer,
    );
    assert!(
        !write_res_str.starts_with("Error:"),
        "Write failed: {}",
        write_res_str
    );
    assert!(nested_dir.join("app/BulbScene.tsx").exists());
}

#[test]
fn test_verify_project_auto_detects_nested_project_root() {
    let dir = tempdir().unwrap();
    let parent_dir = dir.path();

    // 1. Parent dir is empty with no build manifest
    assert_eq!(
        layer::verifier::ProjectType::detect(parent_dir),
        layer::verifier::ProjectType::Unknown
    );

    // 2. Create a nested project in a subfolder (e.g. 'bulb_circuit_sim')
    let sub_proj = parent_dir.join("bulb_circuit_sim");
    std::fs::create_dir_all(&sub_proj).unwrap();
    std::fs::write(
        sub_proj.join("package.json"),
        r#"{"name": "bulb_circuit_sim", "version": "1.0.0"}"#,
    )
    .unwrap();
    std::fs::write(
        sub_proj.join("tsconfig.json"),
        r#"{"compilerOptions": {"module": "esnext"}}"#,
    )
    .unwrap();

    // 3. Direct verification of parent_dir MUST auto-detect the nested project root
    let direct_report = layer::verifier::Verifier::verify_workspace(parent_dir, None);
    assert_ne!(
        direct_report.message, "No recognized build configuration found in workspace root.",
        "Should never return missing config error when nested manifest exists"
    );
    assert_eq!(direct_report.project_type, "typescript");

    // 4. MCP execute_tool('verify_project') from parent workspace auto-switches active root
    let mut graph = VectorGraph::new();
    let mut ws_root = parent_dir.to_path_buf();
    let mut pkg_indexer = PackageIndexer::new();

    let verify_args = serde_json::json!({});
    let mcp_res_str = layer::mcp::execute_tool(
        &mut graph,
        &mut ws_root,
        "verify_project",
        Some(&verify_args),
        &mut pkg_indexer,
    );

    assert!(!mcp_res_str.starts_with("Error:"));
    let report_json: serde_json::Value = serde_json::from_str(&mcp_res_str).unwrap();
    assert_eq!(report_json["project_type"], "typescript");
    assert_ne!(
        report_json["message"],
        "No recognized build configuration found in workspace root."
    );

    // Active workspace root was updated to the nested project
    assert_eq!(
        ws_root.canonicalize().unwrap(),
        sub_proj.canonicalize().unwrap()
    );
}

#[test]
fn test_mcp_panic_boundary_isolation() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();
    let mut root_buf = root.to_path_buf();
    let mut pkg_indexer = PackageIndexer::new();

    // Calling an operation with invalid / extreme data or unknown method does NOT panic the process
    let unknown_args = serde_json::json!({ "invalid_field": 12345 });
    let res = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "non_existent_tool_xyz",
        Some(&unknown_args),
        &mut pkg_indexer,
    );
    assert!(res.contains("Unknown tool"));
}

#[test]
fn test_python_pep701_fstring_and_modern_syntax_write_never_false_positive_rollback() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();
    let mut pkg_indexer = PackageIndexer::new();

    // Python 3.12 code using PEP 701 nested quotes inside f-strings and match-case
    let python_code = r#"import sys

def render_gui(config):
    match config.get("mode"):
        case "dark":
            theme = "theme-dark"
        case "light":
            theme = "theme-light"
        case _:
            theme = "theme-default"

    # PEP 701 nested quotes inside f-strings
    status_msg = f"Status: {config['title']} (brightness: {config['brightness']:.2f})"
    print(f"Rendered {theme}: {status_msg}")
    return status_msg

if __name__ == "__main__":
    cfg = {"mode": "dark", "title": "Circuit Lab", "brightness": 0.85}
    render_gui(cfg)
"#;

    let res = layer::mutation::MutationManager::write_file(
        root,
        Path::new("app.py"),
        python_code,
        layer::mutation::VerifyMode::Fast,
        true, // auto_rollback_on_cascade
        &mut graph,
        Some(&mut pkg_indexer),
    )
    .expect("write_file should succeed for valid modern Python 3.12 syntax");

    assert!(res.success, "Mutation must report success");
    assert!(
        !res.rolled_back(),
        "Valid Python 3.12 code must NEVER be rolled back due to Tree-sitter false positives"
    );
    assert!(root.join("app.py").exists(), "app.py must remain on disk");
    let on_disk = std::fs::read_to_string(root.join("app.py")).unwrap();
    assert_eq!(on_disk, python_code);
}

#[test]
fn test_no_subdirectory_hijacking_when_writing_to_parent_workspace() {
    let dir = tempdir().unwrap();
    let parent_dir = dir.path();

    // Create an old Vite subfolder containing package.json
    let old_vite_app = parent_dir.join("old_vite_app");
    std::fs::create_dir_all(&old_vite_app).unwrap();
    std::fs::write(
        old_vite_app.join("package.json"),
        r#"{"name": "old-vite-app", "version": "1.0.0"}"#,
    )
    .unwrap();

    let mut graph = VectorGraph::new();
    let mut ws_root = parent_dir.to_path_buf();
    let mut pkg_indexer = PackageIndexer::new();

    // Call write_file tool via MCP execute_tool
    let write_args = serde_json::json!({
        "path": "app.py",
        "content": "print('Hello from parent')\n"
    });
    let res = layer::mcp::execute_tool(
        &mut graph,
        &mut ws_root,
        "write_file",
        Some(&write_args),
        &mut pkg_indexer,
    );

    assert!(
        !res.starts_with("Error:"),
        "write_file should not error: {}",
        res
    );
    // ws_root must remain parent_dir, NOT hijacked into old_vite_app!
    assert_eq!(
        ws_root.canonicalize().unwrap(),
        parent_dir.canonicalize().unwrap(),
        "Workspace root must NOT be hijacked into old_vite_app!"
    );
    // app.py must be in parent_dir, NOT in old_vite_app!
    assert!(
        parent_dir.join("app.py").exists(),
        "app.py must exist at parent root"
    );
    assert!(
        !old_vite_app.join("app.py").exists(),
        "app.py must NOT be created inside old_vite_app"
    );
}

#[test]
fn test_scaffolded_tsconfig_universal_compatibility() {
    let dir = tempdir().unwrap();
    let parent_dir = dir.path();
    let mut graph = VectorGraph::new();

    // 1. Next.js scaffold
    let next_res = layer::scaffold::bootstrap_project(
        layer::scaffold::GeneratorPreset::NextJs,
        "next_demo",
        parent_dir,
        None,
        &mut graph,
    )
    .expect("Next.js bootstrap must succeed");
    assert!(next_res.success);

    let next_tsconfig_path = parent_dir.join("next_demo/tsconfig.json");
    let next_tsconfig: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&next_tsconfig_path).unwrap()).unwrap();
    let next_opts = next_tsconfig.get("compilerOptions").unwrap();

    assert_eq!(next_opts.get("target").unwrap(), "ES2020");
    assert_eq!(next_opts.get("module").unwrap(), "ESNext");
    assert_eq!(next_opts.get("moduleResolution").unwrap(), "bundler");
    assert_eq!(next_opts.get("noEmit").unwrap(), true);
    assert_eq!(next_opts.get("strict").unwrap(), true);
    assert_eq!(next_opts.get("skipLibCheck").unwrap(), true);
    assert_eq!(next_opts.get("isolatedModules").unwrap(), true);
    assert_eq!(next_opts.get("esModuleInterop").unwrap(), true);
    // Must NOT contain fragile/incompatible options
    assert!(
        next_opts.get("incremental").is_none(),
        "tsconfig must not have incremental flag causing noEmit conflict"
    );
    assert!(
        next_opts.get("plugins").is_none(),
        "tsconfig must not have uninstalled plugins"
    );
    assert!(next_opts.get("noUncheckedSideEffectImports").is_none());
    assert!(next_opts.get("erasableSyntaxOnly").is_none());

    // 2. React-TS scaffold
    let react_res = layer::scaffold::bootstrap_project(
        layer::scaffold::GeneratorPreset::ReactTs,
        "react_demo",
        parent_dir,
        None,
        &mut graph,
    )
    .expect("React-TS bootstrap must succeed");
    assert!(react_res.success);

    let react_tsconfig_path = parent_dir.join("react_demo/tsconfig.json");
    let react_tsconfig: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&react_tsconfig_path).unwrap()).unwrap();
    let react_opts = react_tsconfig.get("compilerOptions").unwrap();

    assert_eq!(react_opts.get("target").unwrap(), "ES2020");
    assert_eq!(react_opts.get("module").unwrap(), "ESNext");
    assert_eq!(react_opts.get("moduleResolution").unwrap(), "bundler");
    assert_eq!(react_opts.get("jsx").unwrap(), "react-jsx");
    assert_eq!(react_opts.get("noEmit").unwrap(), true);
    assert_eq!(react_opts.get("strict").unwrap(), true);
    assert_eq!(react_opts.get("skipLibCheck").unwrap(), true);
    assert_eq!(react_opts.get("isolatedModules").unwrap(), true);
    assert_eq!(react_opts.get("esModuleInterop").unwrap(), true);
    assert!(react_opts.get("useDefineForClassFields").is_none());
}

#[test]
fn test_do_no_harm_advisory_architecture_defaults_and_python_with_tsconfig() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();
    let mut pkg_indexer = PackageIndexer::new();
    let mut root_buf = root.to_path_buf();

    // 1. Workspace contains a tsconfig.json (e.g. from an existing or sibling setup)
    std::fs::write(
        root.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "target": "ES2020",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true
  }
}"#,
    )
    .unwrap();

    // 2. Write a modern Python 3.12 app.py using MCP execute_tool with DEFAULT options (auto_rollback_on_cascade omitted)
    let python_code = r#"import sys

def main():
    info = {"engine": "Python", "version": 3.12}
    # PEP 701 nested quotes in f-string
    msg = f"Running {info['engine']} {info['version']:.2f}"
    print(msg)

if __name__ == "__main__":
    main()
"#;

    let write_args = serde_json::json!({
        "path": "app.py",
        "content": python_code,
        "verify_mode": "fast"
        // auto_rollback_on_cascade omitted -> defaults to false
    });

    let res_str = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "write_file",
        Some(&write_args),
        &mut pkg_indexer,
    );

    assert!(
        !res_str.starts_with("Error:"),
        "write_file should not error: {}",
        res_str
    );
    let res_json: serde_json::Value = serde_json::from_str(&res_str).unwrap();

    // Do No Harm Assertion: Must succeed and never roll back
    assert!(
        res_json["success"].as_bool().unwrap_or(false),
        "write_file must succeed"
    );
    assert!(
        !res_json["circuit_breaker"]["rolled_back"]
            .as_bool()
            .unwrap_or(false),
        "Valid Python code must never be rolled back"
    );
    assert!(root.join("app.py").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("app.py")).unwrap(),
        python_code
    );
    // Workspace root must not have been modified
    assert_eq!(
        root_buf.canonicalize().unwrap(),
        root.canonicalize().unwrap()
    );

    // 3. Write a syntax-invalid file with DEFAULT options (auto_rollback_on_cascade omitted)
    // The "Do No Harm" advisory architecture preserves the file on disk (success: true, rolled_back: false)
    // and attaches non-blocking advisory diagnostics instead of deleting user/agent work!
    let broken_code = "export function broken() { const x = ; return x; }";
    let broken_args = serde_json::json!({
        "path": "src/broken.ts",
        "content": broken_code,
        "verify_mode": "fast"
        // auto_rollback_on_cascade omitted -> defaults to false
    });

    let broken_res_str = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "write_file",
        Some(&broken_args),
        &mut pkg_indexer,
    );

    let broken_res_json: serde_json::Value = serde_json::from_str(&broken_res_str).unwrap();
    assert!(
        broken_res_json["success"].as_bool().unwrap_or(false),
        "Default advisory write must report success: true"
    );
    assert!(
        !broken_res_json["circuit_breaker"]["rolled_back"]
            .as_bool()
            .unwrap_or(false),
        "Default advisory write must report rolled_back: false"
    );
    assert!(
        broken_res_json["circuit_breaker"]["triggered"]
            .as_bool()
            .unwrap_or(false),
        "Circuit breaker triggered flag is set to alert the agent"
    );
    // File remains preserved on disk
    assert!(root.join("src/broken.ts").exists());

    // 4. Agent can voluntarily call rollback_last_edit in 1ms if desired
    let rollback_res_str = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "rollback_last_edit",
        None,
        &mut pkg_indexer,
    );
    let rollback_res_json: serde_json::Value = serde_json::from_str(&rollback_res_str).unwrap();
    assert!(rollback_res_json["success"].as_bool().unwrap_or(false));
    // Broken file is now reverted
    assert!(!root.join("src/broken.ts").exists());

    // 5. Explicit auto_rollback_on_cascade: true preserves strict rollback behavior
    let strict_broken_args = serde_json::json!({
        "path": "src/strict_broken.ts",
        "content": broken_code,
        "verify_mode": "fast",
        "auto_rollback_on_cascade": true
    });

    let strict_res_str = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "write_file",
        Some(&strict_broken_args),
        &mut pkg_indexer,
    );
    let strict_res_json: serde_json::Value = serde_json::from_str(&strict_res_str).unwrap();
    assert!(
        !strict_res_json["success"].as_bool().unwrap_or(true),
        "Explicit auto_rollback_on_cascade: true must report success: false"
    );
    assert!(
        strict_res_json["circuit_breaker"]["rolled_back"]
            .as_bool()
            .unwrap_or(false),
        "Explicit auto_rollback_on_cascade: true must report rolled_back: true"
    );
    assert!(!root.join("src/strict_broken.ts").exists());
}

#[test]
fn test_compact_json_responses_and_clean_edit_receipts() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();
    let mut root_buf = root.to_path_buf();
    let mut pkg_indexer = PackageIndexer::new();

    let clean_code = "def hello():\n    return 'world'\n";
    let args = serde_json::json!({
        "path": "app.py",
        "content": clean_code,
        "verify_mode": "fast"
    });

    let res_str = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "write_file",
        Some(&args),
        &mut pkg_indexer,
    );

    // 1. Output must be compact single-line JSON without pretty indentation newlines
    assert!(
        !res_str.contains('\n'),
        "MCP response should be compact single-line JSON without newlines"
    );
    assert!(
        res_str.len() < 350,
        "Clean edit receipt should be compact (<350 bytes), was {}",
        res_str.len()
    );

    let res_json: serde_json::Value = serde_json::from_str(&res_str).unwrap();
    assert_eq!(res_json["success"], true);

    // 2. Empty arrays/objects should be omitted on clean edits
    assert!(
        res_json.get("circuit_breaker").is_none(),
        "Empty circuit_breaker should be omitted"
    );
    assert!(
        res_json.get("pending_downstream_updates").is_none(),
        "Empty pending_downstream_updates should be omitted"
    );
    assert!(
        res_json.get("missing_imports_to_create_next").is_none(),
        "Empty missing_imports should be omitted"
    );
}

#[test]
fn test_context_aware_directory_filtering() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create primary code file in src/
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.py"), "def main(): pass\n").unwrap();

    // Create non-core directories (docs_src, examples, benchmarks, site)
    std::fs::create_dir_all(root.join("docs_src")).unwrap();
    std::fs::write(root.join("docs_src/tutorial.py"), "def tutorial(): pass\n").unwrap();

    std::fs::create_dir_all(root.join("examples")).unwrap();
    std::fs::write(root.join("examples/demo.py"), "def demo(): pass\n").unwrap();

    std::fs::create_dir_all(root.join(".venv")).unwrap();
    std::fs::write(root.join(".venv/lib.py"), "def venv_lib(): pass\n").unwrap();

    assert!(layer::walker::has_primary_code_outside_docs(root));

    let mut graph = VectorGraph::new();
    graph.sync_workspace(root);

    // Primary code must be indexed
    assert!(
        graph
            .files
            .contains_key(&std::path::PathBuf::from("src/main.py"))
    );

    // Ignored / non-core directories must NOT be indexed
    assert!(
        !graph
            .files
            .contains_key(&std::path::PathBuf::from("docs_src/tutorial.py"))
    );
    assert!(
        !graph
            .files
            .contains_key(&std::path::PathBuf::from("examples/demo.py"))
    );
    assert!(
        !graph
            .files
            .contains_key(&std::path::PathBuf::from(".venv/lib.py"))
    );

    // When repository is documentation-only, docs/ IS indexed
    let doc_dir = tempdir().unwrap();
    let doc_root = doc_dir.path();
    std::fs::create_dir_all(doc_root.join("docs")).unwrap();
    std::fs::write(doc_root.join("docs/readme.md"), "# Documentation\n").unwrap();

    assert!(!layer::walker::has_primary_code_outside_docs(doc_root));
    let mut doc_graph = VectorGraph::new();
    doc_graph.sync_workspace(doc_root);
    assert!(
        doc_graph
            .files
            .contains_key(&std::path::PathBuf::from("docs/readme.md"))
    );
}

#[test]
fn test_incremental_verification_on_large_repositories() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();
    let mut root_buf = root.to_path_buf();
    let mut pkg_indexer = PackageIndexer::new();

    // Create 32 python files (>30 files threshold)
    std::fs::create_dir_all(root.join("src")).unwrap();
    for i in 0..32 {
        let name = format!("src/module_{}.py", i);
        std::fs::write(
            root.join(&name),
            format!("def func_{}(): return {}\n", i, i),
        )
        .unwrap();
    }
    graph.sync_workspace(root);
    assert!(graph.files.len() >= 32);

    // Modify 1 file via write_file
    let edit_args = serde_json::json!({
        "path": "src/module_0.py",
        "content": "def func_0(): return 999\n",
        "verify_mode": "fast"
    });
    let _ = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "write_file",
        Some(&edit_args),
        &mut pkg_indexer,
    );

    assert!(!graph.session_modified_files.is_empty());

    // verify_project without force_compiler triggers incremental AST verification
    let verify_args = serde_json::json!({});
    let verify_res_str = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "verify_project",
        Some(&verify_args),
        &mut pkg_indexer,
    );

    let verify_json: serde_json::Value = serde_json::from_str(&verify_res_str).unwrap();
    assert_eq!(verify_json["success"], true);
    assert_eq!(verify_json["error_count"], 0);
    let msg = verify_json["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("Incremental AST verification passed"),
        "Expected incremental verification message, got: {}",
        msg
    );
}

#[test]
fn test_hard_capped_output_sizes_and_task_context() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();

    // Create 30 files
    for i in 0..30 {
        let name = format!("file_{}.rs", i);
        std::fs::write(root.join(&name), format!("pub fn f_{}() {{}}\n", i)).unwrap();
    }
    graph.sync_workspace(root);

    // get_tree compact formatting default limit should cap to 25
    let tree_str = graph.format_compact_tree(None, None, None);
    assert!(tree_str.contains("Showing 25 of 30 matching files"));

    // prepare_task_context should cap relevant_files to top 10
    let packet = layer::context_compiler::ContextCompiler::compile_task_context(
        &graph,
        "f_1 f_2 f_3 f_4 f_5 f_6 f_7 f_8 f_9 f_10 f_11 f_12 f_13",
        None,
        4000,
    );
    assert!(
        packet.relevant_files.len() <= 10,
        "relevant_files must be capped to top 10, got {}",
        packet.relevant_files.len()
    );
}
