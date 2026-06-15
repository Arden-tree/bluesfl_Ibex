use crate::agent::block::toolcall_tools::SignalRef;
use crate::wave::mgr::WaveformManager;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ── Navigation state ─────────────────────────────────────────────────

#[derive(Clone)]
pub struct NavBlockInfo {
    pub module: String,
    pub scope: Vec<String>,
    pub code: String,
    pub signals: Vec<(String, i64)>,
    pub bid: u64,
}

pub struct NavState {
    pub nav_map: HashMap<String, NavBlockInfo>,
    pub current: NavBlockInfo,
    pub waveform_mgr: WaveformManager,
    pub suspicious: Vec<(String, u64)>,
    pub done: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum NavError {
    #[error("{0}")]
    Msg(String),
}

// ── read_values ──────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct NavReadArgs {
    pub signals: Vec<SignalRef>,
}

pub struct NavReadValues {
    pub state: Arc<Mutex<NavState>>,
}

impl NavReadValues {
    pub fn new(state: Arc<Mutex<NavState>>) -> Self {
        Self { state }
    }
}

impl Tool for NavReadValues {
    const NAME: &'static str = "read_values";
    type Error = NavError;
    type Args = NavReadArgs;
    type Output = String;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read signal values from the waveform at specific times. Provide signal names and times.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "signals": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string", "description": "Signal name"},
                                "time": {"type": "integer", "description": "Time (clock cycle)"}
                            },
                            "required": ["name", "time"]
                        },
                        "description": "List of signals to read"
                    }
                },
                "required": ["signals"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let state = self.state.lock().map_err(|e| NavError::Msg(e.to_string()))?;
        let scope: Vec<String> = state.current.scope.clone();
        let mut mgr = state.waveform_mgr.clone();
        drop(state);

        let mut results = Vec::new();
        for sig in &args.signals {
            let var: [String; 1] = [sig.name.clone()];
            match mgr.get_signal_value_at_time(&scope, &var, sig.time) {
                Ok(values) => {
                    for (name, (value, bw, _)) in values {
                        results.push(serde_json::json!({
                            "signal_name": name, "time": sig.time,
                            "bit-width": bw, "value": value
                        }));
                    }
                }
                Err(e) => {
                    results.push(serde_json::json!({
                        "signal_name": sig.name, "time": sig.time, "error": e.to_string()
                    }));
                }
            }
        }
        Ok(serde_json::to_string_pretty(&results).unwrap_or_default())
    }
}

// ── check_signals (navigation) ───────────────────────────────────────

#[derive(Deserialize)]
pub struct NavCheckArgs {
    pub signals: Vec<String>,
}

pub struct NavCheckSignals {
    pub state: Arc<Mutex<NavState>>,
}

impl NavCheckSignals {
    pub fn new(state: Arc<Mutex<NavState>>) -> Self {
        Self { state }
    }
}

impl Tool for NavCheckSignals {
    const NAME: &'static str = "check_signals";
    type Error = NavError;
    type Args = NavCheckArgs;
    type Output = String;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Navigate to the code blocks driving the selected signals. Provide signal names to trace backward through the design.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "signals": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Signal names to trace backward"
                    }
                },
                "required": ["signals"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut state = self.state.lock().map_err(|e| NavError::Msg(e.to_string()))?;
        for sig_name in &args.signals {
            if let Some(next) = state.nav_map.get(sig_name).cloned() {
                let module = next.module.clone();
                let code = next.code.clone();
                let signals = next.signals.clone();
                let bid = next.bid;
                state.current = next;
                drop(state);
                return Ok(format!(
                    "Navigated to module {} (block {}).\nCode:\n{}\nDriven signals: {:?}",
                    module, bid, code, signals
                ));
            }
        }
        let keys: Vec<&String> = state.nav_map.keys().take(20).collect();
        Ok(format!(
            "Signal(s) {:?} not found in trace path. Available signals: {:?}",
            args.signals, keys
        ))
    }
}

// ── append_block ─────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct NavEmptyArgs {}

pub struct NavAppendBlock {
    pub state: Arc<Mutex<NavState>>,
}

impl NavAppendBlock {
    pub fn new(state: Arc<Mutex<NavState>>) -> Self {
        Self { state }
    }
}

impl Tool for NavAppendBlock {
    const NAME: &'static str = "append_block";
    type Error = NavError;
    type Args = NavEmptyArgs;
    type Output = String;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Mark the current code block as suspicious (contains the root cause bug).".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    async fn call(&self, _: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut state = self.state.lock().map_err(|e| NavError::Msg(e.to_string()))?;
        let (module, bid) = (state.current.module.clone(), state.current.bid);
        state.suspicious.push((module.clone(), bid));
        Ok(format!("Marked block {} ({}) as suspicious", bid, module))
    }
}

// ── exit ─────────────────────────────────────────────────────────────

pub struct NavExit {
    pub state: Arc<Mutex<NavState>>,
}

impl NavExit {
    pub fn new(state: Arc<Mutex<NavState>>) -> Self {
        Self { state }
    }
}

impl Tool for NavExit {
    const NAME: &'static str = "exit";
    type Error = NavError;
    type Args = NavEmptyArgs;
    type Output = String;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "End the debugging analysis.".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    async fn call(&self, _: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut state = self.state.lock().map_err(|e| NavError::Msg(e.to_string()))?;
        state.done = true;
        Ok("Analysis ended.".to_string())
    }
}
