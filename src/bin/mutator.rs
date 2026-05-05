#![recursion_limit = "256"]
use anyhow::bail;
use clap::Parser;
use dashmap::{DashMap, DashSet};
use fs_extra::dir;
use fs_extra::dir::copy_with_progress;
use fs_extra::dir::{CopyOptions, TransitProcess, TransitProcessResult};
use lazy_static::lazy_static;
use log::{debug, info, trace, warn};
use pathdiff::diff_paths;
use rand::{rng, Rng};
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::{fs, io};
use sv_analysis::{
    get_identifier_str, get_module_files, get_module_name, get_pos_from_offset, init_logger,
    read_coverage_files, save_data_to_json, setup_block_mgr, Block, BlockManager,
    CompositeCoverageTracker, CoverageTracker, DataFlowBlockParser, ParameterCoverageReport,
    PlainCoverageReport,
};
use sv_parser::{unwrap_locate, Locate, NodeEvent, RefNode, SyntaxTree};

#[derive(Parser, Debug)]
#[command(name = "mutator")]
#[command(about = "Inject bugs to SystemVerilog projects.", long_about = None)]
struct Args {
    #[arg(long)]
    project_root: String,
    #[arg(long)]
    source_path: String,
    #[arg(short, long, value_delimiter = ',')]
    include_paths: Vec<String>,
    #[arg(long)]
    rm_params_path: Option<String>,
    #[arg(short, long)]
    wave_path: Option<String>,
    #[arg(short, long)]
    coverage_path: Option<String>,
    #[arg(long)]
    top_module: String,
    #[arg(long)]
    top_scope: String,
    #[arg(long)]
    tmp_path: String,
    #[arg(long)]
    output_path: String,
    #[arg(long, default_value = "zsh")]
    script_cmd: String,
    #[arg(long)]
    script_path: String,
    #[arg(long, default_value_t = false)]
    force_oracle: bool,
    #[arg(long)]
    n: u64,
    #[arg(long, default_value_t = 0)]
    threads: usize,
}

impl Args {
    fn validate(&self) -> anyhow::Result<()> {
        let has_rm_params = self.rm_params_path.is_some();
        let has_wave = self.wave_path.is_some();
        let has_coverage = self.coverage_path.is_some();

        if has_rm_params == has_wave && has_wave == has_coverage {
            Ok(())
        } else {
            bail!("rm_params_path, wave_path, and coverage_path must all be provided together or all be omitted".to_string())
        }
    }
}

#[derive(Debug, Serialize)]
struct OracleInfo {
    bid: u64,
    module_name: String,
    scope_name: String,
}

impl OracleInfo {
    fn new(bid: u64, module_name: &str, scope_name: &str) -> OracleInfo {
        Self {
            bid,
            module_name: module_name.to_string(),
            scope_name: scope_name.to_string(),
        }
    }
}

type MutationRule = Box<dyn Fn(&Arc<SyntaxTree>, &RefNode) -> Option<String> + Send + Sync>;
type MutationPosition<'a> = (RefNode<'a>, String, usize);
const RETRY_LIMIT: u32 = 50;
lazy_static! {
    static ref SCOPE_WEIGHTS: Mutex<HashMap<String, f32>> = Mutex::new(HashMap::new());
}

