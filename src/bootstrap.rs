use crate::agent::block::block_checker_toolcall::BlockCheckerToolAgent;
use crate::agent::block::block_reranker::BlockRerankerAgent;
use crate::llm::LLMAidTracer;
use crate::{
    get_block_manager, save_trace_to_json, Block, BlockChecker, BlockCheckerAgent, BlockManager,
    BugIDType, CompositeCoverageTracker, CoverageTracker, DataFlowBlock, DataFlowBlockParser,
    DataFlowBlockParserBuilder, LocalizationResult, Localizer, ModuleCheckerAgent, NodeID,
    ParameterCoverageReport, TimeAnnotation, Tracer,
};
use async_trait::async_trait;
use clap::ValueEnum;
use regex::Regex;
use rig::agent::AgentBuilder;
use rig::client::completion::{CompletionClient, CompletionModelHandle};
use rig::completion::Usage;
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

#[derive(Debug, Clone, PartialEq, ValueEnum)]
pub enum AgentMode {
    Voting,
    ToolCall,
}

/// Enum dispatcher to allow either voting or toolcall BlockChecker at runtime.
/// Required because BlockChecker: Clone, which prevents trait objects.
#[derive(Clone)]
pub enum BlockCheckerEnum {
    Voting(BlockCheckerAgent<CompletionModelHandle<'static>>),
    ToolCall(BlockCheckerToolAgent<CompletionModelHandle<'static>>),
}

#[async_trait]
impl<'a> BlockChecker<'a, DataFlowBlock> for BlockCheckerEnum {
    async fn determine(
        &mut self,
        block: &DataFlowBlock,
        port_nodes: &[(NodeID, TimeAnnotation)],
        nodes: &[(NodeID, TimeAnnotation)],
        sig: NodeID,
        sig_time: TimeAnnotation,
        appendix_info: &str,
        module_knowledge: &str,
        historical_suspicious_blocks: &Vec<DataFlowBlock>,
    ) -> anyhow::Result<(Option<Vec<(NodeID, TimeAnnotation)>>, bool, bool)> {
        match self {
            BlockCheckerEnum::Voting(v) => {
                v.determine(
                    block,
                    port_nodes,
                    nodes,
                    sig,
                    sig_time,
                    appendix_info,
                    module_knowledge,
                    historical_suspicious_blocks,
                )
                .await
            }
            BlockCheckerEnum::ToolCall(t) => {
                t.determine(
                    block,
                    port_nodes,
                    nodes,
                    sig,
                    sig_time,
                    appendix_info,
                    module_knowledge,
                    historical_suspicious_blocks,
                )
                .await
            }
        }
    }
}

pub fn get_agent_builder<'a>(
    agent_type: AgentType,
    model_name: &str,
) -> AgentBuilder<CompletionModelHandle<'a>> {
    let handle = get_model_handle(agent_type, model_name);
    AgentBuilder::new(handle)
}

/// Create the CompletionModelHandle directly (needed for toolcall mode where
/// we must rebuild the Agent with tools on each determine() call).
pub fn get_model_handle<'a>(
    agent_type: AgentType,
    model_name: &str,
) -> CompletionModelHandle<'a> {
    match agent_type {
        AgentType::OpenAI => {
            let openai_base =
                env::var("API_BASE").unwrap_or("https://api.openai.com/v1".to_string());
            let openai_key = env::var("API_KEY").expect("Please set API_KEY");
            let http_client = reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(60))
                .timeout(std::time::Duration::from_secs(600))
                .tcp_keepalive(std::time::Duration::from_secs(60))
                .build()
                .unwrap();
            let client = openai::Client::builder(&openai_key)
                .base_url(&openai_base)
                .custom_client(http_client)
                .build()
                .unwrap();
            let model = openai::completion::CompletionModel::new(client, model_name);
            CompletionModelHandle {
                inner: std::sync::Arc::new(model),
            }
        }
        AgentType::Claude => {
            let anthropic_base = env::var("ANTHROPIC_BASE_URL")
                .unwrap_or("https://api.anthropic.com".to_string());
            let anthropic_key =
                env::var("ANTHROPIC_API_KEY").expect("Please set ANTHROPIC_API_KEY");
            let client = anthropic::Client::builder(&anthropic_key)
                .base_url(&anthropic_base)
                .build()
                .unwrap();
            let model = client.completion_model(model_name);
            CompletionModelHandle {
                inner: std::sync::Arc::new(model),
            }
        }
        AgentType::Ollama => {
            let ollama_host =
                env::var("OLLAMA_HOST").unwrap_or("http://localhost".to_string());
            let ollama_port = env::var("OLLAMA_PORT").unwrap_or("11434".to_string());
            let base_url = format!("{}:{}", ollama_host, ollama_port);
            let client = ollama::Client::builder()
                .base_url(&base_url)
                .build()
                .unwrap();
            let model = client.completion_model(model_name);
            CompletionModelHandle {
                inner: std::sync::Arc::new(model),
            }
        }
    }
}

