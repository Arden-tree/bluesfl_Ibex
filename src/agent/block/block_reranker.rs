use crate::agent::block::base::BlockAgentBase;
use crate::agent::utils::parse_json_md;
use crate::{prompt_args, Block, NodeID, TimeAnnotation};
use async_trait::async_trait;
use log::debug;
use rig::agent::Agent;
use rig::completion::{CompletionModel, Usage};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[async_trait]
pub trait BlockReranker<'a, B>: Clone {
    /// Rerank a list of suspicious blocks
    async fn rerank(
        &mut self,
        blocks: &[((NodeID, Option<TimeAnnotation>), B)],
        test_info: &str,
    ) -> anyhow::Result<Vec<(((NodeID, Option<TimeAnnotation>), B), f64)>>
    where
        B: Block<'a> + Sync + Send;
}

#[derive(Clone)]
pub struct BlockRerankerAgent<M: CompletionModel> {
    base: BlockAgentBase<M>,
}

impl<M: CompletionModel> BlockRerankerAgent<M> {
    pub fn new(
        prompt_path: &str,
        llm: Agent<M>,
        waveform_path: &str,
        token_usage: Arc<Mutex<Usage>>,
    ) -> Self {
        Self {
            base: BlockAgentBase::new(prompt_path, llm, waveform_path, token_usage),
        }
    }
}

#[async_trait]
impl<'a, B, M> BlockReranker<'a, B> for BlockRerankerAgent<M>
where
    M: CompletionModel,
    B: Block<'a> + Sync + Send,
{
    async fn rerank(
        &mut self,
        blocks: &[((NodeID, Option<TimeAnnotation>), B)],
        test_info: &str,
    ) -> anyhow::Result<Vec<(((NodeID, Option<TimeAnnotation>), B), f64)>>
    where
        B: Block<'a> + Sync + Send,
    {
        // TODO: this agent require some tools to help llm access the value of signals.

        let blocks_data = blocks
            .iter()
            .enumerate()
            .map(|(index, ((node_id, time), block))| {
                json!({
                    "index": index,
                    "suspicious_signal": node_id.get_text(),
                    "time": time.map(|t| t.to_string()),
                    "block_info": {
                        "module_name": block.get_module_name(),
                        "code": block.get_ctx().join("\n"),
                    }
                })
            })
            .collect::<Vec<_>>();

        let args = prompt_args![
            "blocks" => serde_json::to_string_pretty(&blocks_data)?,
            "test_info" => test_info,
        ];
        let data = self.base.invoke(&args).await?;
        debug!("Block Reranker response: {}", data);
        let json_data = parse_json_md(&data)?;
        // Expected: [ { "index": usize, "score": f64, "reason": str }, ... ]

        let arr = json_data
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Expected JSON array from model"))?;

        let res = arr
            .iter()
            .map(|entry| {
                let idx = entry
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow::anyhow!("Missing or invalid 'index' field"))?
                    as usize;

                let score = entry
                    .get("score")
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| anyhow::anyhow!("Missing or invalid 'score' field"))?;

                let block = blocks
                    .get(idx)
                    .map(|data| data.clone())
                    .ok_or_else(|| anyhow::anyhow!("Index {} out of bounds", idx))?;

                Ok((block, score))
            })
            .collect::<anyhow::Result<Vec<_>>>();
        res
    }
}
