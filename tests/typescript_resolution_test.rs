use layer::graph::{normalize_relative_path, resolve_module_import_path, VectorGraph};
use petgraph::Direction;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn test_normalize_relative_path() {
    assert_eq!(
        normalize_relative_path(Path::new("src/components/../physics/engine.ts")),
        PathBuf::from("src/physics/engine.ts")
    );
    assert_eq!(
        normalize_relative_path(Path::new("./src/./types.ts")),
        PathBuf::from("src/types.ts")
    );
}

#[test]
fn test_resolve_module_import_path_extensionless_and_aliased() {
    let mut known_files = HashSet::new();
    known_files.insert(PathBuf::from("src/types/simulation.ts"));
    known_files.insert(PathBuf::from("src/physics/engine.ts"));
    known_files.insert(PathBuf::from("src/components/Canvas.tsx"));
    known_files.insert(PathBuf::from("src/utils/index.ts"));

    let mut aliases = HashMap::new();
    aliases.insert("@/*".to_string(), "src/*".to_string());
    aliases.insert("~/*".to_string(), "src/*".to_string());

    // 1. Extensionless relative import from sibling: ./engine -> src/physics/engine.ts
    let src_physics = Path::new("src/physics/collision.ts");
    let resolved = resolve_module_import_path(src_physics, "./engine", &known_files, &aliases);
    assert_eq!(resolved, Some(PathBuf::from("src/physics/engine.ts")));

    // 2. Relative import from child component: ../physics/engine -> src/physics/engine.ts
    let src_component = Path::new("src/components/Canvas.tsx");
    let resolved2 = resolve_module_import_path(src_component, "../physics/engine", &known_files, &aliases);
    assert_eq!(resolved2, Some(PathBuf::from("src/physics/engine.ts")));

    // 3. Aliased import: @/types/simulation -> src/types/simulation.ts
    let resolved3 = resolve_module_import_path(src_component, "@/types/simulation", &known_files, &aliases);
    assert_eq!(resolved3, Some(PathBuf::from("src/types/simulation.ts")));

    // 4. Index file resolution: @/utils -> src/utils/index.ts
    let resolved4 = resolve_module_import_path(src_component, "@/utils", &known_files, &aliases);
    assert_eq!(resolved4, Some(PathBuf::from("src/utils/index.ts")));

    // 5. External package: 'three' -> None (not in known_files)
    let resolved5 = resolve_module_import_path(src_component, "three", &known_files, &aliases);
    assert_eq!(resolved5, None);
}

#[test]
fn test_live_graph_edges_for_typescript_project() {
    let temp_dir = std::env::temp_dir().join(format!("layer_ts_edges_{}", std::process::id()));
    let src_dir = temp_dir.join("src");
    let comp_dir = src_dir.join("components");
    let phys_dir = src_dir.join("physics");
    fs::create_dir_all(&comp_dir).unwrap();
    fs::create_dir_all(&phys_dir).unwrap();

    // 1. tsconfig.json with custom paths
    let tsconfig = r#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {
      "@/*": ["src/*"],
      "physics/*": ["src/physics/*"]
    }
  }
}
"#;
    fs::write(temp_dir.join("tsconfig.json"), tsconfig).unwrap();

    // 2. types.ts
    let types_code = r#"export interface Particle {
    x: number;
    y: number;
    z: number;
    vx: number;
    vy: number;
    vz: number;
}
"#;
    fs::write(src_dir.join("types.ts"), types_code).unwrap();

    // 3. engine.ts importing ./types
    let engine_code = r#"import { Particle } from '../types';

export function stepSimulation(particles: Particle[]): void {
    // update positions
}
"#;
    fs::write(phys_dir.join("engine.ts"), engine_code).unwrap();

    // 4. Canvas.tsx importing @/types and physics/engine
    let canvas_code = r#"import React from 'react';
import { Particle } from '@/types';
import { stepSimulation } from 'physics/engine';

export const Canvas: React.FC = () => {
    return <canvas />;
};
"#;
    fs::write(comp_dir.join("Canvas.tsx"), canvas_code).unwrap();

    // Run workspace sync
    let mut graph = VectorGraph::new();
    graph.sync_workspace(&temp_dir);

    assert!(graph.files.contains_key(&PathBuf::from("src/types.ts")));
    assert!(graph.files.contains_key(&PathBuf::from("src/physics/engine.ts")));
    assert!(graph.files.contains_key(&PathBuf::from("src/components/Canvas.tsx")));

    let types_buf = PathBuf::from("src/types.ts");
    let engine_buf = PathBuf::from("src/physics/engine.ts");
    let canvas_buf = PathBuf::from("src/components/Canvas.tsx");

    let types_idx = *graph.node_indices.get(&types_buf).expect("types.ts indexed");
    let engine_idx = *graph.node_indices.get(&engine_buf).expect("engine.ts indexed");
    let canvas_idx = *graph.node_indices.get(&canvas_buf).expect("Canvas.tsx indexed");

    // Check outgoing edge: engine.ts -> types.ts
    let engine_targets: Vec<_> = graph
        .graph
        .neighbors_directed(engine_idx, Direction::Outgoing)
        .collect();
    assert!(
        engine_targets.contains(&types_idx),
        "engine.ts must have dependency edge to types.ts"
    );

    // Check outgoing edges: Canvas.tsx -> types.ts AND Canvas.tsx -> engine.ts
    let canvas_targets: Vec<_> = graph
        .graph
        .neighbors_directed(canvas_idx, Direction::Outgoing)
        .collect();
    assert!(
        canvas_targets.contains(&types_idx),
        "Canvas.tsx must have dependency edge to types.ts via @/types"
    );
    assert!(
        canvas_targets.contains(&engine_idx),
        "Canvas.tsx must have dependency edge to engine.ts via physics/engine"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}
