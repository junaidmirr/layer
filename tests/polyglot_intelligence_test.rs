use layer::graph::VectorGraph;
use layer::package_indexer::PackageIndexer;
use layer::scaffold::{
    GeneratorPreset, ModuleFileSpec, ModuleFunctionSpec, ModuleTypeSpec, ScaffoldModuleRequest,
    bootstrap_project, scaffold_module,
};
use layer::scanner::AstScanner;
use layer::verifier::{ProjectType, Verifier};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_python_bootstrap_and_scaffold_intelligence() {
    let dir = tempdir().unwrap();
    let mut graph = VectorGraph::new();

    // 1. Bootstrap Python project
    let result = bootstrap_project(
        GeneratorPreset::Python,
        "quantum_service",
        dir.path(),
        Some(vec!["fastapi".to_string(), "pydantic".to_string()]),
        &mut graph,
    )
    .expect("Python bootstrap must succeed");

    assert!(result.success);
    assert_eq!(result.template, "python");
    let proj_dir = dir.path().join("quantum_service");
    assert!(proj_dir.join("pyproject.toml").exists());
    assert!(proj_dir.join("requirements.txt").exists());
    assert!(proj_dir.join("src/quantum_service/__init__.py").exists());
    assert!(proj_dir.join("src/quantum_service/main.py").exists());
    assert!(proj_dir.join("tests/test_main.py").exists());

    // 2. Immediate workspace sync verified
    assert!(result.files_synced >= 4);
    assert!(
        graph
            .files
            .contains_key(&std::path::PathBuf::from("src/quantum_service/main.py"))
    );

    // 3. Project type detection
    assert_eq!(ProjectType::detect(&proj_dir), ProjectType::Python);

    // 4. Verify clean project returns success: true via AST Tier-1 fallback (never success: false with 0 errors)
    let verify_clean = Verifier::verify_workspace(&proj_dir, None);
    assert!(verify_clean.success);
    assert_eq!(verify_clean.error_count, 0);
    assert!(verify_clean.compiler_skipped_reason.is_some());

    // 5. Scaffold Python module with stubs
    let req = ScaffoldModuleRequest {
        module_name: "state_processor".to_string(),
        files: vec![
            ModuleFileSpec {
                path: "src/quantum_service/models.py".to_string(),
                imports: vec!["from pydantic import BaseModel".to_string()],
                types: vec![ModuleTypeSpec {
                    name: "StateVector".to_string(),
                    kind: "class".to_string(),
                    definition: "class StateVector(BaseModel):\n    dimension: int\n    amplitudes: list[float]".to_string(),
                }],
                functions: vec![],
                raw_content: None,
            },
            ModuleFileSpec {
                path: "src/quantum_service/processor.py".to_string(),
                imports: vec![
                    "from .models import StateVector".to_string(),
                ],
                types: vec![],
                functions: vec![ModuleFunctionSpec {
                    name: "normalize_state".to_string(),
                    signature: "def normalize_state(vec: StateVector) -> StateVector".to_string(),
                    stub_body: None, // should default to raise NotImplementedError
                    doc_comment: Some("Normalizes state amplitudes to unit norm.".to_string()),
                }],
                raw_content: None,
            },
        ],
    };

    let scaffold_res = scaffold_module(&proj_dir, req, &mut graph).expect("scaffold must succeed");
    assert!(scaffold_res.success);
    assert_eq!(scaffold_res.symbols_created.len(), 2);

    let processor_content =
        fs::read_to_string(proj_dir.join("src/quantum_service/processor.py")).unwrap();
    // Python imports must not have semicolons
    assert!(!processor_content.contains(';'));
    // Python stubs must use NotImplementedError
    assert!(processor_content.contains("raise NotImplementedError"));
    // Python docstring must use triple quotes
    assert!(processor_content.contains("\"\"\"Normalizes state amplitudes to unit norm.\"\"\""));

    // 6. Verify cross-file dependency graph edge from processor.py to models.py
    let models_path = std::path::PathBuf::from("src/quantum_service/models.py");
    let dependents = graph.get_downstream_dependents(&models_path);
    assert!(dependents.iter().any(|p| p.ends_with("processor.py")));

    // 7. Verify instant AST precheck catches Python syntax errors
    fs::write(
        proj_dir.join("src/quantum_service/broken.py"),
        "def broken_function(\n    return 42\n",
    )
    .unwrap();
    graph.sync_workspace(&proj_dir);

    let diags = Verifier::instant_ast_precheck(&proj_dir, &graph.files, &graph.path_aliases);
    assert!(
        diags
            .iter()
            .any(|d| d.file.contains("broken.py") && d.code.as_deref() == Some("SYNTAX_ERROR"))
    );
}

