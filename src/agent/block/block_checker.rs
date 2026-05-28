use crate::agent::block::base::BlockAgentBase;
use crate::agent::utils::parse_json_md;
use crate::{prompt_args, Block, NodeID, TimeAnnotation};
use anyhow::anyhow;
use async_trait::async_trait;
use log::{info, warn};
use rand::Rng;
use rig::agent::Agent;
use rig::completion::{CompletionModel, Usage};
use std::sync::{Arc, Mutex};

#[async_trait]
pub trait BlockChecker<'a, B>: Clone {
    /// Determine which node inside a block is suspicious
    async fn determine(
        &mut self,
        block: &B,
        port_nodes: &[(NodeID, TimeAnnotation)],
        nodes: &[(NodeID, TimeAnnotation)],
        sig: NodeID,
        sig_time: TimeAnnotation,
        appendix_info: &str,
        module_knowledge: &str,
        historical_suspicious_blocks: &Vec<B>,
    ) -> anyhow::Result<(Option<Vec<(NodeID, TimeAnnotation)>>, bool, bool)>
    where
        B: Block<'a> + Sync + Send;
}

#[derive(Clone)]
pub struct BlockCheckerAgent<M: CompletionModel> {
    base: BlockAgentBase<M>,
}

impl<M: CompletionModel> BlockCheckerAgent<M> {
    pub fn new(
        prompt_path: &str,
        llm: Agent<M>,
        waveform_path: &str,
        token_usage: Arc<Mutex<Usage>>,
    ) -> Self {
        BlockCheckerAgent {
            base: BlockAgentBase::new(prompt_path, llm, waveform_path, token_usage),
        }
    }
}

// impl<M: CompletionModel> LLMApp for BlockCheckerAgent<M> {
//     fn get_prompt(&self) -> Box<dyn FormatPrompter> {
//         let topic_prompt = fs::read_to_string(&self.base.prompt_path).unwrap();
//         let prompt = message_formatter![fmt_template!(HumanMessagePromptTemplate::new(
//             template_fstring!(
//                 topic_prompt,
//                 "module_name",
//                 "block_code",
//                 "sig_name",
//                 "sig_value",
//                 "sig_time",
//                 "input_wave",
//                 "port_wave",
//                 "scenario",
//                 "module_knowledge",
//                 "suspicious_list",
//             )
//         ))];
//         Box::new(prompt)
//     }
//
//     fn get_llm(&self) -> Box<dyn LLM> {
//         self.base.llm.clone_box()
//     }
//
//     fn token_usage(&self) -> TokenUsage {
//         let token_usage = self.token_usage.lock().unwrap();
//         token_usage.clone()
//     }
//
//     fn add_token_usage(&self, usage: Option<TokenUsage>) {
//         let mut token_usage = self.token_usage.lock().unwrap();
//         if usage.as_ref().is_some() {
//             let usage = usage.unwrap();
//
//             token_usage.completion_tokens += usage.completion_tokens;
//             token_usage.prompt_tokens += usage.prompt_tokens;
//             token_usage.total_tokens += usage.total_tokens;
//         }
//     }
// }

