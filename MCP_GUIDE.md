# `layer` MCP Configuration Guide for Coding Harnesses (Claude Code, Cursor, Codex)

`layer` runs as an ultra-fast in-memory MCP server that acts as a semantic context layer between your AI coding agent and any codebase.

---

## 🛠️ The 7 Core MCP Tools Exposed by `layer`

1. **`get_symbol_body(name, file?)`**:
   - Returns the exact code implementation of a function, method, class, or struct with 1-based line bounds `[start, end]`.
   - *Use this instead of reading or viewing whole files.*
2. **`get_impact_radius(symbol, file?)`**:
   - Returns the 1-hop blast radius of a symbol: who calls it (`callers`) and what functions it calls (`callees`).
   - *Use this before editing or refactoring to see everything that could break.*
3. **`get_file_skeleton(path)`**:
   - Returns imports, function signatures, structs, and line spans without function bodies (saves 75%+ tokens).
4. **`find_symbol(name)`**:
   - Locates declarations of any symbol across all files in the project.
5. **`find_references(symbol)`**:
   - Finds all call sites and references to a symbol across the entire codebase.
6. **`get_lines(path, start, end)`**:
   - Retrieves exact line slices with line numbers.
7. **`search(query, limit?)`**:
   - Hybrid dense vector and keyword search across all code chunks.

---

## 🚀 Setting up with Claude Code

### Option A: Using the Claude CLI (Recommended)
From your project directory:

```bash
claude mcp add -s local layer layer -- mcp -p .
```

Verify that the connection is active:
```bash
claude mcp list
# Output: layer: layer mcp -p . - ✔ Connected
```

### Option B: Direct Configuration in `~/.claude.json` or `.mcp.json`
```json
{
  "mcpServers": {
    "layer": {
      "command": "layer",
      "args": ["mcp", "-p", "."]
    }
  }
}
```

### Step 2: Add Agent Instructions in `CLAUDE.md`
Create or append to `CLAUDE.md` in your project root:

```markdown
# Codebase Navigation Instructions

When exploring and editing this codebase, always use the `layer` MCP tools:
- **Never read entire files with View/ReadFile** if you only need a function. Use `get_symbol_body(name)` or `get_lines(path, start, end)`.
- **To understand a file's structure**, use `get_file_skeleton(path)` first.
- **Before refactoring or changing a function**, call `get_impact_radius(symbol)` to see its callers and callees.
- **To find usages**, use `find_references(symbol)` instead of grep.
```

---

## 🚀 Setting up with Cursor / Windsurf

Add `layer` to your Cursor MCP settings (`cursor/mcp.json`):

```json
{
  "mcpServers": {
    "layer": {
      "command": "layer",
      "args": ["mcp", "-p", "${workspaceFolder}"]
    }
  }
}
```

---

## 🚀 Direct CLI Usage

You can also run every tool directly from your shell with structured JSON output:

```bash
# Get symbol body
layer -p /path/to/project --quiet --json body InsertClipCommand

# Get impact radius
layer -p /path/to/project --quiet --json impact InsertClipCommand

# Get file skeleton
layer -p /path/to/project --quiet --json skeleton src/main.rs

# Find callers
layer -p /path/to/project --quiet --json callers InsertClipCommand
```
