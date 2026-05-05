use crate::agent::block::base::BlockAgentBase;
use crate::agent::utils::parse_json_md;
use crate::{prompt_args, Block, BlockType, NodeID, TimeAnnotation};
use anyhow::anyhow;
use async_trait::async_trait;
use log::{info, warn};
use rand::Rng;
use rig::agent::Agent;
use rig::completion::{CompletionModel, Usage};
use std::sync::{Arc, Mutex};

#[async_trait]
pub trait ModuleChecker: Clone {
    /// determine whether dive into
    /// [input]
    /// 1. module header code
    /// 2. sig signal values @ T
    /// 3. input ports signal values @ T-1
    /// 4. instruction info + module knowledge
    /// [output]
    /// if the input values are incorrect, we will not dive into this module instead go to input ports@t-1
    /// otherwise, check this module
    ///
    /// TODO: 1. a NodeID with a time annotation
    /// TODO: 2. how to decide time annotation?
    async fn determine<'a, B>(
        &mut self,
        blocks: &[(B, TimeAnnotation)],
        sig: NodeID,
        sig_time: TimeAnnotation,
        appendix_info: &str,
    ) -> anyhow::Result<Option<Vec<(NodeID, TimeAnnotation)>>>
    where
        B: Block<'a> + Sync + Send;
}

#[derive(Clone)]
pub struct ModuleCheckerAgent<M: CompletionModel> {
    base: BlockAgentBase<M>,
}

impl<M: CompletionModel> ModuleCheckerAgent<M> {
    pub fn new(
        prompt_path: &str,
        llm: Agent<M>,
        waveform_path: &str,
        token_usage: Arc<Mutex<Usage>>,
    ) -> Self {
        ModuleCheckerAgent {
            base: BlockAgentBase::new(prompt_path, llm, waveform_path, token_usage),
        }
    }
}

// impl LLMApp for ModuleCheckerAgent {
//     fn get_prompt(&self) -> Box<dyn FormatPrompter> {
//         let topic_prompt = fs::read_to_string(&self.base.prompt_path).unwrap();
//         let prompt = message_formatter![fmt_template!(HumanMessagePromptTemplate::new(
//             template_fstring!(
//                 topic_prompt,
//                 "module_header",
//                 "sig_name",
//                 "sig_value",
//                 "sig_time",
//                 "wave_time",
//                 "wave",
//                 "scenario"
//             )
//         ))];
//         Box::new(prompt)
//     }
//
//     fn get_llm(&self) -> Box<dyn LLM> {
//         self.base.llm.clone_box()
//     }
//
//     fn token_usage(&self) -> Usage {
//         let token_usage = self.token_usage.lock().unwrap();
//         token_usage.clone()
//     }
//
//     fn add_token_usage(&self, usage: Option<Usage>) {
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
impl<M: CompletionModel> ModuleChecker for ModuleCheckerAgent<M> {
    async fn determine<'a, B>(
        &mut self,
        port_blocks: &[(B, TimeAnnotation)],
        sig: NodeID,
        sig_time: TimeAnnotation,
        appendix_info: &str,
    ) -> anyhow::Result<Option<Vec<(NodeID, TimeAnnotation)>>>
    where
        B: Block<'a> + Sync + Send,
    {
        if port_blocks.len() == 0 {
            // If no input ports exist, we can only dive into current module inside.
            return Ok(None);
        }
        let sig_name = sig.get_text();
        let cur_scope = port_blocks[0].0.get_scope().split(".").collect::<Vec<_>>();
        let sig_value = self.base.waveform_mgr.display_signal_values_at_time_json(
            &cur_scope,
            &vec![sig_name],
            sig_time,
        )?;

        let input_var_nodes_with_t = port_blocks
            .iter()
            .filter(|(b, _)| matches!(b.get_block_type(), BlockType::ModuleInput))
            .map(|(b, t)| {
                b.get_output_nodes()
                    .into_iter()
                    .map(|node| (node, t.clone()))
            })
            .flatten()
            .collect::<Vec<_>>();
        let input_var_names_with_t = input_var_nodes_with_t
            .iter()
            .map(|(n, t)| (n.get_text(), t))
            .collect::<Vec<_>>();
        let last_time = sig_time;
        let waveform = if let Ok(ret) = self
            .base
            .waveform_mgr
            // FIXME: next time should bet set according to this module type
            .display_signal_values_with_batch_json(&cur_scope, &input_var_names_with_t, false)
        {
            ret
        } else {
            warn!("Error getting signal values @ {:?}", last_time);
            "Cannot get waveform".to_string()
        };

        let module_header = port_blocks
            .iter()
            .map(|(b, _)| b.get_ctx())
            .flatten()
            .collect::<Vec<_>>()
            .join("\n");
        let module_name = port_blocks
            .get(0)
            .map(|(b, _)| b.get_module_name())
            .unwrap_or("Unknown Module Name");
        let args = prompt_args![
            "scenario" => appendix_info,
            "module_header" => module_header,
            "sig_value" => sig_value,
            "input_wave" => waveform,
            "module_name" => module_name
        ];
        let data = self.base.invoke(&args).await?;
        // response format:
        // {
        //     "dive": true,
        //     "vars": []
        //     "next_time: 31
        // }
        // {
        //     "dive": false,
        //     "vars": ["s1", "s2"]
        //     "next_time": 31
        // }
        info!("Module Checker llm response: {}", data);
        let json_data = parse_json_md(&data)?;

        let dive = json_data.get("dive").and_then(|dive| dive.as_bool());

        if let Some(dive) = dive {
            if dive {
                return Ok(None);
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
                            input_var_nodes_with_t
                                .iter()
                                // here llm ignores which time he wnat to backtrace.
                                .filter(|(node, time)| {
                                    // llm returned signal name maybe an array, e.g., var_1[0]
                                    let v = v.find("[").map(|pos| &v[..pos]).unwrap_or(v);
                                    node.get_text() == v && *time == t
                                })
                                .collect::<Vec<_>>()
                        })
                        .flatten()
                        .map(|(n, t)| ((**n).clone(), (*t).clone()))
                        .collect::<Vec<_>>();

                    if nodes.is_empty() {
                        warn!("LLM selected some nodes with times, but cannot parse them and return empty list.");
                        return Ok(None);
                    } else {
                        return Ok(Some(nodes));
                    }
                }
            }
        }
        Err(anyhow!(
            "Failed to parse llm response from module checker response {}",
            data
        ))
    }
}

