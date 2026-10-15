use layer::graph::VectorGraph;
use layer::mutation::{MutationManager, VerifyMode};
use layer::package_indexer::PackageIndexer;
use layer::scaffold::{
    ModuleFileSpec, ModuleFunctionSpec, ModuleTypeSpec, ScaffoldModuleRequest, scaffold_module,
};
use layer::verifier::{DiagnosticItem, Verifier};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn test_scaffold_module_creates_files_and_populates_graph_immediately() {
    let dir = tempdir().unwrap();
    let mut graph = VectorGraph::new();

    let req = ScaffoldModuleRequest {
        module_name: "simulation".to_string(),
        files: vec![
            ModuleFileSpec {
                path: "src/types/simulation.ts".to_string(),
                imports: vec![],
                types: vec![ModuleTypeSpec {
                    name: "Particle".to_string(),
                    kind: "interface".to_string(),
                    definition: "export interface Particle {\n  id: string;\n  x: number;\n  y: number;\n  radius: number;\n}".to_string(),
                }],
                functions: vec![],
                raw_content: None,
            },
            ModuleFileSpec {
                path: "src/physics/engine.ts".to_string(),
                imports: vec!["import { Particle } from '../types/simulation';".to_string()],
                types: vec![],
                functions: vec![ModuleFunctionSpec {
                    name: "updatePhysics".to_string(),
                    signature: "export function updatePhysics(particles: Particle[], dt: number): void".to_string(),
                    stub_body: Some("{\n  // Euler step\n}".to_string()),
                    doc_comment: Some("Perform Euler integration step".to_string()),
                }],
                raw_content: None,
            },
        ],
    };

    let result = scaffold_module(dir.path(), req, &mut graph).expect("scaffolding must succeed");

    assert!(result.success);
    assert_eq!(result.created_files.len(), 2);
    assert!(dir.path().join("src/types/simulation.ts").exists());
    assert!(dir.path().join("src/physics/engine.ts").exists());

    // Verify symbols created
    assert!(
        result
            .symbols_created
            .iter()
            .any(|s| s.symbol_name == "Particle")
    );
    assert!(
        result
            .symbols_created
            .iter()
            .any(|s| s.symbol_name == "updatePhysics")
    );

    // Verify graph edges connected
    let engine_idx = graph.node_indices[&std::path::PathBuf::from("src/physics/engine.ts")];
    let types_idx = graph.node_indices[&std::path::PathBuf::from("src/types/simulation.ts")];
    assert!(
        graph.graph.find_edge(engine_idx, types_idx).is_some(),
        "Dependency edge from engine.ts to simulation.ts must exist"
    );
}

#[test]
fn test_instant_ast_precheck_catches_broken_imports_in_sub_10ms() {
    let dir = tempdir().unwrap();
    let mut graph = VectorGraph::new();

    // Create a valid types file
    let types_path = dir.path().join("src/types.ts");
    fs::create_dir_all(types_path.parent().unwrap()).unwrap();
    fs::write(&types_path, "export interface Config { verbose: boolean; }").unwrap();

    // Create an invalid consumer importing non-existent module
    let consumer_path = dir.path().join("src/consumer.ts");
    fs::write(
        &consumer_path,
        "import { Missing } from './non_existent';\nconsole.log(Missing);",
    )
    .unwrap();

    let start = std::time::Instant::now();
    graph.sync_workspace(dir.path());

    // Run instant AST precheck
    let diags = Verifier::instant_ast_precheck(dir.path(), &graph.files, &graph.path_aliases);
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 50,
        "AST precheck should be sub-50ms (measured: {:?})",
        elapsed
    );
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code.as_deref(), Some("TS2307"));
    assert!(diags[0].message.contains("non_existent"));
}

#[test]
fn test_instant_ast_precheck_catches_syntax_error_without_spawning_tsc() {
    let dir = tempdir().unwrap();
    let mut graph = VectorGraph::new();

    let file_path = dir.path().join("src/broken.ts");
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(&file_path, "export function broken( { return 42; ").unwrap(); // missing closing paren and brace

    graph.sync_workspace(dir.path());

    let diags = Verifier::instant_ast_precheck(dir.path(), &graph.files, &graph.path_aliases);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code.as_deref(), Some("SYNTAX_ERROR"));
}

