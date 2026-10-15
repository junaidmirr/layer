use crate::graph::VectorGraph;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::process::Command;

pub const TEMPLATE_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GeneratorPreset {
    NextJs,
    ReactTs,
    RustBin,
    RustLib,
    Python,
    Go,
    Cpp,
    Swift,
    Kotlin,
    CSharp,
    Java,
    Custom(String),
}

impl GeneratorPreset {
    pub fn parse(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        match trimmed.to_lowercase().as_str() {
            "next" | "nextjs" | "next-js" | "next-ts" | "nextjs-ts" => Some(Self::NextJs),
            "react-ts" | "vite-react-ts" | "react" | "vite" | "vite-ts" => Some(Self::ReactTs),
            "rust-bin" | "cargo-bin" | "rust" | "cargo" => Some(Self::RustBin),
            "rust-lib" | "cargo-lib" => Some(Self::RustLib),
            "python" | "py" | "python-app" | "fastapi" => Some(Self::Python),
            "go" | "golang" => Some(Self::Go),
            "cpp" | "c++" | "c" => Some(Self::Cpp),
            "swift" => Some(Self::Swift),
            "kotlin" | "kt" => Some(Self::Kotlin),
            "csharp" | "cs" | "dotnet" => Some(Self::CSharp),
            "java" | "spring" | "maven" | "gradle" => Some(Self::Java),
            "custom" => Some(Self::Custom("custom".to_string())),
            _ => None,
        }
    }

