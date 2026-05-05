#![recursion_limit = "256"]

use clap::Parser;
use serde::{Deserialize, Serialize};
use std::fs::{read_to_string, write};
use std::path::PathBuf;
use sv_analysis::{
    get_module_files, init_logger, read_coverage_files, setup_block_mgr, Block, BlockManager,
    BugIDType, CompositeCoverageTracker, DataFlowBlockParser, LocalizationChoice,
    LocalizationChoiceBuilder, LocalizationResult, LocalizationResultBuilder,
    ParameterCoverageReport,
};

#[derive(Parser, Debug)]
#[command(name = "l2b")]
#[command(
    about = "Convert SystemVerilog line numbers to block IDs for coverage analysis",
    long_about = "A tool that maps source code line numbers to their corresponding block IDs in SystemVerilog modules. \
                  Takes a JSON file containing module names and line numbers, then outputs the corresponding block information \
                  for coverage analysis and debugging purposes. Each input file should contain only one module."
)]
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
    #[arg(
        long,
        help = "Path to input JSON file with format: [{\"bug_id\": number, \"module_name\": string, \"lineno\": number}, {...}, ...]"
    )]
    input_batch: String,
    #[arg(long, default_value = "./results.json")]
    output_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct BlockInfo {
    block_id: u64,
    module_name: String,
    line_number: u32,
    scope: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct BugInfo {
    bug_id: BugIDType,
    module_name: String,
    line_number: i32,
}

#[derive(Debug, Serialize, Deserialize)]
struct ConversionResult {
    bug_id: BugIDType,
    module_name: String,
    line_number: usize,
    blocks: Vec<BlockInfo>,
    conversion_success: bool,
    error_message: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logger("l2b");

    /*
        --
        --project-path=/home/lzz/exp_wkdir/ibex_test/ibex/rtl
        --include-paths=/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/ip/prim/rtl/,/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/dv/sv/dv_utils
        --rm-params-path=/home/lzz/exp_wkdir/ibex_test/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/rm_params.tree.json
        --coverage-path=/home/lzz/exp_wkdir/ibex_test/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator
        --wave-path=/home/lzz/exp_wkdir/ibex_test/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/sim.fst
        --top-module=ibex_core
        --top-scope=TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core
        --input-batch=/home/lzz/dac26/LiK_ibex/result_data/8_20/lik_data/results_chunk_l2b.json
    */

    let args = Args::parse();
    println!("Processing with args: {:#?}", args);

    let input_data = read_to_string(&args.input_batch)
        .map_err(|e| format!("Failed to read input file '{}': {}", args.input_batch, e))?;

    let data: Vec<LocalizationResult> = serde_json::from_str(&input_data)
        .map_err(|e| format!("Failed to parse JSON from '{}': {}", args.input_batch, e))?;

    if data.is_empty() {
        eprintln!("Warning: Input file contains no data");
        return Ok(());
    }

    println!("Loaded {} module-line mappings from input file", data.len());

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

    println!("Setting up coverage tracker and block manager...");
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

    let mut successful_conversions = 0;
    let mut total_conversions = 0;

    let results: Vec<_> = data
        .into_iter()
        .map(|bug_info| (bug_info.bug_id, bug_info.choices))
        .map(|(bug_id, choices)| {
            let mut new_choices: Vec<LocalizationChoice> = choices
                .into_iter()
                .map(|choice| {
                    let bug_id = bug_id.clone();
                    let module_name = &choice.module_name.unwrap();
                    let line_number = choice.line_number.unwrap();
                    println!(
                        "Processing bug_id {}: module '{}' at line {}",
                        bug_id, module_name, line_number
                    );

                    let additional_info =
                        match process_module_line(&block_manager, &module_name, line_number) {
                            Ok(blocks) => {
                                let success = !blocks.is_empty();
                                if !success {
                                    eprintln!(
                                        "Warning: No blocks found for module '{}' at line {}",
                                        module_name, line_number
                                    );
                                }
                                ConversionResult {
                                    bug_id: bug_id.clone(),
                                    module_name: module_name.clone(),
                                    line_number,
                                    blocks,
                                    conversion_success: success,
                                    error_message: None,
                                }
                            }
                            Err(e) => {
                                eprintln!(
                                    "Error processing module '{}' at line {}: {}",
                                    module_name, line_number, e
                                );
                                ConversionResult {
                                    bug_id: bug_id.clone(),
                                    module_name: module_name.clone(),
                                    line_number,
                                    blocks: vec![],
                                    conversion_success: false,
                                    error_message: Some(e),
                                }
                            }
                        };

                    total_conversions += 1;
                    if additional_info.conversion_success {
                        successful_conversions += 1;
                    }

                    let localized_block = additional_info.blocks.first().map(|b| b.block_id);
                    let choice = LocalizationChoiceBuilder::default()
                        .module_name(module_name.to_string())
                        .line_number(line_number)
                        .block_id(localized_block)
                        .build()
                        .unwrap();
                    choice
                })
                .collect();

            (bug_id, new_choices)
        })
        .map(|(bug_id, choices)| {
            LocalizationResultBuilder::default()
                .bug_id(bug_id)
                .choices(choices)
                .build()
                .unwrap()
        })
        .collect::<Vec<_>>();

    // Write results to output file
    let output_json = serde_json::to_string_pretty(&results)
        .map_err(|e| format!("Failed to serialize results: {}", e))?;

    write(&args.output_path, output_json)
        .map_err(|e| format!("Failed to write output file '{}': {}", args.output_path, e))?;

    println!(
        "Conversion complete! {}/{} successful conversions written to '{}'",
        successful_conversions, total_conversions, args.output_path
    );

    Ok(())
}

fn process_module_line<'a>(
    block_manager: &BlockManager<'a, DataFlowBlockParser>, // Adjust this type based on your actual block manager type
    module_name: &str,
    line_number: usize,
) -> Result<Vec<BlockInfo>, String> {
    if line_number < 0 {
        return Err("Line number is Unknown".to_string());
    }
    let line_number = line_number as u32;
    let scopes = block_manager.get_scopes();

    let blocks: Vec<BlockInfo> = scopes
        .into_iter()
        .filter_map(|scope| {
            // Get the block that contains this line number
            match block_manager.get_belong_block_from_original_lineno(&scope, line_number) {
                Some(block) => {
                    // Check if this block belongs to the specified module
                    if block.get_module_name() == module_name {
                        Some(BlockInfo {
                            block_id: block.get_bid(),
                            module_name: block.get_module_name().to_string(),
                            line_number,
                            scope: scope.to_string(),
                        })
                    } else {
                        None
                    }
                }
                None => None,
            }
        })
        .collect();

    Ok(blocks)
}