fn main() -> anyhow::Result<()> {
    /*
       --
       --source-path=/home/lzz/exp_wkdir/ibex_test/ibex/rtl
       --project-root=/home/lzz/exp_wkdir/ibex_test/ibex
       --include-paths=/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/ip/prim/rtl/,/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/dv/sv/dv_utils
       --rm-params-path=/home/lzz/exp_wkdir/ibex_test/ibex/build_golden/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/rm_params.tree.json
       --wave-path=/home/lzz/exp_wkdir/ibex_test/ibex/build_golden/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/sim.fst
       --coverage-path=/home/lzz/exp_wkdir/ibex_test/ibex/build_golden/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator
       --top-module=ibex_core
       --top-scope=TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core
       --tmp-path=/tmp
       --output-path=/home/lzz/dac26/hdl_fl_data/dataset
       --script-path=scripts/ibex_boot.sh
       --n 1
       --force-oracle
    */

    /*
       --
       --source-path=/home/lzz/RustProjects/sv-analysis/tests/sample_example
       --project-root=/home/lzz/RustProjects/sv-analysis/tests/sample_example
       --include-paths=/home/lzz/RustProjects/sv-analysis/tests/sample_example
       --rm-params-path=/home/lzz/RustProjects/sv-analysis/tests/sample_example/rm_params.tree.json
       --wave-path=/home/lzz/RustProjects/sv-analysis/tests/sample_example/obj_dir/dump.vcd
       --coverage-path=/home/lzz/RustProjects/sv-analysis/tests/sample_example/obj_dir
       --top-module=top_module
       --top-scope=TOP.top_module
       --tmp-path=/tmp/mutate_result_small/tmp
       --output-path=/tmp/mutate_result_small
       --script-path=/home/lzz/RustProjects/sv-analysis/tests/sample_example/boot.sh
       --n 1
       --force-oracle
    */

    /*
       --
       --source-path=/home/lzz/hdl_llm/benchmarks/RTLLM/Miscellaneous/RISC-V/ROM
       --project-root=/home/lzz/hdl_llm/benchmarks/RTLLM/Miscellaneous/RISC-V/ROM
       --include-paths=/home/lzz/hdl_llm/benchmarks/RTLLM/Miscellaneous/RISC-V/ROM
       --top-module=ROM
       --top-scope=TOP
       --tmp-path=/tmp/mutate_result_RTLLM/tmp
       --output-path=/tmp/mutate_result_RTLLM
       --script-path=/home/lzz/RustProjects/sv-analysis/scripts/rtllm_boot.sh
       --n 1
    */

    init_logger("mutator");
    let args = Args::parse();
    args.validate()?;

    println!("{:#?}", args);

    set_threads_number(args.threads);
    output_path_guard(&args.output_path)?;

    let checker = |tmp_proj_path: &str| -> bool {
        let output = Command::new(args.script_cmd.clone())
            .arg(&args.script_path)
            .arg(tmp_proj_path)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        // if test pass, checker considers failed.
        output
    };

    let mod_path = get_module_files(&args.source_path);
    let include_paths = args
        .include_paths
        .iter()
        .map(|s| PathBuf::from(s))
        .collect::<Vec<_>>();

    let block_manager = if args.coverage_path.is_some() {
        let wave_files = read_coverage_files(&args.coverage_path.clone().unwrap());
        let compose_coverage_tracker = CompositeCoverageTracker::new(
            &wave_files,
            args.rm_params_path.as_ref().unwrap().into(),
        );
        let param_tracker = ParameterCoverageReport::new(args.rm_params_path.clone().unwrap());

        setup_block_mgr(
            &compose_coverage_tracker,
            Some(param_tracker),
            &include_paths,
            &mod_path,
            &args.top_module,
            &args.top_scope,
        )
    } else {
        let plain_coverage_tracker = PlainCoverageReport::new(&mod_path);
        setup_block_mgr(
            &plain_coverage_tracker,
            None,
            &include_paths,
            &mod_path,
            &args.top_module,
            &args.top_scope,
        )
    };

    let param_coverage_tracker = Arc::new(create_coverage_tracker_instance(&args));

    let rule_1_operator = {
        let coverage_tracker = param_coverage_tracker.clone();
        Box::new(move |syntax_tree: &Arc<SyntaxTree>, node: &RefNode| {
            let module_name = get_module_name(&syntax_tree)?;
            if let RefNode::BinaryOperator(operator) = node {
                let loc = operator.nodes.0.nodes.0;
                if !check_lineno_is_covered(&loc, syntax_tree, &module_name, &coverage_tracker)
                    .unwrap_or(false)
                {
                    return None;
                }
                let op = syntax_tree.get_str(&loc)?;
                match op {
                    "+" => Some("-"),
                    "-" => Some("+"),
                    "*" => Some("/"),
                    "/" => Some("*"),
                    "&" => Some("|"),
                    "|" => Some("&"),
                    "&&" => Some("||"),
                    "||" => Some("&&"),
                    "==" => Some("!="),
                    "!=" => Some("=="),
                    "<" => Some(">"),
                    ">" => Some("<"),
                    "<=" => Some(">="),
                    ">=" => Some("<="),
                    _ => None,
                }
                .map(|op| op.to_string())
            } else {
                None
            }
        })
    };

    let rule_2_constant_values = {
        let coverage_tracker = param_coverage_tracker.clone();
        Box::new(move |syntax_tree: &Arc<SyntaxTree>, node: &RefNode| {
            let module_name = get_module_name(&syntax_tree)?;

            match node {
                RefNode::UnsignedNumber(dec_val) => {
                    let loc = dec_val.nodes.0;
                    if !check_lineno_is_covered(&loc, syntax_tree, &module_name, &coverage_tracker)
                        .unwrap_or(false)
                    {
                        return None;
                    }
                    let dec_str = syntax_tree.get_str(&loc)?;

                    if let Ok(decimal_val) = u64::from_str_radix(dec_str, 10) {
                        let new_val = if decimal_val == 0 {
                            1
                        } else {
                            rand::rng().random_range(0..decimal_val)
                        };
                        let original_len = dec_str.len();
                        let new_str = format!("{}", new_val);

                        // If the new value has more digits, it exceeded the implicit width
                        if new_str.len() > original_len {
                            // Truncate to fit original width by using modulo
                            let max_val = 10_u64.pow(original_len as u32);
                            let truncated_val = new_val % (max_val);
                            Some(format!("{:0width$}", truncated_val, width = original_len))
                        } else {
                            Some(new_str)
                        }
                    } else {
                        None
                    }
                }

                RefNode::BinaryValue(binary_val) => {
                    let loc = binary_val.nodes.0;
                    if !check_lineno_is_covered(&loc, syntax_tree, &module_name, &coverage_tracker)
                        .unwrap_or(false)
                    {
                        return None;
                    }
                    let binary_str = syntax_tree.get_str(&loc)?;

                    if let Ok(decimal_val) = u64::from_str_radix(binary_str, 2) {
                        let new_val = if decimal_val == 0 {
                            1
                        } else {
                            rand::rng().random_range(0..decimal_val)
                        };
                        let original_width = binary_str.len();

                        // Create mask to truncate to original width
                        let mask = (1u64 << original_width) - 1;
                        let truncated_val = new_val & mask;

                        let new_binary =
                            format!("{:0width$b}", truncated_val, width = original_width);
                        Some(new_binary)
                    } else {
                        None
                    }
                }

                RefNode::OctalValue(octal_val) => {
                    let loc = octal_val.nodes.0;
                    if !check_lineno_is_covered(&loc, syntax_tree, &module_name, &coverage_tracker)
                        .unwrap_or(false)
                    {
                        return None;
                    }
                    let octal_str = syntax_tree.get_str(&loc)?;

                    if let Ok(decimal_val) = u64::from_str_radix(octal_str, 8) {
                        let new_val = if decimal_val == 0 {
                            1
                        } else {
                            rand::rng().random_range(0..decimal_val)
                        };
                        let original_width = octal_str.len();

                        // Each octal digit represents 3 bits
                        let bit_width = original_width * 3;
                        let mask = (1u64 << bit_width) - 1;
                        let truncated_val = new_val & mask;

                        let new_octal =
                            format!("{:0width$o}", truncated_val, width = original_width);
                        Some(new_octal)
                    } else {
                        None
                    }
                }

                RefNode::HexValue(hex_val) => {
                    let loc = hex_val.nodes.0;
                    if !check_lineno_is_covered(&loc, syntax_tree, &module_name, &coverage_tracker)
                        .unwrap_or(false)
                    {
                        return None;
                    }
                    let hex_str = syntax_tree.get_str(&loc)?;

                    // Parse hex value (e.g., "8'hFF" or "'hA5")
                    if let Ok(decimal_val) = u64::from_str_radix(hex_str, 16) {
                        let new_val = if decimal_val == 0 {
                            1
                        } else {
                            rand::rng().random_range(0..decimal_val)
                        };
                        let original_width = hex_str.len();

                        // Each hex digit represents 4 bits
                        let bit_width = original_width * 4;
                        let mask = (1u64 << bit_width) - 1;
                        let truncated_val = new_val & mask;

                        let new_hex = format!("{:0width$X}", truncated_val, width = original_width);
                        Some(new_hex)
                    } else {
                        None
                    }
                }

                _ => None,
            }
        })
    };

    let identifier_names: DashMap<String, Vec<String>> = DashMap::new();
    let rule_3_name_confusion = {
        let coverage_tracker = param_coverage_tracker.clone();
        Box::new(
            move |syntax_tree: &Arc<SyntaxTree>, node: &RefNode| -> Option<String> {
                let module_name = get_module_name(&syntax_tree)?;

                let candidate_names =
                    identifier_names
                        .entry(module_name.clone())
                        .or_insert_with(|| {
                            let mut res = vec![];
                            for node in syntax_tree.as_ref() {
                                if let RefNode::SimpleIdentifier(identifier) = node {
                                    if let Some(name) = unwrap_locate!(identifier)
                                        .map(|loc| syntax_tree.get_str(loc))
                                    {
                                        if let Some(name) = name {
                                            res.push(name.to_string());
                                        }
                                    }
                                }
                            }
                            res
                        });

                if let RefNode::Expression(_) = node {
                    let loc = unwrap_locate!(node.clone()).unwrap();
                    if !check_lineno_is_covered(&loc, syntax_tree, &module_name, &coverage_tracker)
                        .unwrap_or(false)
                    {
                        return None;
                    }

                    // First, collect all identifiers in this expression
                    let mut expr_identifiers = Vec::new();
                    collect_identifiers_from_expression(
                        node.clone(),
                        syntax_tree,
                        &mut expr_identifiers,
                    );

                    if expr_identifiers.is_empty() {
                        return None;
                    }

                    // Randomly select an identifier from the expression
                    let mut rng = rng();
                    let selected_identifier =
                        &expr_identifiers[rng.random_range(0..expr_identifiers.len())];
                    let loc = unwrap_locate!(selected_identifier.clone())?;
                    let original_name = syntax_tree.get_str(loc)?;
                    // ignore constant vars
                    if original_name.chars().any(|c| c.is_uppercase()) {
                        return None;
                    }

                    let best_match = find_most_similar_name(original_name, &candidate_names)?;

                    let expr_text = syntax_tree.get_str(vec![node.clone()]).unwrap();
                    let res = expr_text.replace(original_name, &best_match);
                    Some(res)
                } else {
                    None
                }
            },
        )
    };

    mutate_loop(
        &args.project_root,
        args.n,
        &args.output_path,
        &args.tmp_path,
        vec![
            rule_1_operator,
            rule_2_constant_values,
            rule_3_name_confusion,
        ],
        checker,
        &block_manager,
        args.force_oracle,
    );

    info!(
        "Final distribution of scopes: {:#?}",
        SCOPE_WEIGHTS.lock().unwrap()
    );

    Ok(())
}

