#![recursion_limit = "256"]
use clap::{arg, Parser};
use log::info;
use serde_json::json;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use sv_analysis::{
    get_module_files, init_logger, read_coverage_files, run_llm_tracer, save_data_to_json,
    save_trace, TimeAnnotation,
};
use sv_analysis::{AgentType, Block, BugIDType};

#[derive(Parser, Debug)]
#[command(name = "sv-analysis")]
#[command(about = "Localize source level bug for modern processors.", long_about = None)]
struct Args {
    #[arg(short, long)]
    dot_env: Option<String>,
    #[arg(short, long, value_enum, default_value_t = AgentType::OpenAI)]
    agent_type: AgentType,
    #[arg(short, long, default_value = "gpt-4o-mini")]
    model: String,
    #[arg(long, default_value_t = 1, help = "Set maximum expand node number")]
    vote_top_k: usize,
    #[arg(
        long,
        default_value_t = 2,
        help = "Set vote total number in each stage"
    )]
    vote_total: usize,
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
    start_scope: String,
    #[arg(long)]
    start_sig: String,
    #[arg(long)]
    start_time: TimeAnnotation,
    #[arg(long)]
    test_info: String,
    #[arg(long)]
    time_bound: TimeAnnotation,
    #[arg(long)]
    time_step: TimeAnnotation,
    #[arg(long, default_value = "./")]
    output_path: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    init_logger("sv-analysis");

    /*
        --
        --bug-id=init
        --agent-type=open-ai
        --model=gpt-4o-mini
        --project-path=/home/lzz/exp_wkdir/ibex_test/ibex/rtl
        --include-paths=/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/ip/prim/rtl/,/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/dv/sv/dv_utils
        --rm-params-path=/home/lzz/exp_wkdir/ibex_test/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/rm_params.tree.json
        --coverage-path=/home/lzz/exp_wkdir/ibex_test/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator
        --wave-path=/home/lzz/exp_wkdir/ibex_test/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/sim.fst
        --top-module=ibex_core
        --top-scope=TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core
        --start-scope=TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core.if_stage_i
        --start-sig=pc_id_o
        --start-time=19
        --test-info "Now this core design is buggy when executing a jmp instruction. j pc + 0xa0c0 this instruction jump to a wrong address 0x000f5fc0 which should jump to address 0x0010a140"
        --time-bound=11
        --time-step=2
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

    let project_path = args.project_path;
    let mod_path = get_module_files(project_path);
    let include_paths = args
        .include_paths
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let wave_path = &args.wave_path;
    let wave_files = read_coverage_files(&args.coverage_path);
    let top_module = &args.top_module;
    let top_scope = &args.top_scope;
    let start_scope = &args.start_scope;
    let start_sig = &args.start_sig;
    let start_time = args.start_time;
    let time_bound = args.time_bound;
    let time_step = args.time_step;
    let trace_limit = None;

    let enable_early_stop = true;
    let test_info = args.test_info.replace("\\n", "\n");
    let (total_blocks, trace_blocks, suspicious_blocks, suspicious_modules, localized_results) =
        run_llm_tracer(
            args.bug_id.clone(),
            args.agent_type,
            &args.model,
            args.vote_top_k,
            args.vote_total,
            &mod_path,
            &include_paths,
            wave_path,
            &wave_files,
            &args.rm_params_path,
            top_module,
            top_scope,
            start_scope,
            start_sig,
            start_time,
            test_info,
            time_bound,
            time_step,
            trace_limit,
            enable_early_stop,
            None,
        )
        .await?;

    println!(
        "trace_blocks len: {}, total_blocks len: {}",
        trace_blocks.len(),
        total_blocks.len()
    );
    save_trace(&trace_blocks, &args.output_path)?;
    let suspicious_blocks = suspicious_blocks
        .iter()
        .map(|(block, tag)| json!({"node": tag, "code": block.get_ctx()}))
        .collect::<Vec<_>>();
    save_data_to_json(
        &suspicious_blocks,
        format!("{}/suspicious_blocks.json", &args.output_path),
    )?;
    save_data_to_json(
        &suspicious_modules,
        format!("{}/suspicious_modules.json", &args.output_path),
    )?;
    save_data_to_json(
        &localized_results,
        format!("{}/llm_loc_results_{}.json", &args.output_path, args.bug_id),
    )?;

    Ok(())
}
