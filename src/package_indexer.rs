use crate::scanner::AstScanner;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static TS_EXPORT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^[ \t]*export\s+(?:declare\s+)?(?:default\s+)?(type|interface|class|enum|const|function|let|var)\s+([a-zA-Z0-9_$]+)"#).unwrap()
});

static TS_REEXPORT_ALL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?m)^[ \t]*export\s+\*\s+from\s+['"]([^'"]+)['"]"#).unwrap());

static TS_REEXPORT_NAMED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^[ \t]*export\s*\{([^}]+)\}\s*from\s+['"]([^'"]+)['"]"#).unwrap()
});

/// A declaration extracted from an external package's type definitions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSymbolDecl {
    pub package: String,
    pub symbol_name: String,
    pub kind: String,
    pub signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_dts: Option<String>,
}

/// Compact index of an external package API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageApiSummary {
    pub package: String,
    pub total_symbols: usize,
    pub entry_point: String,
    pub symbols: Vec<ExternalSymbolDecl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_symbol: Option<ExternalSymbolDecl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_query_hint: Option<String>,
}

#[derive(Default, Debug)]
pub struct PackageIndexer {
    /// Cache: package_name -> list of extracted symbols
    cache: HashMap<String, Vec<ExternalSymbolDecl>>,
}

impl PackageIndexer {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Clear cache (e.g. when switching workspaces)
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    /// Inspect a package API in the workspace
    pub fn inspect_package(
        &mut self,
        workspace_root: &Path,
        package_name: &str,
        symbol_name: Option<&str>,
        query: Option<&str>,
    ) -> Result<PackageApiSummary, String> {
        let pkg_clean = package_name.trim();
        if pkg_clean.is_empty() {
            return Err("Package name cannot be empty.".to_string());
        }

        // Check if cached
        if !self.cache.contains_key(pkg_clean) {
            let extracted = Self::resolve_and_index_package(workspace_root, pkg_clean)?;
            self.cache.insert(pkg_clean.to_string(), extracted);
        }

        let all_symbols = self.cache.get(pkg_clean).cloned().unwrap_or_default();
        let total_symbols = all_symbols.len();

        let mut entry_point = "resolved-dts".to_string();
        if let Some(first) = all_symbols.first()
            && let Some(src) = &first.source_dts
        {
            entry_point = src.clone();
        }

        // If exact symbol requested
        if let Some(sym) = symbol_name {
            let sym_clean = sym.trim();
            if let Some(matched) = all_symbols.iter().find(|s| s.symbol_name == sym_clean) {
                return Ok(PackageApiSummary {
                    package: pkg_clean.to_string(),
                    total_symbols,
                    entry_point,
                    symbols: vec![matched.clone()],
                    requested_symbol: Some(matched.clone()),
                    next_query_hint: None,
                });
            } else {
                // Find close matches
                let sym_lower = sym_clean.to_lowercase();
                let prefix_len = sym_lower.len().min(4);
                let prefix = &sym_lower[..prefix_len];
                let candidates: Vec<String> = all_symbols
                    .iter()
                    .filter(|s| {
                        let name_lower = s.symbol_name.to_lowercase();
                        name_lower.contains(&sym_lower)
                            || sym_lower.contains(&name_lower)
                            || (!prefix.is_empty() && name_lower.starts_with(prefix))
                    })
                    .map(|s| s.symbol_name.clone())
                    .take(10)
                    .collect();

                let hint = if candidates.is_empty() {
                    format!(
                        "Symbol '{}' not found in package '{}'. Total exported symbols: {}.",
                        sym_clean, pkg_clean, total_symbols
                    )
                } else {
                    format!(
                        "Symbol '{}' not found. Close matches: {}",
                        sym_clean,
                        candidates.join(", ")
                    )
                };

                return Err(hint);
            }
        }

        // If search query is provided
        let filtered: Vec<ExternalSymbolDecl> = if let Some(q) = query {
            let q_lower = q.to_lowercase();
            all_symbols
                .into_iter()
                .filter(|s| {
                    s.symbol_name.to_lowercase().contains(&q_lower)
                        || s.signature.to_lowercase().contains(&q_lower)
                        || s.doc_comment
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&q_lower)
                })
                .take(30)
                .collect()
        } else {
            all_symbols.into_iter().take(40).collect()
        };

        let hint = if filtered.len() < total_symbols {
            Some(format!(
                "Showing {} of {} exported symbols. Query with symbol_name='<Symbol>' for full declaration.",
                filtered.len(),
                total_symbols
            ))
        } else {
            None
        };

