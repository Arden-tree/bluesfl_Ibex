use crate::agent::block::block_reranker::BlockRerankerAgent;
use crate::llm::LLMAidTracer;
use crate::{
    get_block_manager, save_trace_to_json, Block, BlockCheckerAgent, BlockManager, BugIDType,
    CompositeCoverageTracker, CoverageTracker, DataFlowBlock, DataFlowBlockParser,
    DataFlowBlockParserBuilder, LocalizationResult, Localizer, ModuleCheckerAgent, NodeID,
    ParameterCoverageReport, TimeAnnotation, Tracer,
};
use clap::ValueEnum;
use regex::Regex;
use rig::agent::AgentBuilder;
use rig::client::builder::{ClientFactory, DefaultProviders, DynClientBuilder};
use rig::client::completion::CompletionModelHandle;
use rig::completion::Usage;
use rig::prelude::ProviderClient;
use rig::providers::{anthropic, ollama, openai};
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::{env, fs};

#[derive(Debug, Clone, PartialEq, ValueEnum)]
pub enum AgentType {
    OpenAI,
    Claude,
    Ollama,
}

pub fn get_agent_builder<'a>(
    agent_type: AgentType,
    model_name: &str,
) -> AgentBuilder<CompletionModelHandle<'a>> {
    let multi_client = DynClientBuilder::default().register_all(vec![
        ClientFactory::new(
            DefaultProviders::OLLAMA,
            || {
                let ollama_host = env::var("OLLAMA_HOST").unwrap_or("http://localhost".to_string());
                let ollama_port = env::var("OLLAMA_PORT").unwrap_or("11434".to_string());
                let base_url = format!("{}:{}", ollama_host, ollama_port);
                Box::new(
                    ollama::Client::builder()
                        .base_url(&base_url)
                        .build()
                        .unwrap(),
                )
            },
            ollama::Client::from_val_boxed,
        ),
        ClientFactory::new(
            DefaultProviders::ANTHROPIC,
            || {
                let anthropic_base = env::var("ANTHROPIC_BASE_URL")
                    .unwrap_or("https://api.anthropic.com".to_string());
                let anthropic_key =
                    env::var("ANTHROPIC_API_KEY").expect("Please set ANTHROPIC_API_KEY");
                Box::new(
                    anthropic::Client::builder(&anthropic_key)
                        .base_url(&anthropic_base)
                        .build()
                        .unwrap(),
                )
            },
            anthropic::Client::from_val_boxed,
        ),
        ClientFactory::new(
            DefaultProviders::OPENAI,
            || {
                let openai_base =
                    env::var("API_BASE").unwrap_or("https://api.openai.com".to_string());
                let openai_key = env::var("API_KEY").expect("Please set OPENAI_API_KEY");
                Box::new(
                    openai::Client::builder(&openai_key)
                        .base_url(&openai_base)
                        .build()
                        .unwrap(),
                )
            },
            openai::Client::from_val_boxed,
        ),
    ]);
    let builder = match agent_type {
        AgentType::OpenAI => multi_client.agent("openai", model_name).unwrap(),
        AgentType::Claude => multi_client.agent("anthropic", model_name).unwrap(),
        AgentType::Ollama => multi_client.agent("ollama", model_name).unwrap(),
    };
    builder
}

fn parse_module<'a, P: AsRef<Path>>(
    mod_path: &[P],
    includes: &[P],
    top_module: &str,
    top_scope: &str,
    param_coverage_tracker: Option<ParameterCoverageReport>,
) -> BlockManager<'a, DataFlowBlockParser> {
    let defines = vec![("RVFI".to_string(), None)]
        .into_iter()
        .collect::<HashMap<_, _>>();
    let includes = includes
        .iter()
        .map(|p| p.as_ref().to_path_buf())
        .collect::<Vec<_>>();

    let mut parser_builder = DataFlowBlockParserBuilder::default();
    if let Some(param_coverage_report) = param_coverage_tracker {
        parser_builder.param_coverage_tracker(param_coverage_report);
    }
    let parser = parser_builder.build().unwrap();
    let mod_files = mod_path;
    let block_manager = get_block_manager(
        mod_files, &defines, &includes, top_module, top_scope, parser,
    );
    block_manager
}

