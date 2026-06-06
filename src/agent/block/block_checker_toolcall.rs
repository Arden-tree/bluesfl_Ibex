use crate::agent::block::toolcall_tools::{
    AppendBlockTool, CheckSignalsTool, ExitTool, ReadValuesTool, ToolCallState,
};
use crate::{prompt_args, Block, NodeID, TimeAnnotation};
use async_trait::async_trait;
use log::{info, warn};
use rig::agent::AgentBuilder;
use rig::completion::{CompletionModel, Prompt, Usage};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use super::block_checker::BlockChecker;

/// BlockChecker that uses rig's multi-turn tool-call instead of voting.
///
/// The LLM interacts via 4 tools (read_values, check_signals, append_block, exit)
/// as described in the paper Section 3.4.
pub struct BlockCheckerToolAgent<M: CompletionModel> {
    system_prompt: String,
    model: M,
    waveform_mgr: crate::wave::mgr::WaveformManager,
    prompt_template: String,
    token_usage: Arc<Mutex<Usage>>,
}

impl<M: CompletionModel> Clone for BlockCheckerToolAgent<M>
where
    M: Clone,
{
    fn clone(&self) -> Self {
        Self {
            system_prompt: self.system_prompt.clone(),
            model: self.model.clone(),
            waveform_mgr: self.waveform_mgr.clone(),
            prompt_template: self.prompt_template.clone(),
            token_usage: self.token_usage.clone(),
        }
    }
}

impl<M: CompletionModel> BlockCheckerToolAgent<M> {
    pub fn new(
        system_prompt_path: &str,
        prompt_template_path: &str,
        model: M,
        waveform_path: &str,
        token_usage: Arc<Mutex<Usage>>,
    ) -> Self {
        let system_prompt = std::fs::read_to_string(system_prompt_path)
            .unwrap_or_else(|e| panic!("Failed to read system prompt {}: {}", system_prompt_path, e));
        let prompt_template = std::fs::read_to_string(prompt_template_path)
            .unwrap_or_else(|e| panic!("Failed to read prompt template {}: {}", prompt_template_path, e));
        Self {
            system_prompt,
            model,
            waveform_mgr: crate::wave::mgr::WaveformManager::new(waveform_path),
            prompt_template,
            token_usage,
        }
    }
}

#[async_trait]
impl<'a, B, M: CompletionModel + Clone + Send + Sync> BlockChecker<'a, B>
    for BlockCheckerToolAgent<M>