fn parse_module<'a, P: AsRef<Path>>(
    mod_path: &[P],
    includes: &[P],
    top_module: &str,
    top_scope: &str,
    param_coverage_tracker: Option<ParameterCoverageReport>,
    signal_map: Option<HashMap<String, HashMap<String, String>>>,
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
    if let Some(signal_map) = signal_map {
        parser_builder.signal_map(signal_map);
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
    let mod_path = if covered_files.is_empty() {
        // No coverage data available: assume all files are covered
        mod_path.iter().collect::<Vec<_>>()
    } else {
        mod_path
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
            .collect::<Vec<_>>()
    };
    let includes = includes.iter().map(|p| p).collect::<Vec<_>>();
    // Load signal name map if available
    let signal_map: Option<HashMap<String, HashMap<String, String>>> = {
        let sv_home = env::var("SV_ANALYSIS_HOME").unwrap_or_else(|_| ".".to_string());
        let map_path = format!("{}/signal_name_map.json", sv_home);
        if Path::new(&map_path).exists() {
            let content = fs::read_to_string(&map_path).ok();
            content.and_then(|c| {
                let v: serde_json::Value = serde_json::from_str(&c).ok()?;
                let modules = v.get("modules")?;
                let mut map = HashMap::new();
                for (mod_name, ports) in modules.as_object()? {
                    let mut port_map = HashMap::new();
                    for (port_name, canonical) in ports.as_object()? {
                        port_map.insert(port_name.clone(), canonical.as_str()?.to_string());
                    }
                    map.insert(mod_name.clone(), port_map);
                }
                Some(map)
            })
        } else {
            None
        }
    };
    let block_manager = parse_module(&mod_path, &includes, top_module, top_scope, param_tracker, signal_map);
    block_manager
}

pub fn get_all_llm<'a>(
    agent_type: AgentType,
    agent_mode: AgentMode,
    model: &str,
    wave_path: &str,
    total_token_usage: Arc<Mutex<Usage>>,
) -> (
    ModuleCheckerAgent<CompletionModelHandle<'a>>,
    BlockCheckerEnum,
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

    let block_checker = match agent_mode {
        AgentMode::Voting => {
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
            BlockCheckerEnum::Voting(BlockCheckerAgent::new(
                block_checker_prompt_path,
                llm,
                wave_path,
                total_token_usage.clone(),
            ))
        }
        AgentMode::ToolCall => {
            let system_prompt_path =
                &format!("{sv_analysis_home}/prompts/block_checker_toolcall_system.md");
            let prompt_template_path =
                &format!("{sv_analysis_home}/prompts/block_checker_toolcall.md");
            let model_handle = get_model_handle(agent_type.clone(), model);
            BlockCheckerEnum::ToolCall(BlockCheckerToolAgent::new(
                system_prompt_path,
                prompt_template_path,
                model_handle,
                wave_path,
                total_token_usage.clone(),
            ))
        }
    };

    let block_reranker = {
        let block_reranker_prompt_path = &format!("{sv_analysis_home}/prompts/block_reranker.md");
        let block_reranker_system_prompt_path =
            &format!("{sv_analysis_home}/prompts/block_reranker_system.md");
        let block_reranker_system_prompt = fs::read_to_string(&block_reranker_system_prompt_path)
            .expect("Error when reading block_reranker system prompt");
        let llm_builder = get_agent_builder(agent_type, model);
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
    agent_mode: AgentMode,
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
        get_all_llm(agent_type, agent_mode.clone(), model, wave_path, total_token_usage.clone());

    // In toolcall mode, disable voting (multi-turn tool-call provides robustness)
    let effective_vote_total = match agent_mode {
        AgentMode::ToolCall => 1,
        AgentMode::Voting => vote_total,
    };

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
        effective_vote_total,
        total_token_usage.clone(),
    );
    let total_blocks = get_total_blocks(&block_manager);

    // Paper-aligned two-phase architecture (Section 3):
    // Phase 1: Pure Blues BFS (Algorithm 1) — constructs instruction execution
    //          path via dataflow tracing, WITHOUT any LLM calls.
    // Phase 2: LLM navigation — LLM evaluates blocks in the pre-computed path,
    //          reads signal values, and marks suspicious blocks.
    let trace_blocks = llm_tracer
        .run_phase1_bfs(
            start_scope,
            start_sig,
            Some(time),
            Some(time_bound),
            trace_limit,
        )
        .await;

    // Phase 2: LLM evaluates each Assign/Always block in the path
    llm_tracer.run_phase2_llm(&trace_blocks).await;

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