#[test]
fn test_go_bootstrap_and_scaffold_intelligence() {
    let dir = tempdir().unwrap();
    let mut graph = VectorGraph::new();

    // 1. Bootstrap Go project
    let result = bootstrap_project(
        GeneratorPreset::Go,
        "flight_tracker",
        dir.path(),
        None,
        &mut graph,
    )
    .expect("Go bootstrap must succeed");

    assert!(result.success);
    assert_eq!(result.template, "go");
    let proj_dir = dir.path().join("flight_tracker");
    assert!(proj_dir.join("go.mod").exists());
    assert!(proj_dir.join("main.go").exists());
    assert!(proj_dir.join("pkg/util.go").exists());

    // 2. Project type detection
    assert_eq!(ProjectType::detect(&proj_dir), ProjectType::Go);

    // 3. Clean verification via AST Tier-1 fallback
    let verify_clean = Verifier::verify_workspace(&proj_dir, None);
    assert!(verify_clean.success);

    // 4. Scaffold Go module with panic stub
    let req = ScaffoldModuleRequest {
        module_name: "telemetry".to_string(),
        files: vec![ModuleFileSpec {
            path: "pkg/telemetry.go".to_string(),
            imports: vec!["import \"fmt\"".to_string()],
            types: vec![ModuleTypeSpec {
                name: "AltitudeReport".to_string(),
                kind: "struct".to_string(),
                definition: "type AltitudeReport struct {\n    Meters float64\n}".to_string(),
            }],
            functions: vec![ModuleFunctionSpec {
                name: "CalculateAscentRate".to_string(),
                signature: "func CalculateAscentRate(r1, r2 AltitudeReport, dt float64) float64"
                    .to_string(),
                stub_body: None,
                doc_comment: Some("Calculates vertical velocity in m/s.".to_string()),
            }],
            raw_content: None,
        }],
    };

    let scaffold_res = scaffold_module(&proj_dir, req, &mut graph).expect("scaffold must succeed");
    assert!(scaffold_res.success);

    let telemetry_content = fs::read_to_string(proj_dir.join("pkg/telemetry.go")).unwrap();
    assert!(telemetry_content.contains("panic(\"TODO: implement\")"));

    // 5. Verify AST precheck catches Go syntax errors
    fs::write(
        proj_dir.join("pkg/bad.go"),
        "package pkg\nfunc invalid() {\n    return 10\n",
    )
    .unwrap();
    graph.sync_workspace(&proj_dir);

    let diags = Verifier::instant_ast_precheck(&proj_dir, &graph.files, &graph.path_aliases);
    assert!(
        diags
            .iter()
            .any(|d| d.file.contains("bad.go") && d.code.as_deref() == Some("SYNTAX_ERROR"))
    );
}