fn output_path_guard(output_path: &str) -> anyhow::Result<()> {
    if PathBuf::from(output_path).is_dir() {
        if fs::read_dir(output_path)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
        {
            println!(
                "\x1b[1;33mWarning\x1b[0m: The output directory '\x1b[94m{}\x1b[0m' exists and is not empty.",
                output_path
            );
            println!("\x1b[1mDo you want to overwrite the whole folder? [y/N]\x1b[0m");

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
                println!("\x1b[1;31mOperation cancelled.\x1b[0m");
                std::process::exit(1);
            } else {
                fs::remove_dir_all(output_path)?;
                println!("\x1b[32mDirectory removed.\x1b[0m");
            }
        }
    }
    Ok(())
}

fn set_threads_number(num: usize) {
    ThreadPoolBuilder::new()
        .num_threads(num)
        .build_global()
        .unwrap();
}

fn create_coverage_tracker_instance(args: &Args) -> Box<dyn CoverageTracker + Send + Sync> {
    if args.rm_params_path.is_some() {
        let param_coverage_tracker =
            ParameterCoverageReport::new(args.rm_params_path.as_ref().unwrap());
        Box::new(param_coverage_tracker)
    } else {
        let mod_path = get_module_files(&args.source_path);
        let plain_coverage_tracker = PlainCoverageReport::new(&mod_path);
        Box::new(plain_coverage_tracker)
    }
}

