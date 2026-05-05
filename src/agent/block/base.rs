use crate::agent::PromptArgs;
use crate::wave::mgr::WaveformManager;
use log::debug;
use rig::agent::Agent;
use rig::completion::{CompletionModel, Prompt, Usage};
use std::fs;
use std::sync::{Arc, Mutex};

pub struct BlockAgentBase<M: CompletionModel> {
    pub prompt: String,
    pub llm: Agent<M>,
    pub waveform_mgr: WaveformManager,
    token_usage: Arc<Mutex<Usage>>,
    // pub wave_inspector: WaveInspector,
    // pub current_scope: RwLock<WaveContext<'a>>,
}

impl<M: CompletionModel> Clone for BlockAgentBase<M> {
    fn clone(&self) -> Self {
        Self {
            prompt: self.prompt.clone(),
            llm: self.llm.clone(),
            waveform_mgr: self.waveform_mgr.clone(),
            token_usage: self.token_usage.clone(),
        }
    }
}

impl<M: CompletionModel> BlockAgentBase<M> {
    pub fn new(
        prompt_path: &str,
        llm: Agent<M>,
        waveform_path: &str,
        token_usage: Arc<Mutex<Usage>>,
        // start_scope: &'a [&'a str],
        // start_time: usize,
    ) -> Self {
        BlockAgentBase {
            prompt: fs::read_to_string(prompt_path).unwrap(),
            llm,
            waveform_mgr: WaveformManager::new(waveform_path),
            token_usage,
            // wave_inspector: WaveInspector::new(path).expect("Failed to initialize Wave Inspector"),
            // current_scope: RwLock::new(WaveContext::new(start_scope, start_time)),
        }
    }

    fn get_prompt(&self, args: &PromptArgs) -> anyhow::Result<String> {
        let mut res = self.prompt.to_string();

        for (key, value) in args {
            let placeholder = format!("{{{}}}", key);
            res = res.replace(&placeholder, value);
        }

        Ok(res)
    }

    pub async fn invoke(&mut self, args: &PromptArgs) -> anyhow::Result<String> {
        let prompt = self.get_prompt(args)?;
        debug!("LLM input prompt: {}", prompt);
        let response = self.llm.prompt(&prompt).extended_details().await?;
        if let Ok(mut token_usage) = self.token_usage.lock() {
            *token_usage += response.total_usage;
        }
        Ok(response.output.to_string())
    }
}