pub fn read_coverage_files<P: AsRef<Path>>(coverage_path: P) -> Vec<(TimeAnnotation, PathBuf)> {
    let coverage_path = coverage_path.as_ref();
    let mut files_with_time_annotation = Vec::new();

    let re = Regex::new(r"coverage_[a-z|_]*(\d+)[a-z|_]*.dat").unwrap();

    if let Ok(entries) = fs::read_dir(coverage_path) {
        for entry in entries.filter_map(Result::ok) {
            let file_name = entry.file_name();
            let file_path = entry.path();
            if let Some(time_annotation) = re
                .captures(&file_name.to_string_lossy())
                .and_then(|captures| captures.get(1))
                .and_then(|time_str| time_str.as_str().parse::<TimeAnnotation>().ok())
            {
                files_with_time_annotation.push((time_annotation, file_path));
            }
        }
    }

    files_with_time_annotation
}

pub fn save_trace<'a, B: Block<'a>>(
    trace_blocks: &[(B, Option<TimeAnnotation>)],
    output_path: &str,
) -> Result<(), Box<dyn Error>> {
    let data = save_trace_to_json(&trace_blocks);
    let mut file = File::create(format!("{}/trace.json", output_path))?;
    let json_string = serde_json::to_string_pretty(&data)?;
    file.write_all(json_string.as_bytes())?;
    Ok(())
}

fn get_total_blocks(block_manager: &BlockManager<DataFlowBlockParser>) -> Vec<DataFlowBlock> {
    block_manager
        .get_scopes()
        .into_iter()
        .filter_map(|scope| block_manager.get_scope_blocks(scope))
        .map(|(blocks, _)| blocks.clone())
        .flatten()
        .collect::<Vec<_>>()
}

pub fn setup_block_mgr<'a, P: AsRef<Path>, CT: CoverageTracker>(
    compose_tracker: &CT,
    param_tracker: Option<ParameterCoverageReport>,
    includes: &[P],
    mod_path: &[P],
    top_module: &str,
    top_scope: &str,
) -> BlockManager<'a, DataFlowBlockParser> {
    // FIXME: compose_tracker here looks unnecessary.
    let covered_files = compose_tracker.get_covered_module_files();
    let mod_path = mod_path
        .iter()
        .filter(|p| {
            let file_name = p
                .as_ref()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            covered_files.contains(&file_name)
        })
        .collect::<Vec<_>>();
    let includes = includes.iter().map(|p| p).collect::<Vec<_>>();
    let block_manager = parse_module(&mod_path, &includes, top_module, top_scope, param_tracker);
    block_manager
}

pub fn get_all_llm<'a>(
    agent_type: AgentType,
    model: &str,
    wave_path: &str,
    total_token_usage: Arc<Mutex<Usage>>,
) -> (
    ModuleCheckerAgent<CompletionModelHandle<'a>>,
    BlockCheckerAgent<CompletionModelHandle<'a>>,
    BlockRerankerAgent<CompletionModelHandle<'a>>,
) {
    let sv_analysis_home =
        env::var("SV_ANALYSIS_HOME").expect("Please set SV_ANALYSIS_HOME to project root path.");

    let mod_checker = {
        let mod_checker_prompt_path = &format!("{sv_analysis_home}/prompts/mod_checker.md");
        let mod_checker_system_prompt_path =
            &format!("{sv_analysis_home}/prompts/mod_checker_system.md");
        let mod_checker_system_prompt = fs::read_to_string(&mod_checker_system_prompt_path)
            .expect("Error when reading mod_checker system prompt");
        let llm_builder = get_agent_builder(agent_type.clone(), model);
        let llm = llm_builder
            .preamble(&mod_checker_system_prompt)
            .temperature(0.)
            .build();
        ModuleCheckerAgent::new(
            mod_checker_prompt_path,
            llm,
            wave_path,
            total_token_usage.clone(),
        )
    };
    let block_checker = {
        let block_checker_prompt_path = &format!("{sv_analysis_home}/prompts/block_checker_2.md");
        let block_checker_system_prompt_path =
            &format!("{sv_analysis_home}/prompts/block_checker_system.md");
        let block_checker_system_prompt = fs::read_to_string(&block_checker_system_prompt_path)
            .expect("Error when reading block_checker system prompt");
        let llm_builder = get_agent_builder(agent_type.clone(), model);
        let llm = llm_builder
            .preamble(&block_checker_system_prompt)
            .temperature(0.)
            .build();
        BlockCheckerAgent::new(
            block_checker_prompt_path,
            llm,
            wave_path,
            total_token_usage.clone(),
        )
    };
    let block_reranker = {
        let block_reranker_prompt_path = &format!("{sv_analysis_home}/prompts/block_reranker.md");
        let block_reranker_system_prompt_path =
            &format!("{sv_analysis_home}/prompts/block_reranker_system.md");
        let block_reranker_system_prompt = fs::read_to_string(&block_reranker_system_prompt_path)
            .expect("Error when reading block_reranker system prompt");
        let llm_builder = get_agent_builder(agent_type.clone(), model);
        let llm = llm_builder
            .preamble(&block_reranker_system_prompt)
            .temperature(0.)
            .build();
        BlockRerankerAgent::new(
            block_reranker_prompt_path,
            llm,
            wave_path,
            total_token_usage.clone(),
        )
    };
    (mod_checker, block_checker, block_reranker)
}