fn check_lineno_is_covered(
    locate: &Locate,
    syntax_tree: &Arc<SyntaxTree>,
    module_name: &str,
    coverage_tracker: &Box<dyn CoverageTracker + Send + Sync>,
) -> Option<bool> {
    let (file_path, ast_offset) = syntax_tree.get_origin(locate)?;
    let code_text = fs::read_to_string(file_path).ok()?;
    let origin_lineno = get_pos_from_offset(&code_text, ast_offset).map(|(_col, row)| row)?;
    Some(
        coverage_tracker
            .check_line_covered(None, None, Some(module_name), None, origin_lineno as u32)
            .is_some(),
    )
}

fn mutate_loop(
    project_root: &str,
    n: u64,
    output_path: &str,
    tmp_path: &str,
    rules: Vec<MutationRule>,
    checker: impl Fn(&str) -> bool + Sync,
    block_manager: &BlockManager<DataFlowBlockParser>,
    force_oracle: bool,
) {
    fs::create_dir_all(&output_path).unwrap();
    fs::create_dir_all(&tmp_path).unwrap();

    let available_mutation_positions = get_available_mutation_positions(&rules, block_manager);

    let total_mutation_positions = available_mutation_positions
        .values()
        .map(|positions| positions.iter().len())
        .sum::<usize>();

    info!(
        "Total available mutation positions = {}",
        total_mutation_positions
    );

    let visited_modifications = DashSet::new();

    let success_total = (0..n)
        .into_par_iter()
        .map(|i| {
            trace!("generating id={}", i);
            let mut retry_count = 0;
            let mut mutation_success = false;
            let mut failed_feedback = None;

            while retry_count < RETRY_LIMIT && !mutation_success {
                if failed_feedback.is_none() {
                    debug!("[FEEDBACK] start from failed_feedback=None")
                } else {
                    debug!("[FEEDBACK] feedback from failed={:?}", failed_feedback);
                }

                let (scope_name, syntax_tree) =
                    select_random_tree(&block_manager, &failed_feedback);
                debug!("[select_random_tree] select scope={:?}", scope_name);
                let mutation_positions = available_mutation_positions.get(scope_name.as_str());
                if mutation_positions.is_none() {
                    continue;
                }
                let mutation_positions = mutation_positions.unwrap();

                if let Some((mutated_source, mod_offset, replacement, oracle_info, module_name, file_path)) =
                    apply_mutation_rules(
                        &scope_name,
                        syntax_tree,
                        &mutation_positions,
                        block_manager,
                    )
                {
                    trace!("[apply_mutation_rules] apply mod_offset={:?}, replacement={} at file path={:?}", mod_offset, replacement, file_path);
                    let modification_key = (
                        module_name.clone(),
                        mod_offset,
                        replacement.clone(),
                    );

                    if force_oracle && oracle_info.is_none() {
                        debug!("Force to save oracle_info.json, but current mutation position not belong to any block, so we will ignore it.");
                        continue;
                    }

                    // Try to insert the modification atomically
                    // If it was already present, skip this mutation
                    if visited_modifications.insert(modification_key.clone()) {
                        // Successfully inserted (wasn't present before)
                        let target_dir = format!("{}/{}", tmp_path, i);
                        write_mutated_source(
                            &PathBuf::from(project_root),
                            &PathBuf::from(&target_dir),
                            &file_path,
                            &mutated_source,
                        )
                            .expect("Failed to write mutated source");

                        // Step 3: Test the mutated source code using checker
                        if checker(&target_dir) {
                            // Step 4: Check success - if pass, save to output and continue
                            save_successful_mutation(
                                &PathBuf::from(&target_dir),
                                &PathBuf::from(output_path),
                                &file_path,
                                &mutated_source,
                                i,
                                oracle_info,
                            )
                                .expect("Failed to save mutated source");

                            mutation_success = true;
                            println!("Successful mutation {}/{} created", i, n);
                            trace!(
                                "successful_mutations saved: id={}@{}, mod_offset={:?}, replacement={} at file path={:?}",
                                i,
                                target_dir,
                                mod_offset,
                                replacement,
                                file_path
                            );
                        } else {
                            retry_count += 1;
                            failed_feedback = Some(scope_name.clone());
                            trace!("failed: {}@{}, retry_count={}", i, target_dir, retry_count);
                        }
                    } else {
                        // Modification was already visited by another thread
                        retry_count += 1;
                        failed_feedback = Some(scope_name.clone());
                        trace!(
                            "modifications@{}:{} have been visited",
                            file_path.display(),
                            mod_offset
                        );
                        continue;
                    }
                } else {
                    // No mutation possible with current rules
                    retry_count += 1;
                    failed_feedback = Some(scope_name.clone());
                }
            }

            // If we exhausted retries without success, skip this iteration
            if !mutation_success {
                warn!(
                    "Failed to create mutation after {} retries, skipping id={}...",
                    RETRY_LIMIT, i
                );
                0
            } else {
                1
            }
        })
        .sum::<u64>();

    println!(
        "\x1b[32mSuccessful mutations={}, Total n={}\x1b[0m",
        success_total, n
    );
}