    pub fn parse_or_custom(s: &str) -> Self {
        Self::parse(s).unwrap_or_else(|| Self::Custom(s.trim().to_string()))
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::NextJs => "nextjs",
            Self::ReactTs => "react-ts",
            Self::RustBin => "rust-bin",
            Self::RustLib => "rust-lib",
            Self::Python => "python",
            Self::Go => "go",
            Self::Cpp => "cpp",
            Self::Swift => "swift",
            Self::Kotlin => "kotlin",
            Self::CSharp => "csharp",
            Self::Java => "java",
            Self::Custom(s) => s.as_str(),
        }
    }
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleTypeSpec {
    pub name: String,
    pub kind: String, // "interface" | "type" | "struct" | "enum"
    pub definition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleFunctionSpec {
    pub name: String,
    pub signature: String,
    #[serde(default)]
    pub stub_body: Option<String>,
    #[serde(default)]
    pub doc_comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleFileSpec {
    pub path: String,
    #[serde(default)]
    pub imports: Vec<String>,
    #[serde(default)]
    pub types: Vec<ModuleTypeSpec>,
    #[serde(default)]
    pub functions: Vec<ModuleFunctionSpec>,
    #[serde(default)]
    pub raw_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaffoldModuleRequest {
    pub module_name: String,
    pub files: Vec<ModuleFileSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaffoldedSymbolHandle {
    pub file: String,
    pub symbol_name: String,
    pub kind: String,
    pub lines: [usize; 2],
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaffoldModuleResult {
    pub success: bool,
    pub module_name: String,
    pub created_files: Vec<String>,
    pub symbols_created: Vec<ScaffoldedSymbolHandle>,
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
        environment_note: None,
    })
}

fn try_npm_install(target_dir: &Path, packages: Option<&[String]>) -> Option<String> {
    let offline_note = "Offline/sandboxed environment detected: skipped network package install. Do NOT run 'npm install' in bash (it will hang); proceed directly with Layer tools.".to_string();

    let mut npm_args = vec![
        "install".to_string(),
        "--prefer-offline".to_string(),
        "--no-audit".to_string(),
        "--no-fund".to_string(),
    ];
    if let Some(pkgs) = packages {
        for pkg in pkgs {
            let p = pkg.trim();
            if !p.is_empty() {
                npm_args.push(p.to_string());
            }
        }
    }

    let npm_start = std::time::Instant::now();
    let npm_timeout = std::time::Duration::from_secs(3);

    match Command::new("npm")
        .args(&npm_args)
        .current_dir(target_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(mut npm_child) => {
            while npm_start.elapsed() < npm_timeout {
                match npm_child.try_wait() {
                    Ok(Some(status)) => {
                        if status.success() {
                            return None;
                        } else {
                            return Some(offline_note);
                        }
                    }
                    Ok(None) => {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(_) => {
                        let _ = npm_child.kill();
                        let _ = npm_child.wait();
                        return Some(offline_note);
                    }
                }
            }
            let _ = npm_child.kill();
            let _ = npm_child.wait();
            Some(offline_note)
        }
        Err(_) => Some(offline_note),
    }
}

fn try_cargo_add(target_dir: &Path, packages: &[String]) -> Option<String> {
    let offline_note = "Offline/sandboxed environment detected: skipped network package install. Do NOT run 'cargo add' in bash (it will hang); proceed directly with Layer tools.".to_string();

    let mut cargo_args = vec!["add".to_string()];
    for pkg in packages {
        let p = pkg.trim();
        if !p.is_empty() {
            cargo_args.push(p.to_string());
        }
    }
    let cargo_start = std::time::Instant::now();
    let cargo_timeout = std::time::Duration::from_secs(3);

    if let Ok(mut cargo_child) = Command::new("cargo")
        .args(&cargo_args)
        .current_dir(target_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        while cargo_start.elapsed() < cargo_timeout {
            match cargo_child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        return None;
                    } else {
                        return Some(offline_note);
                    }
                }
                Ok(None) => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(_) => {
                    let _ = cargo_child.kill();
                    let _ = cargo_child.wait();
                    return Some(offline_note);
                }
            }
        }
        let _ = cargo_child.kill();
        let _ = cargo_child.wait();
        Some(offline_note)
    } else {
        Some(offline_note)
    }
}

/// Bootstrap a project in `dest_parent_dir` using standard non-interactive ecosystem generators.
/// Executes with timeout, validates safety, optionally installs packages with fast flags, and indexes the resulting project into `graph`.
pub fn bootstrap_project(
    preset: GeneratorPreset,
    project_name: &str,
    dest_parent_dir: &Path,
    packages: Option<Vec<String>>,
    graph: &mut VectorGraph,
) -> Result<ScaffoldResult, String> {
    bootstrap_project_with_options(
        preset,
        project_name,
        dest_parent_dir,
        packages,
        false,
        graph,
    )
}

pub fn bootstrap_project_with_options(
    preset: GeneratorPreset,
    project_name: &str,
    dest_parent_dir: &Path,
    packages: Option<Vec<String>>,
    install_dependencies: bool,
    graph: &mut VectorGraph,
) -> Result<ScaffoldResult, String> {
    validate_project_name(project_name)?;
    let clean_name = project_name.trim();
    let target_dir = dest_parent_dir.join(clean_name);
    validate_destination(&target_dir)?;

    let mut warning_note: Option<String> = None;

    match preset {
        GeneratorPreset::NextJs => {
            fs::create_dir_all(&target_dir)
                .map_err(|e| format!("Failed to create project dir: {}", e))?;
            let app_dir = target_dir.join("app");
            fs::create_dir_all(&app_dir).map_err(|e| format!("Failed to create app dir: {}", e))?;

            let mut deps = serde_json::json!({
                "next": "^15.1.0",
                "react": "^19.0.0",
                "react-dom": "^19.0.0"
            });
            if let Some(ref pkgs) = packages
                && let Some(obj) = deps.as_object_mut()
            {
                for p in pkgs {
                    let trimmed = p.trim();
                    if !trimmed.is_empty() && !obj.contains_key(trimmed) {
                        obj.insert(
                            trimmed.to_string(),
                            serde_json::Value::String("*".to_string()),
                        );
                    }
                }
            }

            let pkg_json = serde_json::json!({
                "name": clean_name,
                "version": "0.1.0",
                "private": true,
                "scripts": {
                    "dev": "next dev",
                    "build": "next build",
                    "start": "next start",
                    "lint": "next lint"
                },
                "dependencies": deps,
                "devDependencies": {
                    "@types/node": "^22.0.0",
                    "@types/react": "^19.0.0",
                    "@types/react-dom": "^19.0.0",
                    "postcss": "^8.4.0",
                    "tailwindcss": "^4.0.0",
                    "typescript": "^5.7.0"
                }
            });
            fs::write(
                target_dir.join("package.json"),
                serde_json::to_string_pretty(&pkg_json).unwrap_or_default(),
            )
            .map_err(|e| format!("Failed to write package.json: {}", e))?;

            let tsconfig = r#"{
  "compilerOptions": {
    "target": "ES2017",
    "lib": ["dom", "dom.iterable", "esnext"],
    "allowJs": true,
    "skipLibCheck": true,
    "strict": true,
    "noEmit": true,
    "esModuleInterop": true,
    "module": "esnext",
    "moduleResolution": "bundler",
    "resolveJsonModule": true,
    "isolatedModules": true,
    "jsx": "preserve",
    "incremental": true,
    "plugins": [
      {
        "name": "next"
      }
    ],
    "paths": {
      "@/*": ["./*"]
    }
  },
  "include": ["next-env.d.ts", "**/*.ts", "**/*.tsx", ".next/types/**/*.ts"],
  "exclude": ["node_modules"]
}
"#;
            fs::write(target_dir.join("tsconfig.json"), tsconfig)
                .map_err(|e| format!("Failed to write tsconfig.json: {}", e))?;

            fs::write(
                target_dir.join("next-env.d.ts"),
                "/// <reference types=\"next\" />\n/// <reference types=\"next/image-types/global\" />\n",
            )
            .map_err(|e| format!("Failed to write next-env.d.ts: {}", e))?;

            fs::write(
                target_dir.join("next.config.ts"),
                "import type { NextConfig } from \"next\";\n\nconst nextConfig: NextConfig = {};\n\nexport default nextConfig;\n",
            )
            .map_err(|e| format!("Failed to write next.config.ts: {}", e))?;

            fs::write(
                target_dir.join("postcss.config.mjs"),
                "export default {\n  plugins: {\n    \"@tailwindcss/postcss\": {},\n  },\n};\n",
            )
            .map_err(|e| format!("Failed to write postcss.config.mjs: {}", e))?;

            fs::write(
                app_dir.join("globals.css"),
                "@import \"tailwindcss\";\n\n:root {\n  --background: #ffffff;\n  --foreground: #171717;\n}\n\nbody {\n  color: var(--foreground);\n  background: var(--background);\n  font-family: Arial, Helvetica, sans-serif;\n}\n",
            )
            .map_err(|e| format!("Failed to write app/globals.css: {}", e))?;

            fs::write(
                app_dir.join("layout.tsx"),
                format!("import type {{ Metadata }} from \"next\";\nimport \"./globals.css\";\n\nexport const metadata: Metadata = {{\n  title: \"{}\",\n  description: \"Created by Layer\",\n}};\n\nexport default function RootLayout({{\n  children,\n}}: Readonly<{{\n  children: React.ReactNode;\n}}>) {{\n  return (\n    <html lang=\"en\">\n      <body>{{children}}</body>\n    </html>\n  );\n}}\n", clean_name),
            )
            .map_err(|e| format!("Failed to write app/layout.tsx: {}", e))?;

            fs::write(
                app_dir.join("page.tsx"),
                format!("export default function Home() {{\n  return (\n    <main className=\"flex min-h-screen flex-col items-center justify-center p-24\">\n      <h1 className=\"text-4xl font-bold tracking-tight\">\n        Welcome to <span className=\"text-blue-600\">{}</span>\n      </h1>\n      <p className=\"mt-4 text-muted-foreground text-lg\">\n        Bootstrapped with Layer instant Next.js generator\n      </p>\n    </main>\n  );\n}}\n", clean_name),
            )
            .map_err(|e| format!("Failed to write app/page.tsx: {}", e))?;

            if install_dependencies {
                warning_note = try_npm_install(&target_dir, packages.as_deref());
            }
        }
        GeneratorPreset::ReactTs => {
            fs::create_dir_all(&target_dir)
                .map_err(|e| format!("Failed to create project dir: {}", e))?;
            let src_dir = target_dir.join("src");
            fs::create_dir_all(&src_dir).map_err(|e| format!("Failed to create src dir: {}", e))?;

            let mut deps = serde_json::json!({
                "react": "^18.3.1",
                "react-dom": "^18.3.1"
            });
            if let Some(ref pkgs) = packages
                && let Some(obj) = deps.as_object_mut()
            {
                for p in pkgs {
                    let trimmed = p.trim();
                    if !trimmed.is_empty() && !obj.contains_key(trimmed) {
                        obj.insert(
                            trimmed.to_string(),
                            serde_json::Value::String("*".to_string()),
                        );
                    }
                }
            }

            let pkg_json = serde_json::json!({
                "name": clean_name,
                "private": true,
                "version": "0.0.0",
                "type": "module",
                "scripts": {
                    "dev": "vite",
                    "build": "tsc -b && vite build",
                    "preview": "vite preview"
                },
                "dependencies": deps,
                "devDependencies": {
                    "@types/react": "^18.3.12",
                    "@types/react-dom": "^18.3.1",
                    "@vitejs/plugin-react": "^4.3.4",
                    "typescript": "~5.6.2",
                    "vite": "^6.0.0"
                }
            });
            fs::write(
                target_dir.join("package.json"),
                serde_json::to_string_pretty(&pkg_json).unwrap_or_default(),
            )
            .map_err(|e| format!("Failed to write package.json: {}", e))?;

            let tsconfig = r#"{
  "compilerOptions": {
    "target": "ES2020",
    "useDefineForClassFields": true,
    "lib": ["ES2020", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "skipLibCheck": true,
    "moduleResolution": "bundler",
    "isolatedModules": true,
    "noEmit": true,
    "jsx": "react-jsx",
    "strict": true
  },
  "include": ["src"]
}
"#;
            fs::write(target_dir.join("tsconfig.json"), tsconfig)
                .map_err(|e| format!("Failed to write tsconfig.json: {}", e))?;

            fs::write(
                target_dir.join("vite.config.ts"),
                "import { defineConfig } from 'vite';\nimport react from '@vitejs/plugin-react';\n\nexport default defineConfig({\n  plugins: [react()],\n});\n",
            )
            .map_err(|e| format!("Failed to write vite.config.ts: {}", e))?;

            fs::write(
                target_dir.join("index.html"),
                format!("<!doctype html>\n<html lang=\"en\">\n  <head>\n    <meta charset=\"UTF-8\" />\n    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\" />\n    <title>{}</title>\n  </head>\n  <body>\n    <div id=\"root\"></div>\n    <script type=\"module\" src=\"/src/main.tsx\"></script>\n  </body>\n</html>\n", clean_name),
            )
            .map_err(|e| format!("Failed to write index.html: {}", e))?;

            fs::write(
                src_dir.join("index.css"),
                ":root {\n  font-family: Inter, system-ui, Avenir, Helvetica, Arial, sans-serif;\n  line-height: 1.5;\n  font-weight: 400;\n}\nbody {\n  margin: 0;\n  display: flex;\n  min-width: 320px;\n  min-height: 100vh;\n}\n",
            )
            .map_err(|e| format!("Failed to write src/index.css: {}", e))?;

            fs::write(
                src_dir.join("App.css"),
                ".container {\n  max-width: 1280px;\n  margin: 0 auto;\n  padding: 2rem;\n  text-align: center;\n}\n",
            )
            .map_err(|e| format!("Failed to write src/App.css: {}", e))?;

            fs::write(
                src_dir.join("App.tsx"),
                format!("import {{ useState }} from 'react';\nimport './App.css';\n\nexport function App() {{\n  const [count, setCount] = useState(0);\n\n  return (\n    <div className=\"container\">\n      <h1>{}</h1>\n      <button onClick={{{{() => setCount((c) => c + 1)}}}}>\n        count is {{count}}\n      </button>\n    </div>\n  );\n}}\n\nexport default App;\n", clean_name),
            )
            .map_err(|e| format!("Failed to write src/App.tsx: {}", e))?;

            fs::write(
                src_dir.join("main.tsx"),
                "import { StrictMode } from 'react';\nimport { createRoot } from 'react-dom/client';\nimport './index.css';\nimport App from './App.tsx';\n\ncreateRoot(document.getElementById('root')!).render(\n  <StrictMode>\n    <App />\n  </StrictMode>,\n);\n",
            )
            .map_err(|e| format!("Failed to write src/main.tsx: {}", e))?;

            if install_dependencies {
                warning_note = try_npm_install(&target_dir, packages.as_deref());
            }
        }
        GeneratorPreset::Custom(ref custom_name) => {
            fs::create_dir_all(&target_dir)
                .map_err(|e| format!("Failed to create project dir: {}", e))?;

            let mut deps = serde_json::Map::new();
            if let Some(ref pkgs) = packages {
                for p in pkgs {
                    let trimmed = p.trim();
                    if !trimmed.is_empty() {
                        deps.insert(
                            trimmed.to_string(),
                            serde_json::Value::String("*".to_string()),
                        );
                    }
                }
            }

            let pkg_json = serde_json::json!({
                "name": clean_name,
                "version": "0.1.0",
                "private": true,
                "description": format!("{} project bootstrapped by Layer", custom_name),
                "dependencies": deps,
            });
            fs::write(
                target_dir.join("package.json"),
                serde_json::to_string_pretty(&pkg_json).unwrap_or_default(),
            )
            .map_err(|e| format!("Failed to write package.json: {}", e))?;

            let readme = format!(
                "# {}\n\nBootstrapped with Layer (preset: `{}`).\n",
                clean_name, custom_name
            );
            fs::write(target_dir.join("README.md"), readme)
                .map_err(|e| format!("Failed to write README.md: {}", e))?;

            if install_dependencies {
                warning_note = try_npm_install(&target_dir, packages.as_deref());
            }
        }
        GeneratorPreset::RustBin | GeneratorPreset::RustLib => {
            fs::create_dir_all(&target_dir)
                .map_err(|e| format!("Failed to create project dir: {}", e))?;
            let src_dir = target_dir.join("src");
            fs::create_dir_all(&src_dir).map_err(|e| format!("Failed to create src dir: {}", e))?;

            let mut deps_str = String::new();
            if let Some(ref pkgs) = packages {
                for p in pkgs {
                    let trimmed = p.trim();
                    if !trimmed.is_empty() {
                        deps_str.push_str(&format!("{} = \"*\"\n", trimmed));
                    }
                }
            }

            let cargo_toml = format!(
                "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{}",
                clean_name, deps_str
            );
            fs::write(target_dir.join("Cargo.toml"), cargo_toml)
                .map_err(|e| format!("Failed to write Cargo.toml: {}", e))?;

            if preset == GeneratorPreset::RustBin {
                fs::write(
                    src_dir.join("main.rs"),
                    "fn main() {\n    println!(\"Hello, world!\");\n}\n",
                )
                .map_err(|e| format!("Failed to write src/main.rs: {}", e))?;
            } else {
                fs::write(
                    src_dir.join("lib.rs"),
                    "pub fn add(left: u64, right: u64) -> u64 {\n    left + right\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn it_works() {\n        let result = add(2, 2);\n        assert_eq!(result, 4);\n    }\n}\n",
                )
                .map_err(|e| format!("Failed to write src/lib.rs: {}", e))?;
            }

            if install_dependencies
                && let Some(ref pkgs) = packages
                && !pkgs.is_empty()
            {
                warning_note = try_cargo_add(&target_dir, pkgs);
            }
        }
        GeneratorPreset::Python => {
            fs::create_dir_all(&target_dir)
                .map_err(|e| format!("Failed to create project dir: {}", e))?;
            let src_dir = target_dir.join("src").join(clean_name);
            fs::create_dir_all(&src_dir).map_err(|e| format!("Failed to create src dir: {}", e))?;
            fs::create_dir_all(target_dir.join("tests"))
                .map_err(|e| format!("Failed to create tests dir: {}", e))?;

            let pyproject = format!(
                "[build-system]\nrequires = [\"setuptools>=61.0\"]\nbuild-backend = \"setuptools.build_meta\"\n\n[project]\nname = \"{}\"\nversion = \"0.1.0\"\ndescription = \"Python project bootstrapped by Layer\"\ndependencies = []\n",
                clean_name
            );
            fs::write(target_dir.join("pyproject.toml"), pyproject)
                .map_err(|e| format!("Failed to write pyproject.toml: {}", e))?;

            let reqs = packages.as_deref().unwrap_or(&[]).join("\n");
            fs::write(target_dir.join("requirements.txt"), reqs)
                .map_err(|e| format!("Failed to write requirements.txt: {}", e))?;

            fs::write(src_dir.join("__init__.py"), "")
                .map_err(|e| format!("Failed to write __init__.py: {}", e))?;
            fs::write(
                src_dir.join("main.py"),
                format!("def main() -> None:\n    print(\"Hello from {}!\")\n\nif __name__ == '__main__':\n    main()\n", clean_name),
            )
            .map_err(|e| format!("Failed to write main.py: {}", e))?;
            fs::write(
                target_dir.join("tests").join("test_main.py"),
                "def test_sanity():\n    assert True\n",
            )
            .map_err(|e| format!("Failed to write test_main.py: {}", e))?;
        }
        GeneratorPreset::Go => {
            fs::create_dir_all(&target_dir)
                .map_err(|e| format!("Failed to create project dir: {}", e))?;
            let pkg_dir = target_dir.join("pkg");
            fs::create_dir_all(&pkg_dir).map_err(|e| format!("Failed to create pkg dir: {}", e))?;

            let go_mod = format!("module {}\n\ngo 1.21\n", clean_name);
            fs::write(target_dir.join("go.mod"), go_mod)
                .map_err(|e| format!("Failed to write go.mod: {}", e))?;

            let main_go = format!(
                "package main\n\nimport \"fmt\"\n\nfunc main() {{\n    fmt.Println(\"Hello from {}!\")\n}}\n",
                clean_name
            );
            fs::write(target_dir.join("main.go"), main_go)
                .map_err(|e| format!("Failed to write main.go: {}", e))?;
            fs::write(
                pkg_dir.join("util.go"),
                "package pkg\n\nfunc Version() string {\n    return \"0.1.0\"\n}\n",
            )
            .map_err(|e| format!("Failed to write pkg/util.go: {}", e))?;
        }
        GeneratorPreset::Cpp => {
            fs::create_dir_all(&target_dir)
                .map_err(|e| format!("Failed to create project dir: {}", e))?;
            let include_dir = target_dir.join("include").join(clean_name);
            let src_dir = target_dir.join("src");
            fs::create_dir_all(&include_dir)
                .map_err(|e| format!("Failed to create include dir: {}", e))?;
            fs::create_dir_all(&src_dir).map_err(|e| format!("Failed to create src dir: {}", e))?;

            let cmake = format!(
                "cmake_minimum_required(VERSION 3.15)\nproject({} CXX)\nset(CMAKE_CXX_STANDARD 20)\n\ninclude_directories(include)\nadd_executable({} src/main.cpp src/core.cpp)\n",
                clean_name, clean_name
            );
            fs::write(target_dir.join("CMakeLists.txt"), cmake)
                .map_err(|e| format!("Failed to write CMakeLists.txt: {}", e))?;

            fs::write(
                include_dir.join("core.hpp"),
                "#pragma once\n\n#include <string>\n\nstd::string get_version();\n",
            )
            .map_err(|e| format!("Failed to write core.hpp: {}", e))?;

            fs::write(
                src_dir.join("core.cpp"),
                format!("#include \"{}/core.hpp\"\n\nstd::string get_version() {{\n    return \"0.1.0\";\n}}\n", clean_name),
            )
            .map_err(|e| format!("Failed to write core.cpp: {}", e))?;

            fs::write(
                src_dir.join("main.cpp"),
                format!("#include <iostream>\n#include \"{}/core.hpp\"\n\nint main() {{\n    std::cout << \"Hello from \" << get_version() << std::endl;\n    return 0;\n}}\n", clean_name),
            )
            .map_err(|e| format!("Failed to write main.cpp: {}", e))?;
        }
        GeneratorPreset::Swift => {
            fs::create_dir_all(&target_dir)
                .map_err(|e| format!("Failed to create project dir: {}", e))?;
            let src_dir = target_dir.join("Sources").join(clean_name);
            fs::create_dir_all(&src_dir)
                .map_err(|e| format!("Failed to create Sources dir: {}", e))?;

            let pkg_swift = format!(
                "// swift-tools-version: 5.9\nimport PackageDescription\n\nlet package = Package(\n    name: \"{}\",\n    targets: [\n        .executableTarget(name: \"{}\")\n    ]\n)\n",
                clean_name, clean_name
            );
            fs::write(target_dir.join("Package.swift"), pkg_swift)
                .map_err(|e| format!("Failed to write Package.swift: {}", e))?;
            fs::write(
                src_dir.join("main.swift"),
                format!("print(\"Hello from {}!\")\n", clean_name),
            )
            .map_err(|e| format!("Failed to write main.swift: {}", e))?;
        }
        GeneratorPreset::Kotlin => {
            fs::create_dir_all(&target_dir)
                .map_err(|e| format!("Failed to create project dir: {}", e))?;
            let src_dir = target_dir.join("src").join("main").join("kotlin");
            fs::create_dir_all(&src_dir)
                .map_err(|e| format!("Failed to create kotlin dir: {}", e))?;

            let gradle_kts = "plugins {\n    kotlin(\"jvm\") version \"1.9.22\"\n    application\n}\n\nrepositories {\n    mavenCentral()\n}\n\napplication {\n    mainClass.set(\"MainKt\")\n}\n";
            fs::write(target_dir.join("build.gradle.kts"), gradle_kts)
                .map_err(|e| format!("Failed to write build.gradle.kts: {}", e))?;
            fs::write(
                target_dir.join("settings.gradle.kts"),
                format!("rootProject.name = \"{}\"\n", clean_name),
            )
            .map_err(|e| format!("Failed to write settings.gradle.kts: {}", e))?;
            fs::write(
                src_dir.join("Main.kt"),
                format!(
                    "fun main() {{\n    println(\"Hello from {}!\")\n}}\n",
                    clean_name
                ),
            )
            .map_err(|e| format!("Failed to write Main.kt: {}", e))?;
        }
        GeneratorPreset::CSharp => {
            fs::create_dir_all(&target_dir)
                .map_err(|e| format!("Failed to create project dir: {}", e))?;

            let csproj = "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <OutputType>Exe</OutputType>\n    <TargetFramework>net8.0</TargetFramework>\n    <ImplicitUsings>enable</ImplicitUsings>\n    <Nullable>enable</Nullable>\n  </PropertyGroup>\n</Project>\n";
            fs::write(target_dir.join(format!("{}.csproj", clean_name)), csproj)
                .map_err(|e| format!("Failed to write .csproj: {}", e))?;

            let program_cs = format!(
                "namespace {};\n\npublic class Program\n{{\n    public static void Main(string[] args)\n    {{\n        Console.WriteLine(\"Hello from {}!\");\n    }}\n}}\n",
                clean_name, clean_name
            );
            fs::write(target_dir.join("Program.cs"), program_cs)
                .map_err(|e| format!("Failed to write Program.cs: {}", e))?;
        }
        GeneratorPreset::Java => {
            fs::create_dir_all(&target_dir)
                .map_err(|e| format!("Failed to create project dir: {}", e))?;
            let src_dir = target_dir
                .join("src")
                .join("main")
                .join("java")
                .join("com")
                .join("example");
            fs::create_dir_all(&src_dir)
                .map_err(|e| format!("Failed to create java src dir: {}", e))?;

            let pom_xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<project xmlns=\"http://maven.apache.org/POM/4.0.0\"\n         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"\n         xsi:schemaLocation=\"http://maven.apache.org/POM/4.0.0 http://maven.apache.org/xsd/maven-4.0.0.xsd\">\n    <modelVersion>4.0.0</modelVersion>\n    <groupId>com.example</groupId>\n    <artifactId>{}</artifactId>\n    <version>1.0.0-SNAPSHOT</version>\n    <properties>\n        <maven.compiler.source>17</maven.compiler.source>\n        <maven.compiler.target>17</maven.compiler.target>\n    </properties>\n</project>\n",
                clean_name
            );
            fs::write(target_dir.join("pom.xml"), pom_xml)
                .map_err(|e| format!("Failed to write pom.xml: {}", e))?;

            let main_java = format!(
                "package com.example;\n\npublic class Main {{\n    public static void main(String[] args) {{\n        System.out.println(\"Hello from {}!\");\n    }}\n}}\n",
                clean_name
            );
            fs::write(src_dir.join("Main.java"), main_java)
                .map_err(|e| format!("Failed to write Main.java: {}", e))?;
        }
    }

    // Synchronize workspace immediately with newly created project
    graph.sync_workspace(&target_dir);
    let files_synced = graph.files.len();
    let created_files: Vec<String> = graph
        .files
        .keys()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    let message = if let Some(ref warn) = warning_note {
        format!(
            "Successfully bootstrapped '{}' using {} with {} files. Workspace indexed immediately. (Note: {})",
            clean_name,
            preset.as_str(),
            files_synced,
            warn
        )
    } else {
        format!(
            "Successfully bootstrapped '{}' using {} with {} files. Workspace indexed immediately.",
            clean_name,
            preset.as_str(),
            files_synced
        )
    };

    Ok(ScaffoldResult {
        success: true,
        template: preset.as_str().to_string(),
        template_version: "ecosystem".to_string(),
        project_name: clean_name.to_string(),
        destination: target_dir.display().to_string(),
        created_files,
        files_synced,
        message,
        environment_note: warning_note,
    })
}

