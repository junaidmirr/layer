use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ToolMetric {
    pub call_count: usize,
    pub total_execution_ms: u128,
    pub estimated_tokens_saved: usize,
    pub llm_turns_saved: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TelemetryTracker {
    pub total_tool_calls: usize,
    pub surgical_ast_reads: usize,
    pub estimated_tokens_saved: usize,
    pub ast_precheck_catches: usize,
    pub circuit_breaker_saves: usize,
    pub package_apis_inspected: usize,
    pub cross_file_renames: usize,
    pub selective_test_runs: usize,
    pub llm_turns_saved: usize,
    pub total_execution_ms: u128,
    pub tool_breakdown: BTreeMap<String, ToolMetric>,
}

impl TelemetryTracker {
    pub fn new() -> Self {
        Self::default()
    }

    fn telemetry_file_path(workspace_root: &Path) -> PathBuf {
        workspace_root.join(".layer").join("telemetry.json")
    }

    /// Load telemetry from `<workspace>/.layer/telemetry.json` or return default
    pub fn load(workspace_root: &Path) -> Self {
        let p = Self::telemetry_file_path(workspace_root);
        if p.exists()
            && let Ok(data) = fs::read_to_string(&p)
            && let Ok(tracker) = serde_json::from_str::<Self>(&data)
        {
            return tracker;
        }
        Self::default()
    }

    /// Save telemetry to `<workspace>/.layer/telemetry.json`
    pub fn save(&self, workspace_root: &Path) -> Result<(), String> {
        let dir = workspace_root.join(".layer");
        if !dir.exists() {
            let _ = fs::create_dir_all(&dir);
        }
        let p = dir.join("telemetry.json");
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize telemetry: {}", e))?;
        fs::write(p, json).map_err(|e| format!("Failed to write telemetry: {}", e))
    }

    /// Reset all telemetry counters and persist
    pub fn reset(&mut self, workspace_root: &Path) -> Result<(), String> {
        *self = Self::default();
        self.save(workspace_root)
    }

    /// Record a tool execution with performance and token savings metrics
    pub fn record_tool_call(
        &mut self,
        tool_name: &str,
        elapsed_ms: u128,
        tokens_saved: usize,
        turns_saved: usize,
    ) {
        self.total_tool_calls += 1;
        self.total_execution_ms += elapsed_ms;
        self.estimated_tokens_saved += tokens_saved;
        self.llm_turns_saved += turns_saved;

        match tool_name {
            "find_symbol"
            | "get_symbol_body"
            | "get_file_skeleton"
            | "get_callers"
            | "find_references"
            | "get_impact_radius"
            | "get_lines"
            | "get_tree"
            | "search"
            | "prepare_task_context" => {
                self.surgical_ast_reads += 1;
            }
            "inspect_package_api" => {
                self.package_apis_inspected += 1;
            }
            "rename_symbol" => {
                self.cross_file_renames += 1;
            }
            "verify_impacted_tests" => {
                self.selective_test_runs += 1;
            }
            _ => {}
        }

        let entry = self
            .tool_breakdown
            .entry(tool_name.to_string())
            .or_default();
        entry.call_count += 1;
        entry.total_execution_ms += elapsed_ms;
        entry.estimated_tokens_saved += tokens_saved;
        entry.llm_turns_saved += turns_saved;
    }

    /// Record an AST precheck catch
    pub fn record_ast_catch(&mut self) {
        self.ast_precheck_catches += 1;
        self.estimated_tokens_saved += 4000;
        self.llm_turns_saved += 1;
    }

    /// Record a circuit breaker rollback save
    pub fn record_circuit_breaker_save(&mut self) {
        self.circuit_breaker_saves += 1;
        self.estimated_tokens_saved += 4000;
        self.llm_turns_saved += 1;
    }

    /// Render a clean Unicode box-drawn terminal dashboard
    pub fn format_dashboard(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "┌──────────────────────────────────────────────────────────────────────────────┐\n",
        );
        out.push_str(
            "│                     LAYER v1.1 — ROI & SESSION TELEMETRY                     │\n",
        );
        out.push_str(
            "├──────────────────────────────────────────────────────────────────────────────┤\n",
        );
        out.push_str(&format!(
            "│  Total Tool Calls:        {:>8}   │  Total Execution Time:    {:>6} ms    │\n",
            self.total_tool_calls, self.total_execution_ms
        ));
        out.push_str(&format!(
            "│  Est. Tokens Saved:       {:>8}   │  LLM Turns Saved:         {:>6}       │\n",
            self.estimated_tokens_saved, self.llm_turns_saved
        ));
        out.push_str(&format!(
            "│  Surgical AST Reads:      {:>8}   │  AST Pre-check Catches:   {:>6}       │\n",
            self.surgical_ast_reads, self.ast_precheck_catches
        ));
        out.push_str(&format!(
            "│  Package APIs Inspected:  {:>8}   │  Circuit Breaker Saves:   {:>6}       │\n",
            self.package_apis_inspected, self.circuit_breaker_saves
        ));
        out.push_str(&format!(
            "│  Cross-File Renames:      {:>8}   │  Selective Test Runs:     {:>6}       │\n",
            self.cross_file_renames, self.selective_test_runs
        ));
        out.push_str(
            "├──────────────────────────────────────────────────────────────────────────────┤\n",
        );
        out.push_str(
            "│ Tool Breakdown:                                                              │\n",
        );
        out.push_str(
            "│   Tool Name                Calls   Avg Time (ms)   Turns Saved  Tokens Saved │\n",
        );
        out.push_str(
            "│   ───────────────────────  ─────   ─────────────   ───────────  ──────────── │\n",
        );

        if self.tool_breakdown.is_empty() {
            out.push_str("│   (No tools called yet in this session)                                      │\n");
        } else {
            for (tool, metric) in &self.tool_breakdown {
                let avg_ms = if metric.call_count > 0 {
                    metric.total_execution_ms / metric.call_count as u128
                } else {
                    0
                };
                out.push_str(&format!(
                    "│   {:<23}  {:>5}   {:>13}   {:>11}  {:>12} │\n",
                    tool,
                    metric.call_count,
                    avg_ms,
                    metric.llm_turns_saved,
                    metric.estimated_tokens_saved
                ));
            }
        }

        out.push_str(
            "└──────────────────────────────────────────────────────────────────────────────┘\n",
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_telemetry_tracker_lifecycle() {
        let dir = tempdir().unwrap();
        let mut tracker = TelemetryTracker::load(dir.path());
        assert_eq!(tracker.total_tool_calls, 0);

        tracker.record_tool_call("find_symbol", 5, 800, 0);
        tracker.record_tool_call("inspect_package_api", 12, 2500, 1);
        tracker.record_ast_catch();
        tracker.record_circuit_breaker_save();

        assert_eq!(tracker.total_tool_calls, 2);
        assert_eq!(tracker.surgical_ast_reads, 1);
        assert_eq!(tracker.package_apis_inspected, 1);
        assert_eq!(tracker.ast_precheck_catches, 1);
        assert_eq!(tracker.circuit_breaker_saves, 1);
        assert_eq!(tracker.llm_turns_saved, 3);
        assert!(tracker.estimated_tokens_saved >= 11300);

        tracker.save(dir.path()).unwrap();
        assert!(dir.path().join(".layer/telemetry.json").exists());

        let loaded = TelemetryTracker::load(dir.path());
        assert_eq!(loaded, tracker);

        let dashboard = tracker.format_dashboard();
        assert!(dashboard.contains("LAYER v1.1"));
        assert!(dashboard.contains("find_symbol"));
        assert!(dashboard.contains("inspect_package_api"));

        tracker.reset(dir.path()).unwrap();
        let reset_tracker = TelemetryTracker::load(dir.path());
        assert_eq!(reset_tracker.total_tool_calls, 0);
    }
}
