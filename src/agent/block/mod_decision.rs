use crate::agent::block::base::BlockAgentBase;
use crate::agent::utils::parse_json_md;
use crate::prompt_args;
use async_trait::async_trait;
use log::info;
use rig::agent::Agent;
use rig::completion::{CompletionModel, Usage};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[async_trait]
pub trait ModuleDecision: Clone {
    /// Determine which files may be suspicious
    async fn determine(
        &mut self,
        test_report: String,
        project_structure: String,
    ) -> anyhow::Result<Vec<PathBuf>>;
}

#[derive(Clone)]
pub struct ModuleDecisionAgent<M: CompletionModel> {
    top_k: usize,
    base: BlockAgentBase<M>,
}

impl<M: CompletionModel> ModuleDecisionAgent<M> {
    pub fn new(
        prompt_path: &str,
        llm: Agent<M>,
        waveform_path: &str,
        top_k: usize,
        token_usage: Arc<Mutex<Usage>>,
    ) -> Self {
        ModuleDecisionAgent {
            top_k,
            base: BlockAgentBase::new(prompt_path, llm, waveform_path, token_usage),
        }
    }
}

#[async_trait]
impl<M> ModuleDecision for ModuleDecisionAgent<M>
where
    M: CompletionModel,
{
    async fn determine(
        &mut self,
        test_report: String,
        project_structure: String,
    ) -> anyhow::Result<Vec<PathBuf>> {
        let args = prompt_args![
            "top_k" => self.top_k,
            "test_report" => test_report,
            "project_structure" => project_structure,
        ];
        let data = self.base.invoke(&args).await?;

        info!("Module Decision llm response: {}", data);
        let json_data = parse_json_md(&data)?;
        let suspicious_files = json_data
            .get("suspicious_files")
            .and_then(|vars| vars.as_array())
            .map(|suspicious_files| {
                let ret = suspicious_files
                    .iter()
                    .map(|value| value.as_str().unwrap().to_owned())
                    .map(|path_str| PathBuf::from(path_str))
                    .collect::<Vec<_>>();
                ret
            })
            .unwrap_or(vec![]);
        Ok(suspicious_files)
    }
}
