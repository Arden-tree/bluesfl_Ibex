use clap::Parser;
use futures::executor::block_on;
use log::{error, info, trace, warn};
use rig::client::completion::CompletionModelHandle;
use rig::completion::Usage;
use std::error::Error;
use std::path::{absolute, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::{env, fs};
use sv_analysis::{
    get_agent_builder, get_module_files, init_logger, read_coverage_files, save_data_to_json,
    setup_block_mgr, token_price, AgentType, Block, BlockManager, BugIDType,
    CompositeCoverageTracker, DataFlowBlock, DataFlowBlockParser, LocalizationChoice,
    LocalizationResult, ModuleDecision, ModuleDecisionAgent, ModuleScore, ModuleScoreAgent,
    ParameterCoverageReport,
};

#[derive(Parser, Debug)]
#[command(name = "naive_fl")]
#[command(about = "Localize source level bug for modern processors (Naive Implementation).", long_about = None)]
struct Args {
    #[arg(short, long)]
    dot_env: Option<String>,
    #[arg(short, long, value_enum, default_value_t = AgentType::OpenAI)]
    agent_type: AgentType,
    #[arg(short, long, default_value = "gpt-4o-mini")]
    model: String,
    #[arg(short, long)]
    project_path: String,
    #[arg(short, long, value_delimiter = ',')]
    include_paths: Vec<String>,
    #[arg(long)]
    rm_params_path: String,
    #[arg(short, long)]
    wave_path: String,
    #[arg(short, long)]
    coverage_path: String,
    #[arg(long)]
    bug_id: BugIDType,
    #[arg(long)]
    top_module: String,
    #[arg(long)]
    top_scope: String,
    #[arg(long)]
    test_info: String,
    #[arg(long, default_value = "5")]
    top_k: usize,
    #[arg(long, default_value = "./")]
    output_path: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    init_logger("naive_fl");

    /*
    --
        --bug-id=init
        --agent-type=open-ai
        --model=gpt-4o
        --project-path=/home/lzz/exp_wkdir/ibex_test/ibex/rtl
        --include-paths=/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/ip/prim/rtl/,/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/dv/sv/dv_utils
        --rm-params-path=/home/lzz/exp_wkdir/ibex_test/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/rm_params.tree.json
        --coverage-path=/home/lzz/exp_wkdir/ibex_test/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator
        --wave-path=/home/lzz/exp_wkdir/ibex_test/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/sim.fst
        --top-module=ibex_core --top-scope=TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core
        --test-info "Now this core design is buggy when executing a jmp instruction. j pc + 0xa0c0 this instruction jump to a wrong address 0x000f5fc0 which should jump to address 0x0010a140"
        --dot-env=./.env
     */

    let args = Args::parse();

    println!("args: {:#?}", args);

    if args.dot_env.is_none() {
        dotenv::dotenv()?;
    } else {
        dotenv::from_filename(args.dot_env.as_ref().unwrap())?;
    }

    if let Ok(flag) = fs::exists(&args.output_path) {
        if !flag {
            info!("output_path {} not exists, create", &args.output_path);
            fs::create_dir(&args.output_path)?;
        } else {
            info!("output_path {} exists", &args.output_path);
        }
    } else {
        return Err(format!("output_path {} cannot be accessed", args.output_path).into());
    }

    println!("Setting up coverage tracker and block manager...");
    let mod_path = get_module_files(args.project_path.clone());
    let wave_files = read_coverage_files(&args.coverage_path);
    let includes = args
        .include_paths
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let top_module = &args.top_module;
    let top_scope = &args.top_scope;
    let coverage_tracker =
        CompositeCoverageTracker::new(&wave_files, args.rm_params_path.clone().into());
    let param_tracker = ParameterCoverageReport::new(args.rm_params_path);
    let block_manager = setup_block_mgr(
        &coverage_tracker,
        Some(param_tracker),
        &includes,
        &mod_path,
        top_module,
        top_scope,
    );

    // llm1:
    // > input: test report + module names
    // > output: which file is buggy
    //

    let token_usage_clone = Arc::new(Mutex::new(Usage::default()));

    let mut mod_decision_agent = get_module_decision_agent(
        args.agent_type.clone(),
        &args.model,
        &args.wave_path,
        args.top_k,
        token_usage_clone.clone(),
    );

    let mut mod_score_agent = get_module_score_agent(
        args.agent_type.clone(),
        &args.model,
        &args.wave_path,
        args.top_k,
        token_usage_clone.clone(),
    );

    let suspicious_mod_files = get_suspicious_files(
        &mut mod_decision_agent,
        args.project_path.clone(),
        &args.test_info,
    )
    .await
    .unwrap_or(vec![]);

    if suspicious_mod_files.is_empty() {
        warn!("No suspicious module files found");
    }

    let loc_choices: Vec<_> = suspicious_mod_files
        .iter()
        .filter_map(|mod_path| {
            let scores = block_on(score_for_module_code(&mut mod_score_agent, mod_path));
            let scores = if scores.is_err() {
                error!("[score agent]: {}", scores.err().unwrap());
                None
            } else {
                Some(scores.unwrap())
            };
            if scores.is_none() {
                warn!(
                    "[score agent]: module: {}, score is None, consider no bug",
                    mod_path.display()
                );
            }
            scores.map(|scores| (scores, mod_path.file_stem().unwrap().to_str().unwrap()))
        })
        .map(|(scores, mod_name)| {
            scores
                .into_iter()
                .map(|(lineno, score)| {
                    trace!("mod_name: {}, [line {}, score {}]", mod_name, lineno, score);
                    let blocks = get_belong_blocks(&block_manager, mod_name, lineno as u32);
                    if blocks.is_empty() {
                        warn!(
                            "[belong block]: Cannot found lineno={}'s block at mod={}",
                            lineno, mod_name
                        );
                    }
                    blocks.into_iter().map(move |block| LocalizationChoice {
                        module_name: Some(mod_name.to_string()),
                        line_number: Some(lineno as usize),
                        block_id: Some(block.get_bid()),
                        score: Some(score),
                    })
                })
                .flatten()
                .collect::<Vec<_>>()
        })
        .flatten()
        .collect();

    // llm2:
    // > input: test report + module
    // > output: whether contians bugs, if yes give line
    // > postprocess: line -> block
    //
    // llm3:
    // > reranker
    // > post process: localized blocks

    let token_usage = token_usage_clone.lock().unwrap().clone();
    let loc_results = LocalizationResult {
        bug_id: args.bug_id.clone(),
        token_usage: Some(token_usage),
        token_price: token_price(&token_usage, &args.model),
        choices: loc_choices,
    };

    save_data_to_json(
        &loc_results,
        format!(
            "{}/naive_fl_loc_results_{}.json",
            &args.output_path, args.bug_id
        ),
    )?;
    Ok(())
}

fn get_module_decision_agent<'a>(
    agent_type: AgentType,
    model: &str,
    waveform_path: &str,
    top_k: usize,
    token_usage: Arc<Mutex<Usage>>,
) -> ModuleDecisionAgent<CompletionModelHandle<'a>> {
    let sv_analysis_home =
        env::var("SV_ANALYSIS_HOME").expect("Please set SV_ANALYSIS_HOME to project root path.");

    let mod_decision_prompt_path = &format!("{sv_analysis_home}/prompts/naive/mod_decision.md");
    let mod_decision_system_prompt_path =
        &format!("{sv_analysis_home}/prompts/naive/mod_decision_system.md");
    let mod_decision_system_prompt = fs::read_to_string(&mod_decision_system_prompt_path)
        .expect("Error when reading mod_checker system prompt");
    let llm_builder = get_agent_builder(agent_type.clone(), model);
    let llm = llm_builder
        .preamble(&mod_decision_system_prompt)
        .temperature(0.)
        .build();

    let mod_decision_agent = ModuleDecisionAgent::new(
        mod_decision_prompt_path,
        llm,
        waveform_path,
        top_k,
        token_usage,
    );

    mod_decision_agent
}