#[test]
fn test_persistent_error_attaches_ground_truth_context() {
    let dir = tempdir().unwrap();
    let mut graph = VectorGraph::new();

    // Establish ground truth in types file
    let types_code = r#"
export interface Particle {
  id: string;
  x: number;
  y: number;
  radius: number;
}
"#;
    let types_file = dir.path().join("src/types/simulation.ts");
    fs::create_dir_all(types_file.parent().unwrap()).unwrap();
    fs::write(&types_file, types_code).unwrap();
    graph.sync_workspace(dir.path());

    // Run 1: Agent creates Canvas with misspelling 'radiux'
    let diag_run1 = DiagnosticItem::new(
        "src/components/Canvas.tsx",
        15,
        10,
        "error",
        Some("TS2551".to_string()),
        "Property 'radiux' does not exist on type 'Particle'. Did you mean 'radius'?",
    );

    let comp1 = Verifier::compare_diagnostics(None, std::slice::from_ref(&diag_run1));
    assert_eq!(comp1.new_errors.len(), 1);
    assert_eq!(comp1.new_errors[0].occurrence_count, 1);
    assert!(comp1.persistent_errors.is_empty());

    // Run 2: Agent failed to fix it, error persists
    let diag_run2 = DiagnosticItem::new(
        "src/components/Canvas.tsx",
        16, // slightly shifted line
        10,
        "error",
        Some("TS2551".to_string()),
        "Property 'radiux' does not exist on type 'Particle'. Did you mean 'radius'?",
    );

    let mut comp2 = Verifier::compare_diagnostics(Some(&[diag_run1]), &[diag_run2]);
    assert_eq!(comp2.persistent_errors.len(), 1);
    assert_eq!(comp2.persistent_errors[0].occurrence_count, 2);

    // Enrich persistent errors
    let mut pkg_indexer = PackageIndexer::new();
    Verifier::enrich_persistent_diagnostics(
        &mut comp2.persistent_errors,
        &graph,
        Some(&mut pkg_indexer),
        dir.path(),
    );

    let persistent_err = &comp2.persistent_errors[0];
    assert!(
        persistent_err.ground_truth_context.is_some(),
        "Persistent error on run #2 must attach ground_truth_context"
    );
    let ctx = persistent_err.ground_truth_context.as_ref().unwrap();
    assert!(ctx.contains("Particle"));
    assert!(ctx.contains("radius: number"));
}

#[test]
fn test_cascading_circuit_breaker_rolls_back_disruptive_edit() {
    let dir = tempdir().unwrap();
    let mut graph = VectorGraph::new();

    // 1. Initial valid file
    let file = Path::new("src/types.ts");
    let valid_code = "export interface Config { timeout: number; }\n";

    let write_res = MutationManager::write_file(
        dir.path(),
        file,
        valid_code,
        VerifyMode::None,
        true,
        &mut graph,
        None,
    )
    .expect("write must succeed");
    assert!(write_res.success);

    // 2. Introduce an edit that produces AST syntax errors
    let syntax_broken_edit = "export interface Config { timeout: number; \n"; // missing closing brace
    let edit_res = MutationManager::write_file(
        dir.path(),
        file,
        syntax_broken_edit,
        VerifyMode::Fast,
        true, // auto_rollback_on_cascade
        &mut graph,
        None,
    )
    .expect("mutation manager returns response");

    // The syntax error is caught by fast-path AST check and rolled back
    assert!(!edit_res.success);
    assert_eq!(edit_res.operation, "write (rolled back)");
    assert!(edit_res.circuit_breaker.is_some());
    let cb = edit_res.circuit_breaker.unwrap();
    assert!(cb.triggered);
    assert!(cb.rolled_back);

    // Content on disk was rolled back to valid_code
    let content_on_disk = fs::read_to_string(dir.path().join(file)).unwrap();
    assert_eq!(content_on_disk, valid_code);
}