// Mock implementation for testing
#[allow(unused)]
#[derive(Clone)]
pub struct MockModuleCheckerAgent {
    pub dive_probability: f64,      // Probability of diving (0.0 to 1.0)
    pub max_nodes_to_select: usize, // Maximum number of nodes to select when not diving
    token_usage: Arc<Mutex<Usage>>,
}
#[allow(unused)]
impl MockModuleCheckerAgent {
    pub fn new(dive_probability: f64, max_nodes_to_select: usize) -> Self {
        MockModuleCheckerAgent {
            dive_probability: dive_probability.clamp(0.0, 1.0), // Ensure it's between 0 and 1
            max_nodes_to_select,
            token_usage: Arc::new(Mutex::new(Usage::default())),
        }
    }

    /// Create a mock with balanced behavior to ensure algorithm termination
    pub fn new_balanced() -> Self {
        Self::new(0.3, 3) // 30% chance to dive, select up to 3 nodes
    }

    /// Create a mock that prefers not diving (for testing backtracking)
    pub fn new_backtrack_heavy() -> Self {
        Self::new(0.1, 5) // 10% chance to dive, select up to 5 nodes
    }

    /// Create a mock that prefers diving (for testing deep exploration)
    pub fn new_dive_heavy() -> Self {
        Self::new(0.7, 2) // 70% chance to dive, select up to 2 nodes
    }
}

// impl LLMApp for MockModuleCheckerAgent {
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
//         panic!("MockModuleCheckerAgent should not call get_llm()")
//     }
//
//     fn token_usage(&self) -> Usage {
//         let token_usage = self.token_usage.lock().unwrap();
//         token_usage.clone()
//     }
//
//     fn add_token_usage(&self, usage: Option<Usage>) {
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
impl ModuleChecker for MockModuleCheckerAgent {
    async fn determine<'a, B>(
        &mut self,
        port_blocks: &[(B, TimeAnnotation)],
        sig: NodeID,
        sig_time: TimeAnnotation,
        appendix_info: &str,
    ) -> anyhow::Result<Option<Vec<(NodeID, TimeAnnotation)>>>
    where
        B: Block<'a> + Sync + Send,
    {
        if port_blocks.is_empty() {
            return Ok(None);
        }

        let mut rng = rand::rng();

        // Collect input variable nodes
        let input_var_nodes_with_t: Vec<(NodeID, TimeAnnotation)> = port_blocks
            .iter()
            .filter(|(b, _)| matches!(b.get_block_type(), BlockType::ModuleInput))
            .flat_map(|(b, t)| {
                b.get_output_nodes()
                    .into_iter()
                    .map(|node| (node.clone(), t.clone()))
            })
            .collect();

        // If no input nodes available, dive
        if input_var_nodes_with_t.is_empty() {
            info!("Mock: No input nodes available, diving");
            return Ok(None);
        }

        // Random decision: dive or not
        let should_dive = rng.random::<f64>() < self.dive_probability;

        if should_dive {
            info!("Mock: Randomly decided to dive");

            Ok(None)
        } else {
            // Randomly select some input nodes
            let num_nodes = if input_var_nodes_with_t.len() <= self.max_nodes_to_select {
                rng.random_range(1..=input_var_nodes_with_t.len())
            } else {
                rng.random_range(1..=self.max_nodes_to_select)
            };

            let mut selected_nodes = Vec::new();
            let mut available_indices: Vec<usize> = (0..input_var_nodes_with_t.len()).collect();

            for _ in 0..num_nodes {
                if available_indices.is_empty() {
                    break;
                }
                let idx = rng.random_range(0..available_indices.len());
                let node_idx = available_indices.remove(idx);
                selected_nodes.push(input_var_nodes_with_t[node_idx].clone());
            }

            info!(
                "Mock: Randomly decided not to dive, selected {} nodes: {:?}",
                selected_nodes.len(),
                selected_nodes
                    .iter()
                    .map(|(n, t)| (n.get_text(), *t))
                    .collect::<Vec<_>>()
            );

            Ok(Some(selected_nodes))
        }
    }
}