fn get_module_score_agent<'a>(
    agent_type: AgentType,
    model: &str,
    waveform_path: &str,
    top_k: usize,
    token_usage: Arc<Mutex<Usage>>,
) -> ModuleScoreAgent<CompletionModelHandle<'a>> {
    let sv_analysis_home =
        env::var("SV_ANALYSIS_HOME").expect("Please set SV_ANALYSIS_HOME to project root path.");

    let mod_score_prompt_path = &format!("{sv_analysis_home}/prompts/naive/mod_score.md");
    let mod_score_system_prompt_path =
        &format!("{sv_analysis_home}/prompts/naive/mod_score_system.md");
    let mod_score_system_prompt = fs::read_to_string(&mod_score_system_prompt_path)
        .expect("Error when reading mod_checker system prompt");
    let llm_builder = get_agent_builder(agent_type.clone(), model);
    let llm = llm_builder
        .preamble(&mod_score_system_prompt)
        .temperature(0.)
        .build();

    let mod_score_agent = ModuleScoreAgent::new(
        mod_score_prompt_path,
        llm,
        waveform_path,
        top_k,
        token_usage,
    );

    mod_score_agent
}

/// Recursively prints the directory structure
fn build_dir_structure<P: AsRef<Path>>(path: P, indent: &str) -> String {
    let mut result = String::new();

    if path.as_ref().is_dir() {
        result.push_str(&format!(
            "{}{}\n",
            indent,
            path.as_ref().file_name().unwrap().to_string_lossy()
        ));
        let entries = fs::read_dir(path).unwrap();
        for entry in entries {
            let entry = entry.unwrap();
            let path = entry.path();
            result.push_str(&build_dir_structure(&path, &format!("{}    ", indent)));
        }
    } else {
        result.push_str(&format!(
            "{}{}\n",
            indent,
            path.as_ref().file_name().unwrap().to_string_lossy()
        ));
    }

    result
}