        Ok(PackageApiSummary {
            package: pkg_clean.to_string(),
            total_symbols,
            entry_point,
            symbols: filtered,
            requested_symbol: None,
            next_query_hint: hint,
        })
    }

    /// Resolve and index package from disk, checking workspace, parent, sibling node_modules, or built-in fallbacks
    fn resolve_and_index_package(
        workspace_root: &Path,
        package_name: &str,
    ) -> Result<Vec<ExternalSymbolDecl>, String> {
        let pkg_clean = package_name.trim();

        // 1. Try finding package directory in workspace, parent directories, or sibling directories
        let pkg_dir = Self::find_package_dir(workspace_root, pkg_clean);
        let types_pkg_dir = Self::find_types_package_dir(workspace_root, pkg_clean);

        let dts_entry = pkg_dir
            .as_deref()
            .and_then(Self::find_dts_entry)
            .or_else(|| types_pkg_dir.as_deref().and_then(Self::find_dts_entry));

        if let Some(entry_path) = dts_entry {
            let mut symbols = Vec::new();
            let mut visited_paths = std::collections::HashSet::new();

            if Self::crawl_dts_file(&entry_path, pkg_clean, &mut visited_paths, &mut symbols, 0)
                .is_ok()
                && !symbols.is_empty()
            {
                // Deduplicate symbols by name
                symbols.sort_by(|a, b| a.symbol_name.cmp(&b.symbol_name));
                symbols.dedup_by(|a, b| a.symbol_name == b.symbol_name);
                return Ok(symbols);
            }
        }

        // 2. If this is a Rust project, try resolving crate source
        if workspace_root.join("Cargo.toml").exists()
            && let Ok(crate_syms) = Self::resolve_rust_crate(workspace_root, pkg_clean)
            && !crate_syms.is_empty()
        {
            return Ok(crate_syms);
        }

        // 3. Try resolving local C/C++ header in workspace
        if let Ok(cpp_syms) = Self::resolve_cpp_header(workspace_root, pkg_clean)
            && !cpp_syms.is_empty()
        {
            return Ok(cpp_syms);
        }

        // 4. Built-in fallback signatures for common greenfield libraries
        if let Some(fallback_symbols) = Self::get_builtin_package_fallback(pkg_clean) {
            return Ok(fallback_symbols);
        }

        Err(format!(
            "Cannot find type declarations or package definitions for '{}'. Ensure dependencies are installed or specify a supported package.",
            pkg_clean
        ))
    }

    /// Search for a package directory in workspace node_modules, parent directories, or sibling directories
    fn find_package_dir(workspace_root: &Path, package_name: &str) -> Option<PathBuf> {
        // 1. Direct workspace node_modules
        let direct = workspace_root.join("node_modules").join(package_name);
        if direct.is_dir() {
            return Some(direct);
        }

        // 2. Walk up parent directories (e.g. monorepo root or parent workspace)
        let mut curr = workspace_root.parent();
        while let Some(parent) = curr {
            let candidate = parent.join("node_modules").join(package_name);
            if candidate.is_dir() {
                return Some(candidate);
            }
            curr = parent.parent();
        }

        // 3. Sibling directories of workspace_root
        if let Some(parent) = workspace_root.parent()
            && let Ok(entries) = fs::read_dir(parent)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path != workspace_root {
                    let candidate = path.join("node_modules").join(package_name);
                    if candidate.is_dir() {
                        return Some(candidate);
                    }
                }
            }
        }

        None
    }

    /// Search for @types/<package_name> directory in workspace, parent, or sibling node_modules
    fn find_types_package_dir(workspace_root: &Path, package_name: &str) -> Option<PathBuf> {
        let types_subpath = if package_name.starts_with('@') {
            let replaced = package_name
                .strip_prefix('@')
                .unwrap_or(package_name)
                .replace('/', "__");
            format!("@types/{}", replaced)
        } else {
            format!("@types/{}", package_name)
        };
        Self::find_package_dir(workspace_root, &types_subpath)
    }

    /// Return accurate built-in fallback TypeScript declarations for common greenfield packages
    /// when node_modules is missing or npm install was not run.
    pub fn get_builtin_package_fallback(package_name: &str) -> Option<Vec<ExternalSymbolDecl>> {
        let clean = package_name.trim().trim_matches('"').trim_matches('\'');
        let clean_lower = clean.to_lowercase();
        let decl = |pkg: &str, sym: &str, kind: &str, sig: &str, doc: &str| ExternalSymbolDecl {
            package: pkg.to_string(),
            symbol_name: sym.to_string(),
            kind: kind.to_string(),
            signature: sig.to_string(),
            doc_comment: Some(doc.to_string()),
            source_dts: Some(format!("{}/index.d.ts (built-in fallback)", pkg)),
        };

        match clean_lower.as_str() {
            "@react-three/fiber" | "react-three-fiber" | "@types/react-three-fiber" => Some(vec![
                decl(
                    "@react-three/fiber",
                    "Canvas",
                    "component",
                    "export const Canvas: React.ForwardRefExoticComponent<CanvasProps & React.RefAttributes<HTMLCanvasElement>>;",
                    "Top-level React Three Fiber component that manages a Three.js WebGLRenderer scene graph inside an HTML canvas element.",
                ),
                decl(
                    "@react-three/fiber",
                    "CanvasProps",
                    "interface",
                    "export interface CanvasProps extends Omit<React.HTMLAttributes<HTMLDivElement>, 'children' | 'ref'> {\n  children?: React.ReactNode;\n  camera?: { position?: [number, number, number]; fov?: number; near?: number; far?: number };\n  gl?: THREE.WebGLRendererParameters;\n  shadows?: boolean | 'basic' | 'percentage' | 'soft' | 'variance';\n  dpr?: number | [number, number];\n  frameloop?: 'always' | 'demand' | 'never';\n  onCreated?: (state: RootState) => void;\n}",
                    "Properties for the Canvas component including camera, gl, shadows, and lifecycle hooks.",
                ),
                decl(
                    "@react-three/fiber",
                    "useFrame",
                    "function",
                    "export function useFrame(callback: (state: RootState, delta: number, xrFrame?: any) => void, renderPriority?: number): void;",
                    "Executes every frame on the Three.js render loop. Use delta for delta-time physics and animation updates.",
                ),
                decl(
                    "@react-three/fiber",
                    "useThree",
                    "function",
                    "export function useThree<T = RootState>(selector?: (state: RootState) => T, equalityFn?: (a: T, b: T) => boolean): T;",
                    "Returns the current Three.js RootState containing scene, camera, gl, size, viewport, and clock.",
                ),
                decl(
                    "@react-three/fiber",
                    "extend",
                    "function",
                    "export function extend(objects: Record<string, any>): void;",
                    "Registers arbitrary Three.js objects as valid JSX elements in the scene graph.",
                ),
                decl(
                    "@react-three/fiber",
                    "RootState",
                    "interface",
                    "export interface RootState {\n  scene: THREE.Scene;\n  camera: THREE.Camera;\n  gl: THREE.WebGLRenderer;\n  size: { width: number; height: number; top: number; left: number };\n  viewport: { width: number; height: number; factor: number; distance: number; aspect: number };\n  clock: THREE.Clock;\n  invalidate: () => void;\n  advance: (timestamp: number, runGlobalEffects?: boolean) => void;\n}",
                    "The root state object accessible via useThree() and useFrame().",
                ),
            ]),
            "@react-three/drei" | "react-three-drei" | "@types/react-three-drei" => Some(vec![
                decl(
                    "@react-three/drei",
                    "OrbitControls",
                    "component",
                    "export const OrbitControls: React.ForwardRefExoticComponent<OrbitControlsProps & React.RefAttributes<any>>;",
                    "Camera orbit controls helper with smooth damping, rotation, and zoom boundaries.",
                ),
                decl(
                    "@react-three/drei",
                    "OrbitControlsProps",
                    "interface",
                    "export interface OrbitControlsProps {\n  enableDamping?: boolean;\n  dampingFactor?: number;\n  enableZoom?: boolean;\n  enablePan?: boolean;\n  enableRotate?: boolean;\n  autoRotate?: boolean;\n  autoRotateSpeed?: number;\n  minDistance?: number;\n  maxDistance?: number;\n  minPolarAngle?: number;\n  maxPolarAngle?: number;\n  target?: [number, number, number] | THREE.Vector3;\n}",
                    "Configuration properties for OrbitControls.",
                ),
                decl(
                    "@react-three/drei",
                    "Stars",
                    "component",
                    "export const Stars: React.FC<{ radius?: number; depth?: number; count?: number; factor?: number; saturation?: number; fade?: boolean; speed?: number }>;",
                    "Renders procedural deep-space twinkling star particles.",
                ),
                decl(
                    "@react-three/drei",
                    "Line",
                    "component",
                    "export const Line: React.ForwardRefExoticComponent<{ points: Array<[number, number, number] | THREE.Vector3>; color?: string | number; lineWidth?: number; dashed?: boolean } & React.RefAttributes<any>>;",
                    "Renders fat lines, trajectories, and orbits with configurable thickness and color.",
                ),
                decl(
                    "@react-three/drei",
                    "Html",
                    "component",
                    "export const Html: React.ForwardRefExoticComponent<HtmlProps & React.RefAttributes<HTMLDivElement>>;",
                    "Projects 2D HTML/DOM overlay elements (labels, tooltips, HUD controls) into 3D world space coordinates.",
                ),
                decl(
                    "@react-three/drei",
                    "HtmlProps",
                    "interface",
                    "export interface HtmlProps {\n  position?: [number, number, number] | THREE.Vector3;\n  center?: boolean;\n  distanceFactor?: number;\n  zIndexRange?: [number, number];\n  occlude?: boolean | React.RefObject<THREE.Object3D>[];\n  children?: React.ReactNode;\n}",
                    "Properties for Html 3D projection overlay.",
                ),
                decl(
                    "@react-three/drei",
                    "PerspectiveCamera",
                    "component",
                    "export const PerspectiveCamera: React.ForwardRefExoticComponent<{ makeDefault?: boolean; fov?: number; position?: [number, number, number]; near?: number; far?: number } & React.RefAttributes<THREE.PerspectiveCamera>>;",
                    "Declarative PerspectiveCamera that can be marked as default viewport camera.",
                ),
                decl(
                    "@react-three/drei",
                    "Text",
                    "component",
                    "export const Text: React.ForwardRefExoticComponent<{ children: string; color?: string; fontSize?: number; maxWidth?: number; lineHeight?: number; letterSpacing?: number; textAlign?: 'left' | 'right' | 'center' | 'justify'; anchorX?: number | 'left' | 'center' | 'right'; anchorY?: number | 'top' | 'top-baseline' | 'middle' | 'bottom-baseline' | 'bottom' } & React.RefAttributes<any>>;",
                    "High-quality SDF text rendering in 3D canvas.",
                ),
            ]),
            "three" | "@types/three" => Some(vec![
                decl(
                    "three",
                    "Vector3",
                    "class",
                    "export class Vector3 {\n  x: number;\n  y: number;\n  z: number;\n  constructor(x?: number, y?: number, z?: number);\n  set(x: number, y: number, z: number): this;\n  clone(): this;\n  copy(v: Vector3): this;\n  add(v: Vector3): this;\n  sub(v: Vector3): this;\n  multiplyScalar(s: number): this;\n  divideScalar(s: number): this;\n  length(): number;\n  lengthSq(): number;\n  distanceTo(v: Vector3): number;\n  distanceToSquared(v: Vector3): number;\n  normalize(): this;\n  dot(v: Vector3): number;\n  cross(v: Vector3): this;\n}",
                    "3D vector class with vector math operations (add, sub, multiplyScalar, normalize, distanceTo).",
                ),
                decl(
                    "three",
                    "Mesh",
                    "class",
                    "export class Mesh<TGeometry extends BufferGeometry = BufferGeometry, TMaterial extends Material | Material[] = Material | Material[]> extends Object3D {\n  constructor(geometry?: TGeometry, material?: TMaterial);\n  geometry: TGeometry;\n  material: TMaterial;\n}",
                    "Class representing polygonal 3D mesh objects made of geometry and material.",
                ),
                decl(
                    "three",
                    "Group",
                    "class",
                    "export class Group extends Object3D {\n  constructor();\n  readonly isGroup: true;\n  type: 'Group';\n}",
                    "Container object for grouping multiple 3D objects together in a scene hierarchy.",
                ),
                decl(
                    "three",
                    "InstancedMesh",
                    "class",
                    "export class InstancedMesh<TGeometry extends BufferGeometry = BufferGeometry, TMaterial extends Material | Material[] = Material | Material[]> extends Mesh<TGeometry, TMaterial> {\n  constructor(geometry: TGeometry, material: TMaterial, count: number);\n  count: number;\n  instanceMatrix: InstancedBufferAttribute;\n  instanceColor: InstancedBufferAttribute | null;\n  setMatrixAt(index: number, matrix: Matrix4): void;\n  getMatrixAt(index: number, matrix: Matrix4): void;\n  setColorAt(index: number, color: Color): void;\n  getColorAt(index: number, color: Color): void;\n}",
                    "High-performance instanced rendering of many objects sharing geometry and material with 1 draw call.",
                ),
                decl(
                    "three",
                    "Color",
                    "class",
                    "export class Color {\n  r: number;\n  g: number;\n  b: number;\n  constructor(r?: number | string | Color, g?: number, b?: number);\n  set(value: Color | string | number): this;\n  setRGB(r: number, g: number, b: number): this;\n  setHex(hex: number): this;\n}",
                    "Class representing RGB colors in Three.js.",
                ),
                decl(
                    "three",
                    "SphereGeometry",
                    "class",
                    "export class SphereGeometry extends BufferGeometry {\n  constructor(radius?: number, widthSegments?: number, heightSegments?: number, phiStart?: number, phiLength?: number, thetaStart?: number, thetaLength?: number);\n}",
                    "Standard sphere buffer geometry.",
                ),
                decl(
                    "three",
                    "BoxGeometry",
                    "class",
                    "export class BoxGeometry extends BufferGeometry {\n  constructor(width?: number, height?: number, depth?: number, widthSegments?: number, heightSegments?: number, depthSegments?: number);\n}",
                    "Standard box buffer geometry.",
                ),
                decl(
                    "three",
                    "BufferGeometry",
                    "class",
                    "export class BufferGeometry {\n  constructor();\n  dispose(): void;\n  setAttribute(name: string, attribute: BufferAttribute): this;\n  computeVertexNormals(): void;\n}",
                    "Base class for all geometric data in Three.js.",
                ),
                decl(
                    "three",
                    "MeshStandardMaterial",
                    "class",
                    "export class MeshStandardMaterial extends Material {\n  constructor(parameters?: MeshStandardMaterialParameters);\n  color: Color;\n  roughness: number;\n  metalness: number;\n  emissive: Color;\n  wireframe: boolean;\n}",
                    "Physically based rendering (PBR) standard material.",
                ),
                decl(
                    "three",
                    "MeshBasicMaterial",
                    "class",
                    "export class MeshBasicMaterial extends Material {\n  constructor(parameters?: MeshBasicMaterialParameters);\n  color: Color;\n  wireframe: boolean;\n}",
                    "Simple unlit material for non-reflective objects.",
                ),
                decl(
                    "three",
                    "Scene",
                    "class",
                    "export class Scene extends Object3D {\n  constructor();\n  background: Color | Texture | null;\n  environment: Texture | null;\n}",
                    "Top-level scene graph container in Three.js.",
                ),
                decl(
                    "three",
                    "Clock",
                    "class",
                    "export class Clock {\n  constructor(autoStart?: boolean);\n  autoStart: boolean;\n  startTime: number;\n  oldTime: number;\n  elapsedTime: number;\n  running: boolean;\n  start(): void;\n  stop(): void;\n  getElapsedTime(): number;\n  getDelta(): number;\n}",
                    "Object for keeping track of time and delta increments.",
                ),
            ]),
            "lucide-react" | "lucide" => Some(vec![
                decl(
                    "lucide-react",
                    "Play",
                    "component",
                    "export const Play: React.ForwardRefExoticComponent<LucideProps & React.RefAttributes<SVGSVGElement>>;",
                    "Play icon component from lucide-react.",
                ),
                decl(
                    "lucide-react",
                    "Pause",
                    "component",
                    "export const Pause: React.ForwardRefExoticComponent<LucideProps & React.RefAttributes<SVGSVGElement>>;",
                    "Pause icon component from lucide-react.",
                ),
                decl(
                    "lucide-react",
                    "RotateCcw",
                    "component",
                    "export const RotateCcw: React.ForwardRefExoticComponent<LucideProps & React.RefAttributes<SVGSVGElement>>;",
                    "Reset / rotate counter-clockwise icon component from lucide-react.",
                ),
                decl(
                    "lucide-react",
                    "Settings",
                    "component",
                    "export const Settings: React.ForwardRefExoticComponent<LucideProps & React.RefAttributes<SVGSVGElement>>;",
                    "Settings gear icon component from lucide-react.",
                ),
                decl(
                    "lucide-react",
                    "Plus",
                    "component",
                    "export const Plus: React.ForwardRefExoticComponent<LucideProps & React.RefAttributes<SVGSVGElement>>;",
                    "Plus / add icon component from lucide-react.",
                ),
                decl(
                    "lucide-react",
                    "Minus",
                    "component",
                    "export const Minus: React.ForwardRefExoticComponent<LucideProps & React.RefAttributes<SVGSVGElement>>;",
                    "Minus / subtract icon component from lucide-react.",
                ),
                decl(
                    "lucide-react",
                    "Info",
                    "component",
                    "export const Info: React.ForwardRefExoticComponent<LucideProps & React.RefAttributes<SVGSVGElement>>;",
                    "Information icon component from lucide-react.",
                ),
                decl(
                    "lucide-react",
                    "Sliders",
                    "component",
                    "export const Sliders: React.ForwardRefExoticComponent<LucideProps & React.RefAttributes<SVGSVGElement>>;",
                    "Sliders / parameter adjustment icon component from lucide-react.",
                ),
                decl(
                    "lucide-react",
                    "Eye",
                    "component",
                    "export const Eye: React.ForwardRefExoticComponent<LucideProps & React.RefAttributes<SVGSVGElement>>;",
                    "Eye / visibility toggle icon component from lucide-react.",
                ),
                decl(
                    "lucide-react",
                    "RefreshCw",
                    "component",
                    "export const RefreshCw: React.ForwardRefExoticComponent<LucideProps & React.RefAttributes<SVGSVGElement>>;",
                    "Refresh clockwise icon component from lucide-react.",
                ),
                decl(
                    "lucide-react",
                    "Trash2",
                    "component",
                    "export const Trash2: React.ForwardRefExoticComponent<LucideProps & React.RefAttributes<SVGSVGElement>>;",
                    "Trash / delete icon component from lucide-react.",
                ),
                decl(
                    "lucide-react",
                    "LucideProps",
                    "interface",
                    "export interface LucideProps extends React.SVGProps<SVGSVGElement> {\n  size?: string | number;\n  color?: string;\n  strokeWidth?: string | number;\n}",
                    "Common properties for Lucide React icons.",
                ),
            ]),
            "react" | "@types/react" => Some(vec![
                decl(
                    "react",
                    "useState",
                    "function",
                    "export function useState<S>(initialState: S | (() => S)): [S, (value: S | ((prevState: S) => S)) => void];",
                    "Returns a stateful value and a function to update it.",
                ),
                decl(
                    "react",
                    "useEffect",
                    "function",
                    "export function useEffect(effect: () => void | (() => void), deps?: readonly any[]): void;",
                    "Accepts a function that contains imperative, possibly effectful code.",
                ),
                decl(
                    "react",
                    "useRef",
                    "function",
                    "export function useRef<T>(initialValue: T): { current: T };\nexport function useRef<T = undefined>(): { current: T | undefined };",
                    "Returns a mutable ref object whose .current property is initialized to the passed argument.",
                ),
                decl(
                    "react",
                    "useMemo",
                    "function",
                    "export function useMemo<T>(factory: () => T, deps: readonly any[] | undefined): T;",
                    "Returns a memoized value calculated by factory function.",
                ),
                decl(
                    "react",
                    "useCallback",
                    "function",
                    "export function useCallback<T extends (...args: any[]) => any>(callback: T, deps: readonly any[]): T;",
                    "Returns a memoized version of the callback function that only changes if dependencies change.",
                ),
                decl(
                    "react",
                    "FC",
                    "type",
                    "export type FC<P = {}> = React.FunctionComponent<P>;",
                    "Functional component type definition.",
                ),
                decl(
                    "react",
                    "ReactNode",
                    "type",
                    "export type ReactNode = React.ReactChild | React.ReactFragment | React.ReactPortal | boolean | null | undefined;",
                    "Represents all possible React renderable children.",
                ),
            ]),
            "fastapi" => Some(vec![
                decl(
                    "fastapi",
                    "FastAPI",
                    "class",
                    "class FastAPI(Starlette):\n    def __init__(self, debug: bool = False, title: str = 'FastAPI', version: str = '0.1.0') -> None",
                    "Main FastAPI application class providing routing, middleware, and dependency injection.",
                ),
                decl(
                    "fastapi",
                    "APIRouter",
                    "class",
                    "class APIRouter:\n    def __init__(self, prefix: str = '', tags: list[str] | None = None) -> None",
                    "Modular sub-router for organizing FastAPI endpoint routes.",
                ),
                decl(
                    "fastapi",
                    "Depends",
                    "function",
                    "def Depends(dependency: Callable[..., Any] | None = None, *, use_cache: bool = True) -> Any",
                    "Declares a FastAPI dependency parameter.",
                ),
                decl(
                    "fastapi",
                    "HTTPException",
                    "class",
                    "class HTTPException(Exception):\n    def __init__(self, status_code: int, detail: Any = None, headers: dict[str, str] | None = None) -> None",
                    "HTTP exception carrying HTTP status code and JSON error payload.",
                ),
            ]),
            "pydantic" => Some(vec![
                decl(
                    "pydantic",
                    "BaseModel",
                    "class",
                    "class BaseModel:\n    def model_dump(self, **kwargs) -> dict[str, Any]: ...\n    def model_dump_json(self, **kwargs) -> str: ...",
                    "Base data validation and parsing class in Pydantic v2.",
                ),
                decl(
                    "pydantic",
                    "Field",
                    "function",
                    "def Field(default: Any = ..., *, default_factory: Callable[[], Any] | None = None, description: str | None = None, ge: float | None = None, le: float | None = None) -> Any",
                    "Specifies metadata, constraints, and descriptions for model fields.",
                ),
                decl(
                    "pydantic",
                    "ConfigDict",
                    "type",
                    "ConfigDict = TypedDict('ConfigDict', {'extra': str, 'frozen': bool, 'populate_by_name': bool}, total=False)",
                    "Configuration dictionary for Pydantic v2 models.",
                ),
            ]),
            "numpy" => Some(vec![
                decl(
                    "numpy",
                    "ndarray",
                    "class",
                    "class ndarray[Shape, DType]:\n    shape: tuple[int, ...]\n    dtype: DType\n    def reshape(self, *shape: int) -> ndarray: ...",
                    "N-dimensional homogeneous array container.",
                ),
                decl(
                    "numpy",
                    "array",
                    "function",
                    "def array(object: Any, dtype: DTypeLike | None = None, copy: bool = True) -> ndarray",
                    "Creates a numpy ndarray from an array-like object.",
                ),
                decl(
                    "numpy",
                    "zeros",
                    "function",
                    "def zeros(shape: int | tuple[int, ...], dtype: DTypeLike = float) -> ndarray",
                    "Returns an array of given shape and type filled with zeros.",
                ),
                decl(
                    "numpy",
                    "ones",
                    "function",
                    "def ones(shape: int | tuple[int, ...], dtype: DTypeLike = float) -> ndarray",
                    "Returns an array of given shape and type filled with ones.",
                ),
            ]),
            "torch" => Some(vec![
                decl(
                    "torch",
                    "Tensor",
                    "class",
                    "class Tensor:\n    shape: Size\n    device: device\n    dtype: dtype\n    def backward(self, gradient: Tensor | None = None) -> None: ...\n    def to(self, device: device | str) -> Tensor: ...",
                    "Multi-dimensional matrix containing elements of a single data type with autograd support.",
                ),
                decl(
                    "torch",
                    "zeros",
                    "function",
                    "def zeros(*size: int, dtype: dtype | None = None, device: device | str | None = None) -> Tensor",
                    "Returns a tensor filled with the scalar value 0.",
                ),
            ]),
            "tokio" => Some(vec![
                decl(
                    "tokio",
                    "main",
                    "macro",
                    "#[tokio::main]\npub async fn main()",
                    "Marks async main function to run on the multi-threaded Tokio runtime.",
                ),
                decl(
                    "tokio",
                    "spawn",
                    "function",
                    "pub fn spawn<T>(future: T) -> JoinHandle<T::Output>\nwhere\n    T: Future + Send + 'static,\n    T::Output: Send + 'static,",
                    "Spawns a new asynchronous task, returning a JoinHandle.",
                ),
                decl(
                    "tokio",
                    "sleep",
                    "function",
                    "pub async fn sleep(duration: Duration)",
                    "Waits until the given duration has elapsed without blocking the runtime thread.",
                ),
            ]),
            "serde" => Some(vec![
                decl(
                    "serde",
                    "Serialize",
                    "trait",
                    "pub trait Serialize {\n    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>\n    where\n        S: Serializer;\n}",
                    "Trait for serializing data structures into formats supported by Serde.",
                ),
                decl(
                    "serde",
                    "Deserialize",
                    "trait",
                    "pub trait Deserialize<'de>: Sized {\n    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>\n    where\n        D: Deserializer<'de>;\n}",
                    "Trait for deserializing data structures from formats supported by Serde.",
                ),
            ]),
            "axum" => Some(vec![
                decl(
                    "axum",
                    "Router",
                    "struct",
                    "pub struct Router<S = ()> {\n    pub fn new() -> Self;\n    pub fn route(self, path: &str, method_router: MethodRouter<S>) -> Self;\n    pub fn with_state<S2>(self, state: S) -> Router<S2>;\n}",
                    "Top-level web routing container in Axum.",
                ),
                decl(
                    "axum",
                    "Json",
                    "struct",
                    "pub struct Json<T>(pub T);\nimpl<T> IntoResponse for Json<T> where T: Serialize;\nimpl<T, S> FromRequest<S> for Json<T> where T: DeserializeOwned;",
                    "Extractor and response type that serializes/deserializes JSON payloads.",
                ),
            ]),
            "gin" | "github.com/gin-gonic/gin" => Some(vec![
                decl(
                    "gin",
                    "Default",
                    "function",
                    "func Default() *Engine",
                    "Returns an Engine instance with Logger and Recovery middleware already attached.",
                ),
                decl(
                    "gin",
                    "Context",
                    "struct",
                    "type Context struct {\n    func (c *Context) JSON(code int, obj any)\n    func (c *Context) Param(key string) string\n    func (c *Context) BindJSON(obj any) error\n}",
                    "Context is the most important part of gin. It allows passing variables between middleware, manage the flow, and validate JSON.",
                ),
                decl(
                    "gin",
                    "H",
                    "type",
                    "type H map[string]any",
                    "H is a shortcut for map[string]any.",
                ),
            ]),
            "nlohmann/json" | "json" => Some(vec![
                decl(
                    "nlohmann/json",
                    "json",
                    "class",
                    "using json = nlohmann::basic_json<>;",
                    "C++ JSON container from nlohmann/json with intuitive syntax, parsing, and serialization.",
                ),
                decl(
                    "nlohmann/json",
                    "parse",
                    "function",
                    "static json parse(const string& s);",
                    "Parses JSON string into a json instance.",
                ),
            ]),
            "microsoft.aspnetcore" | "microsoft.aspnetcore.mvc" | "aspnetcore" => Some(vec![
                decl(
                    "Microsoft.AspNetCore.Mvc",
                    "ControllerBase",
                    "class",
                    "public abstract class ControllerBase",
                    "A base class for an MVC controller without view support.",
                ),
                decl(
                    "Microsoft.AspNetCore.Mvc",
                    "HttpGetAttribute",
                    "class",
                    "public class HttpGetAttribute : HttpMethodAttribute",
                    "Identifies an action that supports the HTTP GET method.",
                ),
                decl(
                    "Microsoft.AspNetCore.Mvc",
                    "HttpPostAttribute",
                    "class",
                    "public class HttpPostAttribute : HttpMethodAttribute",
                    "Identifies an action that supports the HTTP POST method.",
                ),
                decl(
                    "Microsoft.AspNetCore.Mvc",
                    "IActionResult",
                    "interface",
                    "public interface IActionResult",
                    "Defines a contract that represents the result of an action method.",
                ),
            ]),
            "system.text.json" => Some(vec![
                decl(
                    "System.Text.Json",
                    "JsonSerializer",
                    "class",
                    "public static class JsonSerializer",
                    "Provides high-performance functionality to serialize objects or value types to JSON and deserialize JSON into objects or value types.",
                ),
                decl(
                    "System.Text.Json",
                    "Serialize",
                    "method",
                    "public static string Serialize<TValue>(TValue value, JsonSerializerOptions? options = null);",
                    "Converts the value of a type specified by a generic type parameter into a JSON string.",
                ),
            ]),
            "org.springframework.boot" | "spring-boot" | "springboot" => Some(vec![
                decl(
                    "org.springframework.boot",
                    "SpringBootApplication",
                    "annotation",
                    "@SpringBootApplication public @interface SpringBootApplication",
                    "Indicates a configuration class that declares one or more @Bean methods and triggers auto-configuration.",
                ),
                decl(
                    "org.springframework.boot",
                    "SpringApplication",
                    "class",
                    "public class SpringApplication",
                    "Class that can be used to bootstrap and launch a Spring application from a Java main method.",
                ),
            ]),
            "org.springframework.web.bind.annotation"
            | "spring-web"
            | "spring-boot-starter-web" => Some(vec![
                decl(
                    "org.springframework.web.bind.annotation",
                    "RestController",
                    "annotation",
                    "@RestController public @interface RestController",
                    "A convenience annotation that is itself annotated with @Controller and @ResponseBody.",
                ),
                decl(
                    "org.springframework.web.bind.annotation",
                    "GetMapping",
                    "annotation",
                    "@GetMapping public @interface GetMapping",
                    "Annotation for mapping HTTP GET requests onto specific handler methods.",
                ),
                decl(
                    "org.springframework.web.bind.annotation",
                    "PostMapping",
                    "annotation",
                    "@PostMapping public @interface PostMapping",
                    "Annotation for mapping HTTP POST requests onto specific handler methods.",
                ),
            ]),
            "next" | "next/link" | "next/image" | "next/navigation" => Some(vec![
                decl(
                    "next/link",
                    "Link",
                    "function",
                    "export default function Link(props: LinkProps & React.AnchorHTMLAttributes<HTMLAnchorElement>): JSX.Element;",
                    "Primary client-side navigation component in Next.js.",
                ),
                decl(
                    "next/link",
                    "LinkProps",
                    "interface",
                    "export interface LinkProps {\n  href: string | any;\n  as?: string | any;\n  replace?: boolean;\n  scroll?: boolean;\n  shallow?: boolean;\n  passHref?: boolean;\n  prefetch?: boolean;\n  locale?: string | false;\n}",
                    "Props for Next.js Link component.",
                ),
                decl(
                    "next/image",
                    "Image",
                    "function",
                    "export default function Image(props: ImageProps): JSX.Element;",
                    "Automatic image optimization component in Next.js.",
                ),
                decl(
                    "next/image",
                    "ImageProps",
                    "interface",
                    "export interface ImageProps extends Omit<React.ImgHTMLAttributes<HTMLImageElement>, 'src' | 'width' | 'height'> {\n  src: string | any;\n  width?: number | `${number}`;\n  height?: number | `${number}`;\n  fill?: boolean;\n  alt: string;\n  priority?: boolean;\n  quality?: number | `${number}`;\n  placeholder?: 'blur' | 'empty';\n}",
                    "Props for Next.js Image component.",
                ),
                decl(
                    "next/navigation",
                    "useRouter",
                    "function",
                    "export function useRouter(): AppRouterInstance;",
                    "Hook to access the App Router navigation instance.",
                ),
                decl(
                    "next/navigation",
                    "usePathname",
                    "function",
                    "export function usePathname(): string;",
                    "Returns the current URL pathname.",
                ),
                decl(
                    "next/navigation",
                    "useSearchParams",
                    "function",
                    "export function useSearchParams(): ReadonlyURLSearchParams;",
                    "Returns the current URL search parameters.",
                ),
                decl(
                    "next/navigation",
                    "useParams",
                    "function",
                    "export function useParams<T = Record<string, string | string[]>>(): T;",
                    "Returns dynamic route parameters from the current URL.",
                ),
                decl(
                    "next/navigation",
                    "redirect",
                    "function",
                    "export function redirect(url: string, type?: RedirectType): never;",
                    "Programmatically redirects to another route.",
                ),
                decl(
                    "next/navigation",
                    "notFound",
                    "function",
                    "export function notFound(): never;",
                    "Renders the Next.js 404 Not Found UI boundary.",
                ),
                decl(
                    "next",
                    "Metadata",
                    "interface",
                    "export interface Metadata {\n  title?: string | { default: string; template: string };\n  description?: string;\n  keywords?: string[] | string;\n  openGraph?: Record<string, any>;\n  twitter?: Record<string, any>;\n  icons?: any;\n}",
                    "Static or dynamic page metadata configuration for App Router.",
                ),
                decl(
                    "next",
                    "NextConfig",
                    "interface",
                    "export interface NextConfig {\n  reactStrictMode?: boolean;\n  images?: { domains?: string[]; remotePatterns?: Array<any> };\n  experimental?: Record<string, any>;\n}",
                    "Next.js configuration options interface (next.config.ts).",
                ),
            ]),
            "tailwindcss" | "@tailwindcss/postcss" => Some(vec![
                decl(
                    "tailwindcss",
                    "Config",
                    "interface",
                    "export interface Config {\n  content: string[];\n  theme?: { extend?: Record<string, any> };\n  plugins?: any[];\n  darkMode?: 'media' | 'class' | ['class', string];\n}",
                    "Tailwind CSS configuration schema.",
                ),
                decl(
                    "tailwindcss",
                    "PluginCreator",
                    "type",
                    "export type PluginCreator = (api: { addUtilities: Function; addComponents: Function; addBase: Function; theme: Function }) => void;",
                    "Signature for custom Tailwind CSS plugins.",
                ),
            ]),
            "framer-motion" => Some(vec![
                decl(
                    "framer-motion",
                    "motion",
                    "const",
                    "export const motion: { div: React.ForwardRefExoticComponent<any>; button: React.ForwardRefExoticComponent<any>; span: React.ForwardRefExoticComponent<any>; [key: string]: any };",
                    "Component factory for creating animated HTML and SVG DOM elements.",
                ),
                decl(
                    "framer-motion",
                    "AnimatePresence",
                    "component",
                    "export const AnimatePresence: React.FC<{ children?: React.ReactNode; initial?: boolean; mode?: 'sync' | 'wait' | 'popLayout'; onExitComplete?: () => void }>;",
                    "Renders components with enter and exit animations when removed from React tree.",
                ),
                decl(
                    "framer-motion",
                    "useAnimation",
                    "function",
                    "export function useAnimation(): AnimationControls;",
                    "Hook to create manual animation controls.",
                ),
                decl(
                    "framer-motion",
                    "useMotionValue",
                    "function",
                    "export function useMotionValue<T>(initial: T): MotionValue<T>;",
                    "Creates a reactive MotionValue that updates without re-rendering React.",
                ),
                decl(
                    "framer-motion",
                    "useTransform",
                    "function",
                    "export function useTransform<I, O>(value: MotionValue<I>, inputRange: I[], outputRange: O[]): MotionValue<O>;",
                    "Creates a transformed MotionValue by mapping input range to output range.",
                ),
            ]),
            "zustand" => Some(vec![
                decl(
                    "zustand",
                    "create",
                    "function",
                    "export const create: <T>(initializer: (set: (partial: Partial<T> | ((state: T) => Partial<T>)) => void, get: () => T, store: any) => T) => () => T;",
                    "Primary store creation function in Zustand.",
                ),
                decl(
                    "zustand",
                    "StateCreator",
                    "type",
                    "export type StateCreator<T, Mis = [], Mos = [], U = T> = (set: (partial: Partial<T> | ((state: T) => Partial<T>)) => void, get: () => T, api: any) => U;",
                    "Type helper for creating reusable store slices in Zustand.",
                ),
            ]),
            _ => None,
        }
    }

    /// Find .d.ts entry point for a package
    fn find_dts_entry(pkg_dir: &Path) -> Option<PathBuf> {
        if !pkg_dir.exists() {
            return None;
        }

        let pkg_json = pkg_dir.join("package.json");
        if pkg_json.exists()
            && let Ok(content) = fs::read_to_string(&pkg_json)
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&content)
        {
            // Check "types" or "typings"
            if let Some(types) = v
                .get("types")
                .or_else(|| v.get("typings"))
                .and_then(|t| t.as_str())
            {
                let candidate = pkg_dir.join(types);
                if candidate.exists() {
                    return Some(candidate);
                }
            }

            // Check "exports" field: exports["."].types
            if let Some(exports) = v.get("exports")
                && let Some(types) = exports
                    .get(".")
                    .and_then(|dot| dot.get("types"))
                    .and_then(|t| t.as_str())
            {
                let candidate = pkg_dir.join(types);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }

        // Fallbacks
        let candidates = [
            pkg_dir.join("index.d.ts"),
            pkg_dir.join("dist").join("index.d.ts"),
            pkg_dir.join("types").join("index.d.ts"),
            pkg_dir.join(format!(
                "{}.d.ts",
                pkg_dir.file_name().unwrap_or_default().to_string_lossy()
            )),
        ];

        for cand in &candidates {
            if cand.exists() {
                return Some(cand.clone());
            }
        }

        None
    }

    /// Crawl a .d.ts file and follow re-exports up to depth 4
    fn crawl_dts_file(
        path: &Path,
        package_name: &str,
        visited: &mut std::collections::HashSet<PathBuf>,
        symbols: &mut Vec<ExternalSymbolDecl>,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 4 || !visited.insert(path.to_path_buf()) {
            return Ok(());
        }

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Ok(()),
        };

        let parent_dir = path.parent().unwrap_or(Path::new(""));
        let (ast_symbols, _) = AstScanner::extract_symbols_and_imports(&content, Some("ts"));

        for sym in ast_symbols {
            symbols.push(ExternalSymbolDecl {
                package: package_name.to_string(),
                symbol_name: sym.name,
                kind: sym.kind.to_string(),
                signature: sym.signature,
                doc_comment: sym.doc_comment,
                source_dts: Some(
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                ),
            });
        }

        // Also run TS_EXPORT_RE regex to capture type aliases or interface declarations that AST might categorize broadly
        for cap in TS_EXPORT_RE.captures_iter(&content) {
            let kind = cap.get(1).map(|m| m.as_str()).unwrap_or("type");
            let name = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            if !name.is_empty() && !symbols.iter().any(|s| s.symbol_name == name) {
                // Find line for signature
                let line = content.lines().find(|l| l.contains(name)).unwrap_or("");
                symbols.push(ExternalSymbolDecl {
                    package: package_name.to_string(),
                    symbol_name: name.to_string(),
                    kind: kind.to_string(),
                    signature: line.trim().to_string(),
                    doc_comment: None,
                    source_dts: Some(
                        path.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                    ),
                });
            }
        }

        // Follow export * from '...'
        for cap in TS_REEXPORT_ALL_RE.captures_iter(&content) {
            if let Some(spec) = cap.get(1).map(|m| m.as_str())
                && spec.starts_with('.')
                && let Some(target) = Self::resolve_relative_dts(parent_dir, spec)
            {
                let _ = Self::crawl_dts_file(&target, package_name, visited, symbols, depth + 1);
            }
        }

        // Follow export { ... } from '...'
        for cap in TS_REEXPORT_NAMED_RE.captures_iter(&content) {
            if let Some(spec) = cap.get(2).map(|m| m.as_str())
                && spec.starts_with('.')
                && let Some(target) = Self::resolve_relative_dts(parent_dir, spec)
            {
                let _ = Self::crawl_dts_file(&target, package_name, visited, symbols, depth + 1);
            }
        }

        Ok(())
    }

    /// Resolve relative path in .d.ts files
    fn resolve_relative_dts(base_dir: &Path, spec: &str) -> Option<PathBuf> {
        let joined = base_dir.join(spec);
        let cands = [
            joined.clone(),
            joined.with_extension("d.ts"),
            joined.with_extension("ts"),
            joined.join("index.d.ts"),
            joined.join("index.ts"),
        ];

        for c in &cands {
            if c.exists() && c.is_file() {
                return Some(c.clone());
            }
        }

        None
    }

    /// Fallback resolver for Rust crates in Cargo workspace or global ~/.cargo/registry/src
    fn resolve_rust_crate(
        workspace_root: &Path,
        crate_name: &str,
    ) -> Result<Vec<ExternalSymbolDecl>, String> {
        // 1. Look in workspace members or local crates
        let crates_dir = workspace_root.join("crates").join(crate_name);
        let src_lib = if crates_dir.exists() {
            crates_dir.join("src").join("lib.rs")
        } else {
            workspace_root.join("src").join("lib.rs")
        };

        if src_lib.exists()
            && let Ok(content) = fs::read_to_string(&src_lib)
        {
            let (syms, _) = AstScanner::extract_symbols_and_imports(&content, Some("rs"));
            let decls: Vec<ExternalSymbolDecl> = syms
                .into_iter()
                .filter(|s| s.signature.starts_with("pub "))
                .map(|s| ExternalSymbolDecl {
                    package: crate_name.to_string(),
                    symbol_name: s.name,
                    kind: s.kind.to_string(),
                    signature: s.signature,
                    doc_comment: s.doc_comment,
                    source_dts: Some(src_lib.to_string_lossy().to_string()),
                })
                .collect();
            if !decls.is_empty() {
                return Ok(decls);
            }
        }

        // 2. Global Cargo cache lookup: ~/.cargo/registry/src/index.crates.io-*
        let cargo_home = std::env::var("CARGO_HOME")
            .map(PathBuf::from)
            .ok()
            .or_else(|| {
                std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .ok()
                    .map(|h| PathBuf::from(h).join(".cargo"))
            });

        if let Some(cargo_dir) = cargo_home {
            let registry_src = cargo_dir.join("registry").join("src");
            if registry_src.is_dir()
                && let Ok(registries) = fs::read_dir(&registry_src)
            {
                for reg in registries.flatten() {
                    let reg_path = reg.path();
                    if reg_path.is_dir()
                        && let Ok(crate_entries) = fs::read_dir(&reg_path)
                    {
                        let mut matching_dirs: Vec<PathBuf> = Vec::new();
                        for entry in crate_entries.flatten() {
                            let path = entry.path();
                            if path.is_dir()
                                && let Some(folder_name) = path.file_name().and_then(|n| n.to_str())
                                && let Some(rest) = folder_name.strip_prefix(crate_name)
                                && rest.starts_with('-')
                            {
                                matching_dirs.push(path);
                            }
                        }
                        matching_dirs.sort();
                        if let Some(crate_dir) = matching_dirs.last() {
                            let lib_rs = crate_dir.join("src").join("lib.rs");
                            if lib_rs.exists()
                                && let Ok(content) = fs::read_to_string(&lib_rs)
                            {
                                let (syms, _) =
                                    AstScanner::extract_symbols_and_imports(&content, Some("rs"));
                                let decls: Vec<ExternalSymbolDecl> = syms
                                    .into_iter()
                                    .filter(|s| s.signature.starts_with("pub "))
                                    .map(|s| ExternalSymbolDecl {
                                        package: crate_name.to_string(),
                                        symbol_name: s.name,
                                        kind: s.kind.to_string(),
                                        signature: s.signature,
                                        doc_comment: s.doc_comment,
                                        source_dts: Some(lib_rs.to_string_lossy().to_string()),
                                    })
                                    .collect();
                                if !decls.is_empty() {
                                    return Ok(decls);
                                }
                            }
                        }
                    }
                }
            }
        }

        Err(format!(
            "Crate '{}' source cannot be resolved locally or in global cargo registry.",
            crate_name
        ))
    }

    /// Resolve C/C++ header files from workspace include/ or root
    fn resolve_cpp_header(
        workspace_root: &Path,
        pkg_name: &str,
    ) -> Result<Vec<ExternalSymbolDecl>, String> {
        let candidates = [
            workspace_root
                .join("include")
                .join(format!("{}.hpp", pkg_name)),
            workspace_root
                .join("include")
                .join(format!("{}.h", pkg_name)),
            workspace_root
                .join("include")
                .join(pkg_name)
                .join(format!("{}.hpp", pkg_name)),
            workspace_root
                .join("include")
                .join(pkg_name)
                .join(format!("{}.h", pkg_name)),
            workspace_root.join(format!("{}.hpp", pkg_name)),
            workspace_root.join(format!("{}.h", pkg_name)),
        ];

        for cand in &candidates {
            if cand.exists()
                && cand.is_file()
                && let Ok(content) = fs::read_to_string(cand)
            {
                let (syms, _) = AstScanner::extract_symbols_and_imports(&content, Some("hpp"));
                let decls: Vec<ExternalSymbolDecl> = syms
                    .into_iter()
                    .map(|s| ExternalSymbolDecl {
                        package: pkg_name.to_string(),
                        symbol_name: s.name,
                        kind: s.kind.to_string(),
                        signature: s.signature,
                        doc_comment: s.doc_comment,
                        source_dts: Some(cand.to_string_lossy().to_string()),
                    })
                    .collect();
                if !decls.is_empty() {
                    return Ok(decls);
                }
            }
        }

        Err(format!(
            "Header for '{}' not found in workspace include directory.",
            pkg_name
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_inspect_package_ts_declarations() {
        let dir = tempdir().unwrap();
        let node_modules = dir.path().join("node_modules");
        let pkg_dir = node_modules.join("my-sim-lib");
        fs::create_dir_all(&pkg_dir).unwrap();

        let pkg_json = r#"{
            "name": "my-sim-lib",
            "version": "1.0.0",
            "types": "./dist/index.d.ts"
        }"#;
        fs::write(pkg_dir.join("package.json"), pkg_json).unwrap();

        let dist_dir = pkg_dir.join("dist");
        fs::create_dir_all(&dist_dir).unwrap();

        let dts_content = r#"
export interface Vector3D {
    x: number;
    y: number;
    z: number;
}

export type ParticleState = 'active' | 'dead';

/**
 * Updates physics simulation for a particle set
 */
export declare function stepSimulation(particles: Vector3D[], dt: number): void;

export * from './types';
"#;
        fs::write(dist_dir.join("index.d.ts"), dts_content).unwrap();

        let types_dts = r#"
export interface SimulationOptions {
    gravity: number;
    friction: number;
}
"#;
        fs::write(dist_dir.join("types.d.ts"), types_dts).unwrap();

        let mut indexer = PackageIndexer::new();

        // 1. Query all symbols
        let res_all = indexer
            .inspect_package(dir.path(), "my-sim-lib", None, None)
            .expect("indexing must succeed");
        assert!(res_all.total_symbols >= 4);
        assert!(res_all.symbols.iter().any(|s| s.symbol_name == "Vector3D"));
        assert!(
            res_all
                .symbols
                .iter()
                .any(|s| s.symbol_name == "stepSimulation")
        );
        assert!(
            res_all
                .symbols
                .iter()
                .any(|s| s.symbol_name == "SimulationOptions")
        );

        // 2. Query exact symbol
        let res_sym = indexer
            .inspect_package(dir.path(), "my-sim-lib", Some("Vector3D"), None)
            .expect("lookup symbol must succeed");
        assert_eq!(res_sym.symbols.len(), 1);
        assert_eq!(res_sym.symbols[0].symbol_name, "Vector3D");
        assert!(res_sym.requested_symbol.is_some());

        // 3. Query non-existent symbol with helpful hint
        let res_err = indexer.inspect_package(dir.path(), "my-sim-lib", Some("Vector4D"), None);
        assert!(res_err.is_err());
        assert!(res_err.unwrap_err().contains("Vector3D"));
    }

    #[test]
    fn test_inspect_package_types_scope_fallback() {
        let dir = tempdir().unwrap();
        let types_pkg = dir.path().join("node_modules").join("@types").join("three");
        fs::create_dir_all(&types_pkg).unwrap();

        let dts = r#"
export class Scene {
    name: string;
    add(object: any): this;
}
"#;
        fs::write(types_pkg.join("index.d.ts"), dts).unwrap();

        let mut indexer = PackageIndexer::new();
        let res = indexer
            .inspect_package(dir.path(), "three", Some("Scene"), None)
            .expect("should find in @types/three");

        assert_eq!(res.symbols[0].symbol_name, "Scene");
        assert_eq!(res.symbols[0].kind, "class");
    }
}