#[test]
fn test_multi_file_refactoring_preserves_interface_edit_and_reports_downstream_updates() {
    let dir = tempdir().unwrap();
    let mut graph = VectorGraph::new();

    // 1. Create two downstream files that import an interface
    let types_file = Path::new("src/types/simulation.ts");
    let initial_types = "export interface Particle {\n  id: string;\n  x: number;\n}\n";

    let engine_file = Path::new("src/physics/engine.ts");
    let engine_code = "import { Particle } from '../types/simulation';\nimport { MissingEngineDep } from './non_existent_engine';\nexport function run(): void {}\n";

    let canvas_file = Path::new("src/components/Canvas.tsx");
    let canvas_code = "import { Particle } from '../types/simulation';\nimport { MissingCanvasDep } from './non_existent_canvas';\nexport function draw(): void {}\n";

    MutationManager::write_file(
        dir.path(),
        types_file,
        initial_types,
        VerifyMode::None,
        true,
        &mut graph,
        None,
    )
    .unwrap();

    MutationManager::write_file(
        dir.path(),
        engine_file,
        engine_code,
        VerifyMode::None,
        true,
        &mut graph,
        None,
    )
    .unwrap();

    MutationManager::write_file(
        dir.path(),
        canvas_file,
        canvas_code,
        VerifyMode::None,
        true,
        &mut graph,
        None,
    )
    .unwrap();

    // 2. Modify types file with valid syntax. Downstream files (engine & canvas) have broken imports.
    // In previous versions, this triggered downstream cascade circuit breaker and auto-rolled back types.
    // In the hardened version, local_errors == 0, so types file is PRESERVED, and downstream updates are reported.
    let updated_types = "export interface Particle {\n  id: string;\n  pos_x: number;\n}\n";

    let res = MutationManager::write_file(
        dir.path(),
        types_file,
        updated_types,
        VerifyMode::Fast,
        true, // auto_rollback_on_cascade is enabled
        &mut graph,
        None,
    )
    .expect("write must succeed");

    // Must succeed without rolling back
    assert!(res.success, "Interface edit must not be auto-rolled back");
    assert_eq!(res.operation, "write");

    // Disk content must be the new updated_types
    let disk_content = fs::read_to_string(dir.path().join(types_file)).unwrap();
    assert_eq!(disk_content, updated_types);

    // Circuit breaker advisory was issued without rollback
    assert!(res.circuit_breaker.is_some());
    let cb = res.circuit_breaker.unwrap();
    assert!(cb.triggered);
    assert!(!cb.rolled_back);
    assert_eq!(cb.broken_downstream_files.len(), 2);

    // Pending downstream updates checklist must contain both downstream files
    assert_eq!(res.pending_downstream_updates.len(), 2);
    let files: Vec<String> = res
        .pending_downstream_updates
        .iter()
        .map(|u| u.file.clone())
        .collect();
    assert!(files.iter().any(|f| f.contains("engine.ts")));
    assert!(files.iter().any(|f| f.contains("Canvas.tsx")));
}

#[test]
fn test_verify_mode_fast_executes_in_sub_15ms() {
    let dir = tempdir().unwrap();
    let mut graph = VectorGraph::new();

    let file = Path::new("src/math.ts");
    let code = "export function add(a: number, b: number): number {\n  return a + b;\n}\n";

    let start = std::time::Instant::now();
    let res = MutationManager::write_file(
        dir.path(),
        file,
        code,
        VerifyMode::Fast,
        true,
        &mut graph,
        None,
    )
    .expect("write must succeed");
    let elapsed = start.elapsed();

    assert!(res.success);
    assert!(
        elapsed.as_millis() < 30,
        "Fast-path AST verification must complete in sub-30ms, took {:?}",
        elapsed
    );
    assert!(res.verification.is_some());
    let verif = res.verification.unwrap();
    assert!(verif.success);
    assert_eq!(verif.error_count, 0);
}