async fn get_suspicious_files<'a, P: AsRef<Path>>(
    agent: &mut ModuleDecisionAgent<CompletionModelHandle<'a>>,
    source_root: P,
    tests_report: &str,
) -> anyhow::Result<Vec<PathBuf>> {
    let abs_path = absolute(source_root.as_ref())?;
    let project_structure = build_dir_structure(abs_path.clone(), "");
    let ret = agent
        .determine(tests_report.to_string(), project_structure)
        .await;

    let ret = ret.map(|paths| {
        paths
            .into_iter()
            .map(|path| {
                if let Some(parent_path) = abs_path.parent() {
                    parent_path.join(path)
                } else {
                    abs_path.join(path)
                }
            })
            .collect()
    });

    ret
}

async fn score_for_module_code<P: AsRef<Path>>(
    agent: &mut ModuleScoreAgent<CompletionModelHandle<'_>>,
    module_file: P,
) -> anyhow::Result<Vec<(u64, f64)>> {
    let module_code = fs::read_to_string(module_file).unwrap();
    let ret = agent.evaluate(module_code).await;
    ret
}

pub fn get_belong_blocks<'a>(
    block_manager: &BlockManager<'a, DataFlowBlockParser>,
    module_name: &str,
    line_number: u32,
) -> Vec<DataFlowBlock> {
    let scopes = block_manager.get_scopes();

    let blocks: Vec<_> = scopes
        .into_iter()
        .filter_map(|scope| {
            // Get the block that contains this line number
            match block_manager.get_belong_block_from_original_lineno(&scope, line_number) {
                Some(block) => {
                    // Check if this block belongs to the specified module
                    if block.get_module_name() == module_name {
                        Some(block)
                    } else {
                        None
                    }
                }
                None => None,
            }
        })
        .collect();
    blocks
}