#[async_trait]
impl<'a, B, M: CompletionModel> BlockChecker<'a, B> for BlockCheckerAgent<M>
where
    B: Block<'a> + Sync + Send,
{
    /// return nodes, whether current block is suspicious, whether to terminate
    async fn determine(
        &mut self,
        block: &B,
        port_nodes: &[(NodeID, TimeAnnotation)],
        input_nodes: &[(NodeID, TimeAnnotation)],
        sig: NodeID,
        sig_time: TimeAnnotation,
        appendix_info: &str,
        module_knowledge: &str,
        historical_suspicious_blocks: &Vec<B>,
    ) -> anyhow::Result<(Option<Vec<(NodeID, TimeAnnotation)>>, bool, bool)> {
        let sig_name = sig.get_text();
        let cur_scope = block.get_scope().split(".").collect::<Vec<_>>();
        let sig_value = self.base.waveform_mgr.display_signal_values_at_time_json(
            &cur_scope,
            &vec![sig_name],
            sig_time,
        )?;

        let input_var_nodes_with_t = input_nodes;
        let input_var_names_with_t = input_nodes
            .iter()
            .map(|(n, t)| (n.get_text(), t))
            .collect::<Vec<_>>();
        let input_waveform = if let Ok(ret) = self
            .base
            .waveform_mgr
            .display_signal_values_with_batch_json(&cur_scope, &input_var_names_with_t, false)
        {
            ret
        } else {
            warn!("Error getting signal values @ {:?}", sig_time);
            "Cannot get waveform".to_string()
        };

        let port_names_with_t = port_nodes
            .iter()
            .map(|(n, t)| (n.get_text(), t))
            .collect::<Vec<_>>();
        let _port_waveform = if let Ok(ret) = self
            .base
            .waveform_mgr
            .display_signal_values_with_batch(&cur_scope, &port_names_with_t, false)
        {
            ret
        } else {
            warn!("Error getting signal values @ {:?}", sig_time);
            "Cannot get waveform".to_string()
        };

        // TODO: the code context for a signal is too large. we need dataflow focused code context here.
        let block_code = block.get_ctx().join("\n");
        let _suspicious_list = historical_suspicious_blocks
            .iter()
            .enumerate()
            .map(|(index, b)| {
                let title = format!(
                    "Suspicious Block {} from module {}:\n",
                    index,
                    b.get_module_name()
                );
                let block_ctx = b.get_ctx().join("\n");
                format!("{}{}\n", title, block_ctx)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let args = prompt_args![
            "scenario" => appendix_info,
            "module_knowledge" => module_knowledge,
            "module_name" => block.get_module_name(),
            "block_code" => block_code,
            // "sig_name" => sig_name,
            "sig_value" => sig_value,
            // "sig_time" => sig_time,
            "input_wave" => input_waveform,
            // "port_wave"=> port_waveform,
            // "suspicious_list" => suspicious_list
        ];
        let data = self.base.invoke(&args).await?;
        // response format:
        // {
        //     "dive": true,
        // }
        // {
        //     "dive": false,
        //     "vars": ["s1", "s2"]
        //     "next_time": 31
        // }
        //
        info!("Block Checker llm response: {}", data);
        let json_data = parse_json_md(&data)?;

        let suspicious = json_data
            .get("suspicious")
            .and_then(|suspicious| suspicious.as_bool())
            .unwrap_or(false);

        let terminate = json_data
            .get("terminate")
            .and_then(|terminate| terminate.as_bool());

        if let Some(terminate) = terminate {
            if terminate {
                return Ok((None, suspicious, terminate));
            } else {
                let vars = json_data
                    .get("check_signals")
                    .and_then(|vars| vars.as_array());
                if let Some(vars) = vars {
                    let vars = vars
                        .iter()
                        .filter_map(|v| v.as_object())
                        .collect::<Vec<_>>();
                    let nodes = vars
                        .into_iter()
                        .filter_map(|pair| {
                            pair.get("name").and_then(|name| {
                                pair.get("time").and_then(|time| Some((name, time)))
                            })
                        })
                        .filter_map(|(v, t)| {
                            v.as_str()
                                .and_then(|name| t.as_i64().and_then(|time| Some((name, time))))
                        })
                        .map(|(v, t)| {
                            let v_clean = v.find("[").map(|pos| &v[..pos]).unwrap_or(v);
                            let ret = input_var_nodes_with_t
                                .iter()
                                // here llm ignores which time he wnat to backtrace.
                                .filter(|(node, time)| {
                                    node.get_text() == v_clean && *time == t
                                })
                                .collect::<Vec<_>>();
                            if ret.is_empty() {
                                info!("[out dataflow] LLM request for a signal out of dataflow");
                            }
                            ret
                        })
                        .flatten()
                        .map(|(n, t)| ((*n).clone(), (*t).clone()))
                        .collect::<Vec<_>>();

                    if nodes.is_empty() {
                        warn!("LLM selected some nodes with times, but cannot parse them and return empty list.");
                        return Ok((None, suspicious, terminate));
                    } else {
                        return Ok((Some(nodes), suspicious, terminate));
                    }
                }
            }
        }
        Err(anyhow!(
            "Failed to parse llm response from block checker response {}",
            data
        ))
    }
}

// Mock implementation for testing
#[allow(unused)]
#[derive(Clone)]
pub struct MockBlockCheckerAgent {
    pub terminate_probability: f64, // Probability of terminating (0.0 to 1.0)
    pub suspicious_probability: f64, // Probability of marking block as suspicious (0.0 to 1.0)
    pub max_nodes_to_select: usize, // Maximum number of nodes to select when backtracing
    token_usage: Arc<Mutex<Usage>>,
}

#[allow(unused)]
impl MockBlockCheckerAgent {
    pub fn new(
        terminate_probability: f64,
        suspicious_probability: f64,
        max_nodes_to_select: usize,
    ) -> Self {
        MockBlockCheckerAgent {
            terminate_probability: terminate_probability.clamp(0.0, 1.0),
            suspicious_probability: suspicious_probability.clamp(0.0, 1.0),
            max_nodes_to_select,
            token_usage: Arc::new(Mutex::new(Usage::default())),
        }
    }

    /// Create a mock with balanced behavior for general testing
    pub fn new_balanced() -> Self {
        Self::new(0.3, 0.4, 3) // 30% terminate, 40% suspicious, max 3 nodes
    }

    /// Create a mock that rarely terminates (for testing backtracking)
    pub fn new_backtrack_heavy() -> Self {
        Self::new(0.1, 0.5, 4) // 10% terminate, 50% suspicious, max 4 nodes
    }

    /// Create a mock that terminates often (for testing termination conditions)
    pub fn new_terminate_heavy() -> Self {
        Self::new(0.7, 0.6, 2) // 70% terminate, 60% suspicious, max 2 nodes
    }

    /// Create a mock that marks blocks as suspicious often
    pub fn new_suspicious_heavy() -> Self {
        Self::new(0.2, 0.8, 3) // 20% terminate, 80% suspicious, max 3 nodes
    }

    /// Create a mock that rarely marks blocks as suspicious
    pub fn new_clean_blocks() -> Self {
        Self::new(0.3, 0.1, 3) // 30% terminate, 10% suspicious, max 3 nodes
    }
}

// impl LLMApp for MockBlockCheckerAgent {
//     fn get_prompt(&self) -> Box<dyn FormatPrompter> {
//         // Mock implementation - return empty prompt
//         let prompt = message_formatter![fmt_template!(HumanMessagePromptTemplate::new(
//             template_fstring!("mock prompt", "dummy")
//         ))];
//         Box::new(prompt)
//     }
//
//     fn get_llm(&self) -> Box<dyn LLM> {
//         // Mock implementation - this should not be called in mock mode
//         panic!("MockBlockCheckerAgent should not call get_llm()")
//     }
//
//     fn token_usage(&self) -> TokenUsage {
//         let token_usage = self.token_usage.lock().unwrap();
//         token_usage.clone()
//     }
//
//     fn add_token_usage(&self, usage: Option<TokenUsage>) {
//         let mut token_usage = self.token_usage.lock().unwrap();
//         if let Some(usage) = usage {
//             token_usage.completion_tokens += usage.completion_tokens;
//             token_usage.prompt_tokens += usage.prompt_tokens;
//             token_usage.total_tokens += usage.total_tokens;
//         }
//     }
// }

#[allow(unused)]
#[async_trait]
impl<'a, B> BlockChecker<'a, B> for MockBlockCheckerAgent
where
    B: Block<'a> + Sync + Send,
{
    async fn determine(
        &mut self,
        block: &B,
        port_nodes: &[(NodeID, TimeAnnotation)],
        input_nodes: &[(NodeID, TimeAnnotation)],
        sig: NodeID,
        sig_time: TimeAnnotation,
        appendix_info: &str,
        module_knowledge: &str,
        historical_suspicious_blocks: &Vec<B>,
    ) -> anyhow::Result<(Option<Vec<(NodeID, TimeAnnotation)>>, bool, bool)> {
        let mut rng = rand::rng();

        // Random decision: terminate or not
        let should_terminate = rng.random::<f64>() < self.terminate_probability;

        // Random decision: suspicious or not
        let is_suspicious = rng.random::<f64>() < self.suspicious_probability;

        if should_terminate {
            info!(
                "Mock: Block {} - Terminating (suspicious: {})",
                block.get_module_name(),
                is_suspicious
            );

            Ok((None, is_suspicious, true))
        } else {
            // Don't terminate, need to select nodes for backtracing
            if input_nodes.is_empty() {
                info!(
                    "Mock: Block {} - No input nodes available, terminating (suspicious: {})",
                    block.get_module_name(),
                    is_suspicious
                );
                return Ok((None, is_suspicious, true));
            }

            // Randomly select some input nodes for backtracing
            let num_nodes = if input_nodes.len() <= self.max_nodes_to_select {
                rng.random_range(1..=input_nodes.len())
            } else {
                rng.random_range(1..=self.max_nodes_to_select)
            };

            let mut selected_nodes = Vec::new();
            let mut available_indices: Vec<usize> = (0..input_nodes.len()).collect();

            for _ in 0..num_nodes {
                if available_indices.is_empty() {
                    break;
                }
                let idx = rng.random_range(0..available_indices.len());
                let node_idx = available_indices.remove(idx);
                selected_nodes.push(input_nodes[node_idx].clone());
            }

            info!(
                "Mock: Block {} - Backtracing {} nodes: {:?} (suspicious: {})",
                block.get_module_name(),
                selected_nodes.len(),
                selected_nodes
                    .iter()
                    .map(|(n, t)| (n.get_text(), *t))
                    .collect::<Vec<_>>(),
                is_suspicious
            );

            Ok((Some(selected_nodes), is_suspicious, false))
        }
    }
}
