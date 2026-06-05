use crate::wave::mgr::WaveformManager;
use crate::TimeAnnotation;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

// ── Shared state between tools and BlockCheckerToolAgent ──────────────

#[derive(Debug, Default)]
pub struct ToolCallState {
    pub suspicious: bool,
    pub terminate: bool,
    pub checked_signals: Vec<(String, TimeAnnotation)>,
}

// ── Error types ───────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ToolCallError {
    #[error("Waveform error: {0}")]
    Waveform(String),
    #[error("Invalid arguments: {0}")]
    InvalidArgs(String),
}

// ── read_values tool ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ReadValuesArgs {
    pub signals: Vec<SignalRef>,
}

#[derive(Deserialize)]
pub struct SignalRef {
    pub name: String,
    pub time: TimeAnnotation,
}

#[derive(Serialize)]
pub struct SignalValue {
    pub signal_name: String,
    pub time: TimeAnnotation,
    #[serde(rename = "bit-width")]
    pub bit_width: String,
    pub value: String,
}

pub struct ReadValuesTool {
    waveform_mgr: Arc<Mutex<WaveformManager>>,
    scope: Vec<String>,
}

impl ReadValuesTool {
    pub fn new(waveform_mgr: Arc<Mutex<WaveformManager>>, scope: &[&str]) -> Self {
        Self {
            waveform_mgr,
            scope: scope.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl Tool for ReadValuesTool {
    const NAME: &'static str = "read_values";
    type Error = ToolCallError;
    type Args = ReadValuesArgs;
    type Output = Vec<SignalValue>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Read signal values from the waveform at specific times. Provide a list of signal names and times."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "signals": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string", "description": "Signal name" },
                                "time": { "type": "integer", "description": "Time (clock cycle)" }
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
        let mut mgr = self.waveform_mgr.lock().map_err(|e| ToolCallError::Waveform(e.to_string()))?;
        let mut results = Vec::new();
        for sig in &args.signals {
            let var_names: Vec<String> = vec![sig.name.clone()];
            match mgr.get_signal_value_at_time(&self.scope, &var_names, sig.time) {
                Ok(values) => {
                    for (name, (value, bit_width, _time)) in values {
                        results.push(SignalValue {
                            signal_name: name,
                            time: sig.time,
                            bit_width,
                            value,
                        });
                    }
                }
                Err(e) => {
                    log::warn!("read_values: failed to read signal '{}' at time {}: {}", sig.name, sig.time, e);
                    results.push(SignalValue {
                        signal_name: sig.name.clone(),
                        time: sig.time,
                        bit_width: "unknown".to_string(),
                        value: format!("Error: {}", e),
                    });
                }
            }
        }
        Ok(results)
    }
}

// ── check_signals tool ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CheckSignalsArgs {
    pub signals: Vec<SignalRef>,
}

#[derive(Serialize)]
pub struct CheckSignalsOutput {
    status: String,
    signals_count: usize,
}

pub struct CheckSignalsTool {
    state: Arc<Mutex<ToolCallState>>,
}

impl CheckSignalsTool {
    pub fn new(state: Arc<Mutex<ToolCallState>>) -> Self {
        Self { state }
    }
}

impl Tool for CheckSignalsTool {
    const NAME: &'static str = "check_signals";
    type Error = ToolCallError;
    type Args = CheckSignalsArgs;
    type Output = CheckSignalsOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Select upstream signals to trace further. The signals must be from the provided driven signals list."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "signals": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string", "description": "Signal name to check upstream" },
                                "time": { "type": "integer", "description": "Time of the signal" }
                            },
                            "required": ["name", "time"]
                        },
                        "description": "Signals selected for upstream tracing"
                    }
                },
                "required": ["signals"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut state = self.state.lock().map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
        let count = args.signals.len();
        for sig in args.signals {
            state.checked_signals.push((sig.name, sig.time));
        }
        log::info!("check_signals: {} signals selected for upstream tracing", count);
        Ok(CheckSignalsOutput {
            status: "ok".to_string(),
            signals_count: count,
        })
    }
}

// ── append_block tool ─────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AppendBlockArgs {
    pub reason: String,
}

#[derive(Serialize)]
pub struct AppendBlockOutput {
    status: String,
}

pub struct AppendBlockTool {
    state: Arc<Mutex<ToolCallState>>,
}

impl AppendBlockTool {
    pub fn new(state: Arc<Mutex<ToolCallState>>) -> Self {
        Self { state }
    }
}

impl Tool for AppendBlockTool {
    const NAME: &'static str = "append_block";
    type Error = ToolCallError;
    type Args = AppendBlockArgs;
    type Output = AppendBlockOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Mark the current code block as suspicious (likely contains the bug). Provide a reason."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "reason": { "type": "string", "description": "Why this block is suspicious" }
                },
                "required": ["reason"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut state = self.state.lock().map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
        state.suspicious = true;
        log::info!("append_block: marked as suspicious, reason: {}", args.reason);
        Ok(AppendBlockOutput {
            status: "ok".to_string(),
        })
    }
}

// ── exit tool ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ExitArgs {
    pub reason: String,
}

#[derive(Serialize)]
pub struct ExitOutput {
    status: String,
}

pub struct ExitTool {
    state: Arc<Mutex<ToolCallState>>,
}

impl ExitTool {
    pub fn new(state: Arc<Mutex<ToolCallState>>) -> Self {
        Self { state }
    }
}

impl Tool for ExitTool {
    const NAME: &'static str = "exit";
    type Error = ToolCallError;
    type Args = ExitArgs;
    type Output = ExitOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Terminate the debugging process. Call this when you have identified the root cause or exhausted all leads."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "reason": { "type": "string", "description": "Why you are terminating the analysis" }
                },
                "required": ["reason"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut state = self.state.lock().map_err(|e| ToolCallError::InvalidArgs(e.to_string()))?;
        state.terminate = true;
        log::info!("exit: terminating analysis, reason: {}", args.reason);
        Ok(ExitOutput {
            status: "ok".to_string(),
        })
    }
}