fn select_random_tree(
    block_manager: &BlockManager<DataFlowBlockParser>,
    last_failed: &Option<String>,
) -> (String, Arc<SyntaxTree>) {
    let existing_scopes = block_manager.get_scopes();

    // Apply negative feedback
    if let Some(failed_scope) = last_failed {
        let mut weights = SCOPE_WEIGHTS.lock().unwrap();
        let current_weight = *weights.get(failed_scope).unwrap_or(&1.0);
        weights.insert(failed_scope.to_string(), current_weight * 0.5);
    }

    // Initialize weights and calculate total
    let mut weights = SCOPE_WEIGHTS.lock().unwrap();
    let mut total_weight = 0.0;
    for scope in &existing_scopes {
        let prob = if scope.contains("cs_registers_i") {
            1.0 / 26.0
        } else {
            1.0
        };
        let weight = weights.entry(scope.to_string()).or_insert(prob);
        total_weight += *weight;
    }

    // Weighted random selection
    use rand::Rng;
    let mut rng = rng();
    let mut random_point = rng.random::<f32>() * total_weight;

    for scope in &existing_scopes {
        let weight = weights.get(&scope.to_string()).unwrap();
        if random_point <= *weight {
            let (_, tree) = block_manager.get_scope_blocks(scope).unwrap();
            return (scope.to_string(), tree.clone());
        }
        random_point -= weight;
    }

    // Fallback
    let chosen_scope = &existing_scopes[0];
    let (_, tree) = block_manager.get_scope_blocks(chosen_scope).unwrap();
    (chosen_scope.to_string(), tree.clone())
}