/// Scaffold a multi-file architectural module, write clean files to disk,
/// synchronize the workspace graph, validate cross-file imports, and return symbol handles.
pub fn scaffold_module(
    root: &Path,
    req: ScaffoldModuleRequest,
    graph: &mut VectorGraph,
) -> Result<ScaffoldModuleResult, String> {
    if req.module_name.trim().is_empty() {
        return Err("Module name cannot be empty.".to_string());
    }
    if req.files.is_empty() {
        return Err("ScaffoldModuleRequest requires at least one file specification.".to_string());
    }

    let mut created_paths = Vec::new();

    for file_spec in &req.files {
        let trimmed_path = file_spec.path.trim();
        if trimmed_path.is_empty() {
            return Err("File path in module specification cannot be empty.".to_string());
        }

        let abs_path = root.join(trimmed_path);
        if let Some(parent) = abs_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory '{}': {}", parent.display(), e))?;
        }

        let ext = Path::new(trimmed_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let file_content = if let Some(ref raw) = file_spec.raw_content {
            raw.clone()
        } else {
            let mut buf = String::new();

            if matches!(ext.as_str(), "h" | "hpp" | "hxx") {
                buf.push_str("#pragma once\n\n");
            }

            // Header imports
            if !file_spec.imports.is_empty() {
                for imp in &file_spec.imports {
                    let imp_trimmed = imp.trim();
                    if !imp_trimmed.is_empty() {
                        buf.push_str(imp_trimmed);
                        if matches!(
                            ext.as_str(),
                            "ts" | "tsx" | "js" | "jsx" | "rs" | "cs" | "java"
                        ) && !imp_trimmed.ends_with(';')
                        {
                            buf.push(';');
                        }
                        buf.push('\n');
                    }
                }
                buf.push('\n');
            }

            // Types / Interfaces / Structs
            for t in &file_spec.types {
                let def_trimmed = t.definition.trim();
                if !def_trimmed.is_empty() {
                    buf.push_str(def_trimmed);
                    buf.push_str("\n\n");
                }
            }

            // Functions / Components / Stubs
            for f in &file_spec.functions {
                let sig_trimmed = f.signature.trim();
                let default_stub = match ext.as_str() {
                    "py" | "pyi" => "    raise NotImplementedError(\"TODO: implement\")",
                    "go" => "{\n\tpanic(\"TODO: implement\")\n}",
                    "rs" => "{\n    todo!()\n}",
                    "swift" => "{\n    fatalError(\"TODO: implement\")\n}",
                    "kt" | "kts" => "{\n    TODO(\"implement\")\n}",
                    "cpp" | "cc" => "{\n    throw std::runtime_error(\"TODO: implement\");\n}",
                    "cs" => "{\n    throw new NotImplementedException();\n}",
                    "java" => {
                        "{\n    throw new UnsupportedOperationException(\"TODO: implement\");\n}"
                    }
                    _ => "{\n  // TODO: implement\n}",
                };

                let stub = f.stub_body.as_deref().unwrap_or(default_stub);

                if ext == "py" || ext == "pyi" {
                    let full_sig = if sig_trimmed.ends_with(':') {
                        sig_trimmed.to_string()
                    } else {
                        format!("{}:", sig_trimmed)
                    };
                    buf.push_str(&full_sig);
                    buf.push('\n');
                    if let Some(ref doc) = f.doc_comment {
                        buf.push_str(&format!("    \"\"\"{}\"\"\"\n", doc.trim()));
                    }
                    buf.push_str(stub);
                    buf.push_str("\n\n");
                } else {
                    if let Some(ref doc) = f.doc_comment {
                        buf.push_str("/**\n");
                        for line in doc.lines() {
                            buf.push_str(&format!(" * {}\n", line));
                        }
                        buf.push_str(" */\n");
                    }
                    buf.push_str(&format!("{} {}\n\n", sig_trimmed, stub));
                }
            }

            buf
        };

        fs::write(&abs_path, file_content).map_err(|e| {
            format!(
                "Failed to write scaffolded file '{}': {}",
                abs_path.display(),
                e
            )
        })?;

        created_paths.push(trimmed_path.to_string());
    }

    // Immediately synchronize workspace graph
    graph.sync_workspace(root);

    // Extract symbol handles from updated VectorGraph
    let mut symbols_created = Vec::new();
    for path_str in &created_paths {
        let p = std::path::PathBuf::from(path_str);
        if let Some(node) = graph.files.get(&p) {
            for sym in &node.symbols {
                symbols_created.push(ScaffoldedSymbolHandle {
                    file: path_str.clone(),
                    symbol_name: sym.name.clone(),
                    kind: sym.kind.to_string(),
                    lines: [sym.start_line, sym.end_line],
                    signature: sym.signature.clone(),
                });
            }
        }
    }

    Ok(ScaffoldModuleResult {
        success: true,
        module_name: req.module_name.clone(),
        created_files: created_paths.clone(),
        symbols_created,
        files_synced: graph.files.len(),
        message: format!(
            "Successfully scaffolded module '{}' across {} file(s). All symbols and dependency edges indexed into graph.",
            req.module_name,
            created_paths.len()
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
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .status();

        if let Ok(st) = status {
            assert!(st.success(), "scaffolded project failed cargo check");
        }
    }

    #[test]
    fn test_generator_preset_parsing() {
        assert_eq!(
            GeneratorPreset::parse("react-ts"),
            Some(GeneratorPreset::ReactTs)
        );
        assert_eq!(
            GeneratorPreset::parse("vite-react-ts"),
            Some(GeneratorPreset::ReactTs)
        );
        assert_eq!(
            GeneratorPreset::parse("nextjs"),
            Some(GeneratorPreset::NextJs)
        );
        assert_eq!(
            GeneratorPreset::parse("next-ts"),
            Some(GeneratorPreset::NextJs)
        );
        assert_eq!(
            GeneratorPreset::parse("rust-bin"),
            Some(GeneratorPreset::RustBin)
        );
        assert_eq!(
            GeneratorPreset::parse("cargo-bin"),
            Some(GeneratorPreset::RustBin)
        );
        assert_eq!(GeneratorPreset::parse("unknown_preset"), None);
        assert_eq!(
            GeneratorPreset::parse_or_custom("svelte"),
            GeneratorPreset::Custom("svelte".to_string())
        );
    }

    #[test]
    fn test_bootstrap_project_rust_bin_and_subsequent_incremental_indexing() {
        let dir = tempdir().unwrap();
        let mut graph = VectorGraph::new();

        // 1. Bootstrap project
        let res = bootstrap_project(
            GeneratorPreset::RustBin,
            "bootstrapped_app",
            dir.path(),
            None,
            &mut graph,
        )
        .expect("bootstrap must succeed");

        assert!(res.success);
        let target_dir = dir.path().join("bootstrapped_app");
        assert!(target_dir.join("Cargo.toml").exists());
        assert!(target_dir.join("src").join("main.rs").exists());

        // Verified immediate indexing
        assert!(
            graph
                .files
                .contains_key(&std::path::PathBuf::from("Cargo.toml"))
        );
        assert!(
            graph
                .files
                .contains_key(&std::path::PathBuf::from("src/main.rs"))
        );

        // 2. Reject repeat invocation on same destination
        let repeat = bootstrap_project(
            GeneratorPreset::RustBin,
            "bootstrapped_app",
            dir.path(),
            None,
            &mut graph,
        );
        assert!(repeat.is_err(), "Must reject non-empty destination");

        // 3. Add a new file and ensure sync updates graph correctly
        let util_file = target_dir.join("src").join("utils.rs");
        fs::write(&util_file, "pub fn helper() -> bool { true }").unwrap();

        graph.sync_workspace(&target_dir);
        assert!(
            graph
                .files
                .contains_key(&std::path::PathBuf::from("src/utils.rs"))
        );
        let syms = graph.find_symbol_filtered("helper", None, None);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].1.name, "helper");
    }
}