#[test]
fn test_cpp_bootstrap_and_scaffold_intelligence() {
    let dir = tempdir().unwrap();
    let mut graph = VectorGraph::new();

    // 1. Bootstrap C++ project
    let result = bootstrap_project(
        GeneratorPreset::Cpp,
        "phys_engine",
        dir.path(),
        None,
        &mut graph,
    )
    .expect("Cpp bootstrap must succeed");

    assert!(result.success);
    assert_eq!(result.template, "cpp");
    let proj_dir = dir.path().join("phys_engine");
    assert!(proj_dir.join("CMakeLists.txt").exists());
    assert!(proj_dir.join("include/phys_engine/core.hpp").exists());
    assert!(proj_dir.join("src/core.cpp").exists());
    assert!(proj_dir.join("src/main.cpp").exists());

    // 2. Project type detection
    assert_eq!(ProjectType::detect(&proj_dir), ProjectType::Cpp);

    // 3. Clean verification
    let verify_clean = Verifier::verify_workspace(&proj_dir, None);
    assert!(verify_clean.success);

    // 4. Scaffold C++ header and source
    let req = ScaffoldModuleRequest {
        module_name: "rigid_body".to_string(),
        files: vec![
            ModuleFileSpec {
                path: "include/phys_engine/rigid_body.hpp".to_string(),
                imports: vec![],
                types: vec![ModuleTypeSpec {
                    name: "RigidBody".to_string(),
                    kind: "struct".to_string(),
                    definition: "struct RigidBody {\n    double mass;\n};".to_string(),
                }],
                functions: vec![],
                raw_content: None,
            },
            ModuleFileSpec {
                path: "src/rigid_body.cpp".to_string(),
                imports: vec![
                    "#include \"phys_engine/rigid_body.hpp\"".to_string(),
                    "#include <stdexcept>".to_string(),
                ],
                types: vec![],
                functions: vec![ModuleFunctionSpec {
                    name: "integrate_euler".to_string(),
                    signature: "void integrate_euler(RigidBody& body, double dt)".to_string(),
                    stub_body: None,
                    doc_comment: Some("Explicit Euler numerical integrator.".to_string()),
                }],
                raw_content: None,
            },
        ],
    };

    let scaffold_res = scaffold_module(&proj_dir, req, &mut graph).expect("scaffold must succeed");
    assert!(scaffold_res.success);

    let hpp_content =
        fs::read_to_string(proj_dir.join("include/phys_engine/rigid_body.hpp")).unwrap();
    assert!(hpp_content.contains("#pragma once"));

    let cpp_content = fs::read_to_string(proj_dir.join("src/rigid_body.cpp")).unwrap();
    assert!(cpp_content.contains("throw std::runtime_error(\"TODO: implement\");"));

    // 5. Dependency edge resolution for #include "phys_engine/rigid_body.hpp"
    let hpp_path = std::path::PathBuf::from("include/phys_engine/rigid_body.hpp");
    let dependents = graph.get_downstream_dependents(&hpp_path);
    assert!(dependents.iter().any(|p| p.ends_with("rigid_body.cpp")));
}