pub async fn run_llm_tracer<P: AsRef<Path>, I: ToString>(
    bug_id: BugIDType,
    agent_type: AgentType,
    model: &str,
    vote_top_k: usize,
    vote_total: usize,
    mod_path: &[P],
    includes: &[P],
    wave_path: &str,
    wave_files: &[(TimeAnnotation, PathBuf)],
    rm_params_path: &str,
    top_module: &str,
    top_scope: &str,
    start_scope: &str,
    start_sig: &str,
    time: TimeAnnotation,
    test_info: I,
    time_bound: TimeAnnotation,
    time_step: TimeAnnotation,
    trace_limit: Option<usize>,
    enable_early_stop: bool,
    prefix: Option<Vec<String>>,
) -> Result<
    (
        Vec<DataFlowBlock>,
        Vec<(DataFlowBlock, Option<TimeAnnotation>)>,
        Vec<(DataFlowBlock, Option<(NodeID, Option<TimeAnnotation>)>)>,
        Vec<(String, Option<(NodeID, Option<TimeAnnotation>)>)>,
        LocalizationResult,
    ),
    Box<dyn Error>,
> {
    let coverage_tracker = CompositeCoverageTracker::new(wave_files, rm_params_path.into());
    let param_tracker = ParameterCoverageReport::new(rm_params_path);
    // let covered_files = coverage_tracker.get_covered_module_files();
    let block_manager = setup_block_mgr(
        &coverage_tracker,
        Some(param_tracker),
        includes,
        mod_path,
        top_module,
        top_scope,
    );
    block_manager.dump_blocks_distribution("./")?;
    let total_token_usage = Arc::new(Mutex::new(Usage::default()));
    let (mod_checker, block_checker, block_reranker) =
        get_all_llm(agent_type, model, wave_path, total_token_usage.clone());

    // use crate::agent::block::block_checker::MockBlockCheckerAgent;
    // use crate::agent::block::mod_checker::MockModuleCheckerAgent;
    // let mod_checker = MockModuleCheckerAgent::new(
    //     0.5,
    //     2,
    // );
    // let block_checker = MockBlockCheckerAgent::new(0.1, 0.5, 4);

    // let vote_top_k = 2; // control expand how many nodes
    // let vote_total = 1; // control voting times
    let mut llm_tracer = LLMAidTracer::new(
        bug_id,
        model,
        test_info,
        time_step,
        Some(time_bound),
        trace_limit,
        &block_manager,
        coverage_tracker,
        mod_checker,
        block_checker,
        block_reranker,
        enable_early_stop,
        vote_top_k,
        vote_total,
        total_token_usage.clone(),
    );
    let total_blocks = get_total_blocks(&block_manager);
    let trace_blocks = llm_tracer
        .trace(
            start_scope,
            start_sig,
            Some(time),
            Some(time_bound),
            trace_limit,
            prefix,
        )
        .await;

    let suspicious_modules = llm_tracer.get_localized_modules();
    let suspicious_blocks = llm_tracer.get_localized_blocks();
    let localized_blocks = llm_tracer.get_localization_results();

    Ok((
        total_blocks,
        trace_blocks,
        suspicious_blocks,
        suspicious_modules,
        localized_blocks,
    ))
}