fn get_available_mutation_positions<'a, R>(
    rules: &[R],
    block_manager: &'a BlockManager<DataFlowBlockParser>,
) -> HashMap<String, Vec<MutationPosition<'a>>>
where
    R: Fn(&Arc<SyntaxTree>, &RefNode) -> Option<String> + Sync,
{
    let res = DashMap::new();
    block_manager
        .get_scopes()
        .into_par_iter()
        .for_each(|scope| {
            // Walk through the syntax tree and collect all possible mutations
            let mut mutations = Vec::new();

            let (_, syntax_tree) = block_manager.get_scope_blocks(scope).unwrap();
            let mut disable = false;
            for event in syntax_tree.as_ref().into_iter().event() {
                match event {
                    // disable mutation on ports
                    NodeEvent::Enter(RefNode::AnsiPortDeclaration(_)) => {
                        disable = true;
                    }
                    NodeEvent::Leave(RefNode::AnsiPortDeclaration(_)) => {
                        disable = false;
                    }
                    NodeEvent::Enter(node) if !disable => {
                        for (rule_i, rule) in rules.iter().enumerate() {
                            if let Some(replacement) = rule(&syntax_tree, &node) {
                                mutations.push((node.clone(), replacement, rule_i));
                            }
                        }
                    }
                    _ => {}
                }
            }

            res.insert(scope.to_string(), mutations);
        });
    res.into_iter().collect::<HashMap<_, _>>()
}