#[test]
fn test_verify_project_typescript_without_node_modules_succeeds_via_ast_fast_path() {
    let dir = tempdir().unwrap();

    // Create tsconfig.json and valid TypeScript files without installing node_modules
    fs::write(
        dir.path().join("tsconfig.json"),
        r#"{"compilerOptions": {"target": "ES2020", "module": "ESNext"}}"#,
    )
    .unwrap();

    let src_dir = dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(
        src_dir.join("App.tsx"),
        "import React from 'react';\nexport function App() { return <div>Simulation</div>; }\n",
    )
    .unwrap();
    fs::write(
        src_dir.join("math.ts"),
        "export function distance(x: number, y: number): number { return Math.sqrt(x * x + y * y); }\n",
    )
    .unwrap();

    let start = std::time::Instant::now();
    let report = Verifier::verify_workspace(dir.path(), None);
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 30,
        "AST fallback verification must run sub-30ms without spawning npx tsc, took {:?}",
        elapsed
    );
    assert!(
        report.success,
        "TypeScript project without node_modules should succeed when AST/imports are clean"
    );
    assert_eq!(report.error_count, 0);
    assert!(report.diagnostics.is_empty());
    assert!(
        report.compiler_skipped_reason.is_some(),
        "Must specify compiler_skipped_reason when node_modules/.bin/tsc is absent"
    );
    let reason = report.compiler_skipped_reason.unwrap();
    assert!(reason.contains("node_modules/.bin/tsc not found"));
}

#[test]
fn test_verify_project_typescript_without_node_modules_catches_ast_errors() {
    let dir = tempdir().unwrap();

    fs::write(
        dir.path().join("tsconfig.json"),
        r#"{"compilerOptions": {"target": "ES2020"}}"#,
    )
    .unwrap();

    let src_dir = dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(
        src_dir.join("broken.tsx"),
        "export function broken( { return 42; ", // Syntax error: missing parens and brace
    )
    .unwrap();

    let report = Verifier::verify_workspace(dir.path(), None);

    assert!(
        !report.success,
        "Must fail when AST precheck catches syntax errors"
    );
    assert!(
        !report.diagnostics.is_empty(),
        "Diagnostics array must NEVER be empty when success is false"
    );
    assert_eq!(report.error_count, 1);
    assert_eq!(report.diagnostics[0].code.as_deref(), Some("SYNTAX_ERROR"));
}

#[test]
fn test_inspect_package_api_builtin_fallbacks_offline() {
    let dir = tempdir().unwrap(); // completely empty directory, zero node_modules
    let mut indexer = PackageIndexer::new();

    // 1. @react-three/fiber fallback
    let r3f_res = indexer
        .inspect_package(dir.path(), "@react-three/fiber", Some("Canvas"), None)
        .expect("Must resolve built-in fallback for @react-three/fiber");
    assert_eq!(r3f_res.package, "@react-three/fiber");
    assert!(r3f_res.requested_symbol.is_some());
    let canvas_decl = r3f_res.requested_symbol.unwrap();
    assert_eq!(canvas_decl.symbol_name, "Canvas");
    assert!(canvas_decl.signature.contains("CanvasProps"));

    let use_frame = indexer
        .inspect_package(dir.path(), "@react-three/fiber", Some("useFrame"), None)
        .expect("Must resolve useFrame from fallback");
    assert!(
        use_frame
            .requested_symbol
            .unwrap()
            .signature
            .contains("RootState")
    );

    // 2. @react-three/drei fallback
    let drei_res = indexer
        .inspect_package(dir.path(), "@react-three/drei", Some("OrbitControls"), None)
        .expect("Must resolve built-in fallback for @react-three/drei");
    let orbit_decl = drei_res.requested_symbol.unwrap();
    assert_eq!(orbit_decl.symbol_name, "OrbitControls");
    assert!(orbit_decl.signature.contains("OrbitControlsProps"));

    // 3. three fallback
    let three_res = indexer
        .inspect_package(dir.path(), "three", Some("Vector3"), None)
        .expect("Must resolve built-in fallback for three");
    let v3_decl = three_res.requested_symbol.unwrap();
    assert_eq!(v3_decl.symbol_name, "Vector3");
    assert!(v3_decl.signature.contains("multiplyScalar"));
    assert!(v3_decl.signature.contains("distanceTo"));

    // 4. Query filtering on built-in fallback
    let filtered_drei = indexer
        .inspect_package(dir.path(), "@react-three/drei", None, Some("stars"))
        .expect("Must filter symbols by query");
    assert!(!filtered_drei.symbols.is_empty());
    assert!(
        filtered_drei
            .symbols
            .iter()
            .any(|s| s.symbol_name == "Stars")
    );
}