where
    B: Block<'a> + Sync + Send,
    M: 'static,
{
    async fn determine(
        &mut self,
        block: &B,
        _port_nodes: &[(NodeID, TimeAnnotation)],
        input_nodes: &[(NodeID, TimeAnnotation)],
        sig: NodeID,
        sig_time: TimeAnnotation,
        appendix_info: &str,
        _module_knowledge: &str,
        _historical_suspicious_blocks: &Vec<B>,
    ) -> anyhow::Result<(Option<Vec<(NodeID, TimeAnnotation)>>, bool, bool)> {
        let block_code = block.get_ctx().join("\n");
        let cur_scope: Vec<&str> = block.get_scope().split(".").collect();

        // Read the suspicious output signal value (still provided upfront)
        let sig_name = sig.get_text();
        let sig_value = self
            .waveform_mgr
            .display_signal_values_at_time_json(&cur_scope, &vec![sig_name], sig_time)
            .unwrap_or_else(|e| {
                warn!("Failed to read sig value: {}", e);
                format!("{{\"error\": \"{}\"}}", e)
            });

        // Build driven signals list with values (aligned with paper Figure 3)
        // LLM can also use read_values tool to inspect additional signals
        let input_var_names_with_t: Vec<_> = input_nodes
            .iter()
            .map(|(n, t)| (n.get_text(), t))
            .collect();
        let driven_signals_json = if let Ok(ret) = self
            .waveform_mgr
            .display_signal_values_with_batch_json(&cur_scope, &input_var_names_with_t, false)
        {
            ret
        } else {
            warn!("Failed to read driven signal values, falling back to names only");
            serde_json::to_string_pretty(
                &input_nodes
                    .iter()
                    .map(|(node, t)| {
                        serde_json::json!({
                            "name": node.get_text(),
                            "time": t
                        })
                    })
                    .collect::<Vec<_>>(),
            )?
        };

        // Build the prompt using the template
        let args = prompt_args![
            "scenario" => appendix_info,
            "module_name" => block.get_module_name(),
            "block_code" => block_code,
            "sig_value" => sig_value,
            "driven_signals" => driven_signals_json
        ];
        let mut prompt = self.prompt_template.clone();
        for (key, value) in &args {
            let placeholder = format!("{{{}}}", key);
            prompt = prompt.replace(&placeholder, value);
        }

        // Create shared state
        let state = Arc::new(Mutex::new(ToolCallState::default()));

        // Build agent with tools
        let waveform_arc = Arc::new(Mutex::new(self.waveform_mgr.clone()));
        let agent = AgentBuilder::new(self.model.clone())
            .preamble(&self.system_prompt)
            .temperature(0.0)
            .tool(ReadValuesTool::new(waveform_arc, &cur_scope))
            .tool(CheckSignalsTool::new(state.clone()))
            .tool(AppendBlockTool::new(state.clone()))
            .tool(ExitTool::new(state.clone()))
            .build();

        // Multi-turn tool-call (up to 20 rounds)
        let result = agent
            .prompt(&prompt)
            .multi_turn(20)
            .extended_details()
            .await;

        match result {
            Ok(response) => {
                if let Ok(mut usage) = self.token_usage.lock() {
                    *usage += response.total_usage;
                }
                info!("ToolCall agent final response: {}", response.output);
            }
            Err(e) => {
                warn!("ToolCall agent error (may have hit max depth): {}", e);
            }
        }

        // Read final state from tools
        let s = state.lock().unwrap();
        let suspicious = s.suspicious;
        let terminate = s.terminate;

        info!(
            "ToolCall result: suspicious={}, terminate={}, checked_signals={}",
            suspicious,
            terminate,
            s.checked_signals.len()
        );

        // Per paper Section 3.4: exit ends the LLM session for this block,
        // but check_signals should still feed into the BFS for upstream tracing.
        // Only terminate the BFS branch when terminate=true AND no checked_signals.
        if terminate && s.checked_signals.is_empty() {
            return Ok((None, suspicious, terminate));
        }

        // Match checked_signals back to input_nodes (same logic as block_checker.rs)
        let mut selected_nodes: Vec<(NodeID, TimeAnnotation)> = Vec::new();

        for (name, time) in &s.checked_signals {
            let clean_name = name.find("[").map(|pos| &name[..pos]).unwrap_or(name.as_str());

            // Try exact (name, time) match
            let matched: Vec<_> = input_nodes
                .iter()
                .filter(|(node, t)| node.get_text() == clean_name && *t == *time)
                .collect();

            let matched = if matched.is_empty() {
                // Fallback: name-only match
                input_nodes
                    .iter()
                    .filter(|(node, _)| node.get_text() == clean_name)
                    .collect::<Vec<_>>()
            } else {
                matched
            };

            for (node, t) in matched {
                selected_nodes.push((node.clone(), *t));
            }
        }

        // Deduplicate
        let mut seen = HashSet::new();
        let selected_nodes: Vec<_> = selected_nodes
            .into_iter()
            .filter(|(n, t)| seen.insert((n.get_text().to_string(), *t)))
            .collect();

        if selected_nodes.is_empty() && !s.checked_signals.is_empty() {
            warn!(
                "ToolCall: LLM selected {} signals but none matched dataflow nodes",
                s.checked_signals.len()
            );
        }

        // When terminate=true but we have checked_signals, don't propagate terminate
        // so the tracer will actually use these signals for BFS continuation.
        let should_terminate = terminate && selected_nodes.is_empty();

        if selected_nodes.is_empty() {
            Ok((None, suspicious, should_terminate))
        } else {
            Ok((Some(selected_nodes), suspicious, should_terminate))
        }
    }
}