fn apply_mutation_rules<'a>(
    scope_name: &str,
    syntax_tree: Arc<SyntaxTree>,
    mutation_positions: &Vec<MutationPosition<'a>>,
    block_manager: &'a BlockManager<DataFlowBlockParser>,
) -> Option<(String, usize, String, Option<OracleInfo>, String, PathBuf)> {
    if mutation_positions.is_empty() {
        return None;
    }

    let mut rng = rng();
    let (node_to_replace, replacement, _) =
        &mutation_positions[rng.random_range(0..mutation_positions.len())];

    let ast_lineno = unwrap_locate!(node_to_replace.clone())?;
    let (path, _) = syntax_tree.get_origin(ast_lineno)?;
    let module_name = get_module_name(&syntax_tree).unwrap_or("UnknownModuleName".to_string());

    let oracle_info = block_manager
        .get_belong_block_from_ast_locate(scope_name, ast_lineno)
        .map(|block| {
            let bid: u64 = block.get_bid();
            OracleInfo::new(bid, &module_name, scope_name)
        });

    // Apply the mutation to create new source code
    let (mutated_source, offset) = apply_replacement(&syntax_tree, node_to_replace, replacement)?;
    Some((
        mutated_source,
        offset,
        replacement.clone(),
        oracle_info,
        module_name,
        path.clone(),
    ))
}

fn apply_replacement(
    syntax_tree: &Arc<SyntaxTree>,
    node: &RefNode,
    replacement: &str,
) -> Option<(String, usize)> {
    let locate = unwrap_locate!(node.clone())?;
    let (file_path, offset) = syntax_tree.get_origin(&locate)?;
    let code = fs::read_to_string(file_path).ok()?;
    let pre = &code[..offset];
    let post = &code[offset + locate.len..];
    Some((
        format!("{}{}{}\n", pre, replacement.to_string(), post),
        offset,
    ))
}

fn write_mutated_source<P: AsRef<Path>>(
    project_root: &P,
    target_dir: &P,
    src_file_path: &P,
    mutated_source: &str,
) -> anyhow::Result<()> {
    // Note that src_file_path is the original abs path of the mutated file.
    fs::create_dir_all(target_dir)?;

    let mut options = CopyOptions::new();
    options.overwrite = true;
    options.copy_inside = true;
    options.content_only = true;
    // Filter function: skip .git
    let handle_process = |process_info: TransitProcess| -> TransitProcessResult {
        let file_name = process_info.file_name.to_string();
        if file_name.contains(".git") {
            TransitProcessResult::Skip
        } else {
            TransitProcessResult::ContinueOrAbort
        }
    };

    copy_with_progress(project_root, target_dir, &options, handle_process)?;

    let relative_path = diff_paths(src_file_path, project_root).unwrap();
    let target_file = target_dir.as_ref().join(relative_path);
    assert!(target_file.exists());
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&target_file)?;

    file.write_all(mutated_source.as_bytes())?;
    file.flush()?;
    Ok(())
}

