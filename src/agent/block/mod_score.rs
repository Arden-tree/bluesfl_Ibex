use crate::agent::block::base::BlockAgentBase;
use crate::agent::utils::parse_json_md;
use crate::prompt_args;
use async_trait::async_trait;
use log::{debug, error, info};
use rig::agent::Agent;
use rig::completion::{CompletionModel, Usage};
use std::sync::{Arc, Mutex};

#[async_trait]
pub trait ModuleScore: Clone {
    /// Analyze the given module code and determine if it contains a buggy line.
    /// Returns:
    /// - Ok(Some((line_number, score))) if a bug is detected
    /// - Ok(None) if no bugs are detected
    async fn evaluate(&mut self, module_code: String) -> anyhow::Result<Vec<(u64, f64)>>;
}

#[derive(Clone)]
pub struct ModuleScoreAgent<M: CompletionModel> {
    top_k: usize,
    base: BlockAgentBase<M>,
}

impl<M: CompletionModel> ModuleScoreAgent<M> {
    pub fn new(
        prompt_path: &str,
        llm: Agent<M>,
        waveform_path: &str,
        top_k: usize,
        token_usage: Arc<Mutex<Usage>>,
    ) -> Self {
        ModuleScoreAgent {
            top_k,
            base: BlockAgentBase::new(prompt_path, llm, waveform_path, token_usage),
        }
    }
}

#[async_trait]
impl<M> ModuleScore for ModuleScoreAgent<M>
where
    M: CompletionModel,
{
    async fn evaluate(&mut self, module_code: String) -> anyhow::Result<Vec<(u64, f64)>> {
        // Prepare prompt arguments for the LLM
        let args = prompt_args![
            "top_k" => self.top_k,
            "module_code" => module_code.clone(),
        ];

        // Invoke the LLM
        let data = self.base.invoke(&args).await?;
        info!("Module Score LLM response: {}", data);

        // Parse the LLM's JSON markdown response
        let json_data = parse_json_md(&data)?;

        // Expected structure:
        // {
        //   "buggy_line": "<the buggy line as string>" | null,
        //   "score": <float> | null
        // }

        let buggy_lines = json_data.as_array().map(|lines| {
            lines
                .iter()
                .filter_map(|line| line.as_object())
                .filter_map(|line| {
                    let buggy_line_str = line
                        .get("buggy_line")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string());

                    let score = line.get("score").and_then(|v| v.as_f64()).map(|v| v);

                    debug!(
                        "[module score] parse result: buggy_line: {:?}, score: {:?}",
                        buggy_line_str, score
                    );

                    // If both exist, try to locate the line number from the code text
                    let result = match (buggy_line_str, score) {
                        (Some(line_content), Some(score_value)) => {
                            let lineno = module_code.lines().enumerate().find_map(|(i, line)| {
                                if line.trim() == line_content.trim() {
                                    Some((i + 1) as u64)
                                } else {
                                    None
                                }
                            });

                            lineno.map(|n| (n, score_value))
                        }
                        _ => {
                            error!(
                                "[module score] error when parsing from array element: {:?}",
                                line
                            );
                            None
                        }
                    };

                    result
                })
                .collect::<Vec<_>>()
        });

        if buggy_lines.is_none() {
            anyhow::bail!("error when parsing json data: {:?}", json_data);
        } else {
            Ok(buggy_lines.unwrap())
        }
    }
}
