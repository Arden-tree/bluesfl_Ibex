#![recursion_limit = "256"]

use clap::Parser;
use log::info;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use sv_analysis::{
    get_module_files, init_logger, read_coverage_files, save_data_to_json, setup_block_mgr,
    BugIDType, CompositeCoverageTracker, IntervalCoverageSampler, Localizer,
    ParameterCoverageReport, SBFLocalizer, SpectrumMetric, TimeAnnotation,
};

#[derive(Parser, Debug)]
#[command(name = "sbfl")]
#[command(about = "Spectrum-based Fault Localization", long_about = None)]
struct Args {
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
    top_module: String,
    #[arg(long)]
    top_scope: String,
    #[arg(long)]
    bug_id: BugIDType,
    #[arg(long)]
    interval: TimeAnnotation,
    #[arg(long)]
    time_step: TimeAnnotation,
    #[arg(long)]
    failed_time: TimeAnnotation,
    #[arg(long, value_enum)]
    metric: SpectrumMetric,
    #[arg(long)]
    top_k: usize,
    #[arg(long, default_value = "./")]
    output_path: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    init_logger("sbfl");

    /*
        --
        --project-path=/home/lzz/exp_wkdir/ibex_test/ibex/rtl
        --include-paths=/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/ip/prim/rtl/,/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/dv/sv/dv_utils
        --rm-params-path=/home/lzz/exp_wkdir/ibex_test/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/rm_params.tree.json
        --coverage-path=/home/lzz/exp_wkdir/ibex_test/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator
        --wave-path=/home/lzz/exp_wkdir/ibex_test/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/sim.fst
        --top-module=ibex_core
        --top-scope=TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core
        --bug-id=0
        --interval=2
        --time-step=2
        --failed-time=19
        --metric=tarantula
        --top-k=10
    */

    let args = Args::parse();

    println!("args: {:#?}", args);

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

    let sampler = IntervalCoverageSampler::new(
        args.interval,
        args.time_step,
        args.failed_time,
        coverage_tracker,
        &block_manager,
    );
    let mut sbfl_localizer =
        SBFLocalizer::new(args.bug_id.clone(), &block_manager, sampler, args.metric);
    // println!(
    //     "{:#?}",
    //     sbfl_localizer
    //         .localize()
    //         .iter()
    //         .take(args.top_k)
    //         .collect::<Vec<_>>()
    // );

    let mut loc_results = sbfl_localizer.get_localization_results();
    let top_k_choices = loc_results
        .choices
        .into_iter()
        .take(args.top_k)
        .collect::<Vec<_>>();
    loc_results.choices = top_k_choices;
    save_data_to_json(
        &loc_results,
        format!(
            "{}/sbfl_loc_results_{}.json",
            &args.output_path, args.bug_id
        ),
    )?;
    Ok(())
}