fn save_successful_mutation<P: AsRef<Path>>(
    tmp_path: &P,
    output_path: &P,
    original_file: &P,
    mutated_source: &str,
    mutation_id: u64,
    oracle_info: Option<OracleInfo>,
) -> anyhow::Result<()> {
    // create save_dir
    let save_path = output_path.as_ref().join(mutation_id.to_string());
    if save_path.exists() {
        dir::remove(&save_path)?;
    }
    fs::create_dir_all(&save_path)?;

    let file_name = original_file
        .as_ref()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let output_file_path = save_path.join(file_name);
    let mut file = fs::File::create(&output_file_path)?;

    file.write_all(mutated_source.as_bytes())?;

    let diff_output = Command::new("diff")
        .arg("-u")
        .arg(&original_file.as_ref().display().to_string())
        .arg(&output_file_path)
        .output()?;

    let diff_file_path = save_path.join(format!("{}.diff", file_name));
    fs::write(diff_file_path, diff_output.stdout)?;

    let mut options = CopyOptions::new();
    options.overwrite = true;
    options.copy_inside = true;
    dir::copy(tmp_path, &save_path, &options)?;

    if let Some(oracle_info) = oracle_info {
        let oracle_file_path = save_path.join("oracle_info.json");
        save_data_to_json(&oracle_info, &oracle_file_path)?;
    }

    Ok(())
}

fn collect_identifiers_from_expression<'a>(
    expr_node: RefNode<'a>,
    syntax_tree: &SyntaxTree,
    identifiers: &mut Vec<RefNode<'a>>,
) {
    for node in expr_node {
        if let RefNode::Identifier(identifier) = node {
            if let Some(_) = get_identifier_str(syntax_tree, identifier) {
                identifiers.push(node.clone());
            }
        }
    }
}

// Helper function to find the most similar name using Levenshtein distance
fn find_most_similar_name(original: &str, candidates: &[String]) -> Option<String> {
    if candidates.is_empty() {
        return None;
    }

    let mut best_match = None;
    let mut best_distance = usize::MAX;

    for candidate in candidates {
        if candidate == original {
            continue; // Skip identical names
        }

        let distance = levenshtein_distance(original, candidate);
        if distance < best_distance {
            best_distance = distance;
            best_match = Some(candidate.clone());
        }
    }

    best_match
}

// Simple Levenshtein distance implementation
fn levenshtein_distance(s1: &str, s2: &str) -> usize {
    let s1_upper = s1
        .chars()
        .any(|c| c.is_uppercase())
        .then_some(s1.to_uppercase())
        .unwrap_or(s1.to_string());
    let s1 = s1_upper.as_str();

    let s2_upper = s2
        .chars()
        .any(|c| c.is_uppercase())
        .then_some(s2.to_uppercase())
        .unwrap_or(s2.to_string());
    let s2 = s2_upper.as_str();

    let len1 = s1.chars().count();
    let len2 = s2.chars().count();

    if len1 == 0 {
        return len2;
    }
    if len2 == 0 {
        return len1;
    }

    let mut matrix = vec![vec![0; len2 + 1]; len1 + 1];

    for i in 0..=len1 {
        matrix[i][0] = i;
    }
    for j in 0..=len2 {
        matrix[0][j] = j;
    }

    let s1_chars: Vec<char> = s1.chars().collect();
    let s2_chars: Vec<char> = s2.chars().collect();

    for i in 1..=len1 {
        for j in 1..=len2 {
            let cost = if s1_chars[i - 1] == s2_chars[j - 1] {
                0
            } else {
                1
            };
            matrix[i][j] = std::cmp::min(
                std::cmp::min(
                    matrix[i - 1][j] + 1, // deletion
                    matrix[i][j - 1] + 1, // insertion
                ),
                matrix[i - 1][j - 1] + cost, // substitution
            );
        }
    }

    matrix[len1][len2]
}