#[test]
fn test_swift_and_kotlin_lexical_intelligence() {
    // 1. Balanced Swift code
    let valid_swift = r#"
import Foundation

public struct UserProfile {
    public let id: String
    public var name: String

    public func displayName() -> String {
        return "User: \(name)"
    }
}
"#;
    assert!(AstScanner::check_lexical_syntax(valid_swift).is_ok());

    // 2. Unbalanced Swift code (unclosed bracket)
    let invalid_swift = r#"
import Foundation

public struct UserProfile {
    public let id: String
"#;
    let err = AstScanner::check_lexical_syntax(invalid_swift).unwrap_err();
    assert_eq!(err.0, 4); // Line of unclosed '{'
    assert!(err.2.contains("Unclosed delimiter '{'"));

    // 3. Balanced Kotlin code
    let valid_kt = r#"
package com.example.service

data class SimulationConfig(
    val stepSize: Double = 0.016,
    val iterations: Int = 100
)

fun runSimulation(config: SimulationConfig) {
    println("Running: $config")
}
"#;
    assert!(AstScanner::check_lexical_syntax(valid_kt).is_ok());

    // 4. Mismatched Kotlin delimiter
    let invalid_kt = r#"
package com.example.service

fun test() {
    val items = listOf(1, 2, 3]
}
"#;
    let err_kt = AstScanner::check_lexical_syntax(invalid_kt).unwrap_err();
    assert!(err_kt.2.contains("Mismatched delimiter ']'"));

    // 5. Instant AST precheck on Swift & Kotlin files
    let dir = tempdir().unwrap();
    let mut graph = VectorGraph::new();

    let result_swift = bootstrap_project(
        GeneratorPreset::Swift,
        "ios_engine",
        dir.path(),
        None,
        &mut graph,
    )
    .unwrap();
    assert!(result_swift.success);
    assert_eq!(
        ProjectType::detect(&dir.path().join("ios_engine")),
        ProjectType::Swift
    );

    let verify_swift = Verifier::verify_workspace(&dir.path().join("ios_engine"), None);
    assert!(verify_swift.success);

    // Write a syntax error in Swift file
    fs::write(
        dir.path().join("ios_engine/Sources/ios_engine/main.swift"),
        "func test() {\n",
    )
    .unwrap();
    graph.sync_workspace(&dir.path().join("ios_engine"));
    let diags = Verifier::instant_ast_precheck(
        &dir.path().join("ios_engine"),
        &graph.files,
        &graph.path_aliases,
    );
    assert!(
        diags
            .iter()
            .any(|d| d.code.as_deref() == Some("SYNTAX_ERROR"))
    );
}

#[test]
fn test_polyglot_package_indexer_fallbacks() {
    let mut indexer = PackageIndexer::new();
    let root = std::path::Path::new("/workspace");

    // 1. Python: FastAPI
    let fastapi = indexer
        .inspect_package(root, "fastapi", Some("FastAPI"), None)
        .unwrap();
    assert_eq!(fastapi.symbols[0].symbol_name, "FastAPI");
    assert!(fastapi.symbols[0].signature.contains("class FastAPI"));

    // 2. Python: Pydantic
    let pydantic = indexer
        .inspect_package(root, "pydantic", Some("BaseModel"), None)
        .unwrap();
    assert_eq!(pydantic.symbols[0].symbol_name, "BaseModel");

    // 3. Python: PyTorch
    let torch = indexer
        .inspect_package(root, "torch", Some("Tensor"), None)
        .unwrap();
    assert_eq!(torch.symbols[0].symbol_name, "Tensor");

    // 4. Rust: Tokio
    let tokio = indexer
        .inspect_package(root, "tokio", Some("spawn"), None)
        .unwrap();
    assert_eq!(tokio.symbols[0].symbol_name, "spawn");
    assert!(tokio.symbols[0].signature.contains("pub fn spawn"));

    // 5. Rust: Serde
    let serde = indexer
        .inspect_package(root, "serde", Some("Serialize"), None)
        .unwrap();
    assert_eq!(serde.symbols[0].symbol_name, "Serialize");

    // 6. Rust: Axum
    let axum = indexer
        .inspect_package(root, "axum", Some("Router"), None)
        .unwrap();
    assert_eq!(axum.symbols[0].symbol_name, "Router");

    // 7. Go: Gin
    let gin = indexer
        .inspect_package(root, "gin", Some("Context"), None)
        .unwrap();
    assert_eq!(gin.symbols[0].symbol_name, "Context");

    // 8. C++: Nlohmann JSON
    let json_lib = indexer
        .inspect_package(root, "nlohmann/json", Some("json"), None)
        .unwrap();
    assert_eq!(json_lib.symbols[0].symbol_name, "json");

    // 9. C#: Microsoft.AspNetCore.Mvc
    let aspnet = indexer
        .inspect_package(
            root,
            "Microsoft.AspNetCore.Mvc",
            Some("ControllerBase"),
            None,
        )
        .unwrap();
    assert_eq!(aspnet.symbols[0].symbol_name, "ControllerBase");

    // 10. C#: System.Text.Json
    let sys_json = indexer
        .inspect_package(root, "System.Text.Json", Some("JsonSerializer"), None)
        .unwrap();
    assert_eq!(sys_json.symbols[0].symbol_name, "JsonSerializer");

    // 11. Java: Spring Boot
    let spring_boot = indexer
        .inspect_package(
            root,
            "org.springframework.boot",
            Some("SpringBootApplication"),
            None,
        )
        .unwrap();
    assert_eq!(spring_boot.symbols[0].symbol_name, "SpringBootApplication");

    // 12. Java: Spring Web
    let spring_web = indexer
        .inspect_package(
            root,
            "org.springframework.web.bind.annotation",
            Some("RestController"),
            None,
        )
        .unwrap();
    assert_eq!(spring_web.symbols[0].symbol_name, "RestController");
}

#[test]
fn test_csharp_bootstrap_and_scaffold_intelligence() {
    let dir = tempdir().unwrap();
    let mut graph = VectorGraph::new();

    // 1. Bootstrap C# project
    let result = bootstrap_project(
        GeneratorPreset::CSharp,
        "dotnet_sim",
        dir.path(),
        None,
        &mut graph,
    )
    .expect("C# bootstrap must succeed");

    assert!(result.success);
    assert_eq!(result.template, "csharp");
    let proj_dir = dir.path().join("dotnet_sim");
    assert!(proj_dir.join("dotnet_sim.csproj").exists());
    assert!(proj_dir.join("Program.cs").exists());

    // 2. Project detection
    assert_eq!(ProjectType::detect(&proj_dir), ProjectType::CSharp);

    // 3. Clean verification via AST Tier-1 fallback
    let verify_clean = Verifier::verify_workspace(&proj_dir, None);
    assert!(verify_clean.success);
    assert_eq!(verify_clean.error_count, 0);

    // 4. Scaffold C# module with stubs
    let req = ScaffoldModuleRequest {
        module_name: "physics_module".to_string(),
        files: vec![
            ModuleFileSpec {
                path: "Models/BodyState.cs".to_string(),
                imports: vec!["using System".to_string()], // missing semicolon should be added automatically
                types: vec![ModuleTypeSpec {
                    name: "BodyState".to_string(),
                    kind: "class".to_string(),
                    definition: "public class BodyState {\n    public double Mass { get; set; }\n    public double[] Position { get; set; } = new double[3];\n}".to_string(),
                }],
                functions: vec![],
                raw_content: None,
            },
            ModuleFileSpec {
                path: "Engines/PhysicsEngine.cs".to_string(),
                imports: vec!["using dotnet_sim.Models;".to_string()],
                types: vec![],
                functions: vec![ModuleFunctionSpec {
                    name: "StepSimulation".to_string(),
                    signature: "public void StepSimulation(BodyState body, double dt)".to_string(),
                    stub_body: None,
                    doc_comment: Some("Advances simulation by delta time dt.".to_string()),
                }],
                raw_content: None,
            },
        ],
    };

    let scaffold_res = scaffold_module(&proj_dir, req, &mut graph).expect("scaffold must succeed");
    assert!(scaffold_res.success);

    let engine_content = fs::read_to_string(proj_dir.join("Engines/PhysicsEngine.cs")).unwrap();
    assert!(engine_content.contains("throw new NotImplementedException();"));

    let models_content = fs::read_to_string(proj_dir.join("Models/BodyState.cs")).unwrap();
    assert!(models_content.contains("using System;"));

    // 5. Cross-file graph edge
    let models_path = std::path::PathBuf::from("Models/BodyState.cs");
    let dependents = graph.get_downstream_dependents(&models_path);
    assert!(
        dependents
            .iter()
            .any(|d| d.ends_with("Engines/PhysicsEngine.cs")),
        "Graph must track C# using dependency: {:?}",
        dependents
    );

    // 6. Instant AST precheck on C# syntax error
    fs::write(proj_dir.join("Program.cs"), "public class Program {\n").unwrap();
    graph.sync_workspace(&proj_dir);
    let diags = Verifier::instant_ast_precheck(&proj_dir, &graph.files, &graph.path_aliases);
    assert!(
        diags
            .iter()
            .any(|d| d.code.as_deref() == Some("SYNTAX_ERROR"))
    );
}

#[test]
fn test_java_bootstrap_and_scaffold_intelligence() {
    let dir = tempdir().unwrap();
    let mut graph = VectorGraph::new();

    // 1. Bootstrap Java project
    let result = bootstrap_project(
        GeneratorPreset::Java,
        "spring_sim",
        dir.path(),
        None,
        &mut graph,
    )
    .expect("Java bootstrap must succeed");

    assert!(result.success);
    assert_eq!(result.template, "java");
    let proj_dir = dir.path().join("spring_sim");
    assert!(proj_dir.join("pom.xml").exists());
    assert!(
        proj_dir
            .join("src/main/java/com/example/Main.java")
            .exists()
    );

    // 2. Project detection
    assert_eq!(ProjectType::detect(&proj_dir), ProjectType::Java);

    // 3. Clean verification via AST Tier-1 fallback
    let verify_clean = Verifier::verify_workspace(&proj_dir, None);
    assert!(verify_clean.success);
    assert_eq!(verify_clean.error_count, 0);

    // 4. Scaffold Java module with stubs
    let req = ScaffoldModuleRequest {
        module_name: "particle_service".to_string(),
        files: vec![
            ModuleFileSpec {
                path: "src/main/java/com/example/models/Particle.java".to_string(),
                imports: vec!["package com.example.models".to_string()],
                types: vec![ModuleTypeSpec {
                    name: "Particle".to_string(),
                    kind: "class".to_string(),
                    definition: "public class Particle {\n    private double x, y, z;\n    public Particle(double x, double y, double z) { this.x = x; this.y = y; this.z = z; }\n}".to_string(),
                }],
                functions: vec![],
                raw_content: None,
            },
            ModuleFileSpec {
                path: "src/main/java/com/example/service/SimulationService.java".to_string(),
                imports: vec![
                    "package com.example.service".to_string(),
                    "import com.example.models.Particle".to_string(),
                ],
                types: vec![],
                functions: vec![ModuleFunctionSpec {
                    name: "computeTrajectory".to_string(),
                    signature: "public void computeTrajectory(Particle p, double duration)".to_string(),
                    stub_body: None,
                    doc_comment: Some("Computes numerical trajectory integration.".to_string()),
                }],
                raw_content: None,
            },
        ],
    };

    let scaffold_res = scaffold_module(&proj_dir, req, &mut graph).expect("scaffold must succeed");
    assert!(scaffold_res.success);

    let service_content = fs::read_to_string(
        proj_dir.join("src/main/java/com/example/service/SimulationService.java"),
    )
    .unwrap();
    assert!(
        service_content.contains("throw new UnsupportedOperationException(\"TODO: implement\");")
    );
    assert!(service_content.contains("import com.example.models.Particle;"));

    // 5. Cross-file graph edge
    let particle_path = std::path::PathBuf::from("src/main/java/com/example/models/Particle.java");
    let dependents = graph.get_downstream_dependents(&particle_path);
    assert!(
        dependents
            .iter()
            .any(|d| d.ends_with("SimulationService.java")),
        "Graph must track Java import dependency: {:?}",
        dependents
    );

    // 6. Instant AST precheck on Java syntax error (mismatched delimiter)
    fs::write(
        proj_dir.join("src/main/java/com/example/Main.java"),
        "package com.example;\npublic class Main {\n    public static void main(String[] args) {\n        int[] arr = new int[5);\n    }\n}\n",
    )
    .unwrap();
    graph.sync_workspace(&proj_dir);
    let diags = Verifier::instant_ast_precheck(&proj_dir, &graph.files, &graph.path_aliases);
    assert!(
        diags
            .iter()
            .any(|d| d.code.as_deref() == Some("SYNTAX_ERROR"))
    );
}
