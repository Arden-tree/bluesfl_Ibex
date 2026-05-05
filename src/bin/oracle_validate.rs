#![recursion_limit = "256"]
use clap::Parser;
use log::warn;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use sv_analysis::{
    get_module_files, init_logger, read_coverage_files, setup_block_mgr, Block, BlockManager,
    CompositeCoverageTracker, DataFlowBlockParser, ParameterCoverageReport,
};

#[derive(Parser, Debug)]
#[command(name = "oracle_validate")]
#[command(about = "Inject bugs to SystemVerilog projects.", long_about = None)]
struct Args {
    #[arg(long)]
    dataset_path: String,

    #[arg(long, default_value = "0")]
    /// Number of threads to use (0 = auto-detect)
    threads: usize,
}

#[derive(Deserialize, Serialize, Debug)]
struct OracleInfo {
    bid: u64,
    module_name: String,
    scope_name: String,
}

#[derive(Debug)]
struct ValidationResult {
    bug_project_name: String,
    status: ValidationStatus,
}

#[derive(Debug)]
enum ValidationStatus {
    Success { original_bid: u64, correct_bid: u64 },
    Consistent { bid: u64 },
    Error(String),
}

/// the folder structure at dataset_path is:
/// dataset_path:
/// - 62
///     - *.sv.diff
///     - oracle_info.json
/// - 75
///     - *.sv.diff
///     - oracle_info.json
/// - 23
/// - ...
///
/// the data in oracle_info.json is like:
/// ```json
/// {
//   "bid": 822,
//   "module_name": "ibex_wb_stage",
//   "scope_name": "TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core.wb_stage_i"
// }
/// ```
/// bid is block id.
///
/// Now the problem is the bid may be not consist with the diff file.
/// the bid's block code snippet should contain the added line in diff file.
///
/// A diff file look like:
/// ```diff
/// --- /home/lzz/exp_wkdir/ibex_test/ibex/rtl/ibex_wb_stage.sv 2025-06-03 20:44:21.840868252 +0800
// +++ /tmp/mutate_result/62/ibex_wb_stage.sv  2025-06-07 11:22:16.409867508 +0800
// @@ -242,7 +242,7 @@
//
//    // RF write data can come from ID results (all RF writes that aren't because of loads will come
//    // from here) or the LSU (RF writes for load data)
// -  assign rf_wdata_wb_o = ({32{rf_wdata_wb_mux_we[0]}} & rf_wdata_wb_mux[0]) |
// +  assign rf_wdata_wb_o = ({32{rf_wdata_wb_mux_we[0]}} & rf_wdata_wb_mux[0]) &
//                           ({32{rf_wdata_wb_mux_we[1]}} & rf_wdata_wb_mux[1]);
//    assign rf_we_wb_o    = |rf_wdata_wb_mux_we;
//
// @@ -250,3 +250,4 @@
//
//    `ASSERT(RFWriteFromOneSourceOnly, $onehot0(rf_wdata_wb_mux_we))
//  endmodule
// +
/// ```
/// your task is to find the correct block that code context contains the line:
/// `assign rf_wdata_wb_o = ({32{rf_wdata_wb_mux_we[0]}} & rf_wdata_wb_mux[0]) &`
/// if the bid is not consist with `bid` in json file, println
/// and save the correct bid to json
///

fn extract_diff_line(diff_content: &str) -> Option<String> {
    for line in diff_content.lines() {
        if line.starts_with("+") && !line.starts_with("+++") && line.len() > 1 {
            // Remove the '+' prefix and trim whitespace
            let diff_line = line[1..].trim();
            // Skip empty lines or lines that are just whitespace
            if !diff_line.is_empty() {
                return Some(diff_line.to_string());
            }
        }
    }
    None
}

fn process_bug_project(entry: fs::DirEntry) -> ValidationResult {
    let path = entry.path();
    let bug_project_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name.to_string(),
        None => {
            return ValidationResult {
                bug_project_name: "unknown".to_string(),
                status: ValidationStatus::Error("Invalid project name".to_string()),
            }
        }
    };

    // Read oracle_info.json
    let oracle_info_path = path.join("oracle_info.json");
    if !oracle_info_path.exists() {
        return ValidationResult {
            bug_project_name,
            status: ValidationStatus::Error("oracle_info.json not found".to_string()),
        };
    }

    let oracle_info_content = match fs::read_to_string(&oracle_info_path) {
        Ok(content) => content,
        Err(e) => {
            return ValidationResult {
                bug_project_name,
                status: ValidationStatus::Error(format!("Failed to read oracle_info.json: {}", e)),
            }
        }
    };

    let oracle_info: OracleInfo = match serde_json::from_str(&oracle_info_content) {
        Ok(info) => info,
        Err(e) => {
            return ValidationResult {
                bug_project_name,
                status: ValidationStatus::Error(format!("Failed to parse oracle_info.json: {}", e)),
            }
        }
    };

    // Find and read diff file
    let diff_files: Vec<_> = match fs::read_dir(&path) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map_or(false, |ext| ext == "diff")
            })
            .collect(),
        Err(e) => {
            return ValidationResult {
                bug_project_name,
                status: ValidationStatus::Error(format!("Failed to read directory: {}", e)),
            }
        }
    };

    if diff_files.is_empty() {
        return ValidationResult {
            bug_project_name,
            status: ValidationStatus::Error("No diff file found".to_string()),
        };
    }

    let diff_path = diff_files[0].path();
    let diff_content = match fs::read_to_string(&diff_path) {
        Ok(content) => content,
        Err(e) => {
            return ValidationResult {
                bug_project_name,
                status: ValidationStatus::Error(format!("Failed to read diff file: {}", e)),
            }
        }
    };

    // Extract the diff line (added line starting with +)
    let diff_line = match extract_diff_line(&diff_content) {
        Some(line) => line,
        None => {
            return ValidationResult {
                bug_project_name,
                status: ValidationStatus::Error("No added line found in diff".to_string()),
            }
        }
    };

    // remove comments from diff_line
    let diff_line = if let Some(pos) = diff_line.find("//") {
        // Remove everything from "//" to the end of the line
        let mut result = diff_line[..pos].to_string();
        // Keep any trailing whitespace that might be meaningful
        // result.push_str(&diff_line[pos..]);
        result.trim().to_string()
    } else {
        diff_line.to_string()
    };

    // Get block manager and find matching blocks
    let block_mgr = match std::panic::catch_unwind(|| {
        get_block_mgr(path.join(&bug_project_name).to_str().unwrap())
    }) {
        Ok(mgr) => mgr,
        Err(_) => {
            return ValidationResult {
                bug_project_name,
                status: ValidationStatus::Error("Failed to create block manager".to_string()),
            }
        }
    };

    let blocks: Vec<_> = block_mgr
        .get_scopes()
        .iter()
        .flat_map(|scope| {
            block_mgr
                .get_scope_blocks(scope)
                .map(|(blocks, _)| {
                    blocks
                        .into_iter()
                        .filter(|block| {
                            let ctx = block.get_ctx().join("\n");
                            ctx.contains(&diff_line)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or(vec![])
        })
        .collect();

    if blocks.is_empty() {
        return ValidationResult {
            bug_project_name,
            status: ValidationStatus::Error("No block contains diff line".to_string()),
        };
    }

    let block = blocks.get(0).cloned().unwrap();
    let real_bid = block.get_bid();

    // Check if bid is consistent with json's bid
    if real_bid != oracle_info.bid {
        ValidationResult {
            bug_project_name,
            status: ValidationStatus::Success {
                original_bid: oracle_info.bid,
                correct_bid: real_bid,
            },
        }
    } else {
        ValidationResult {
            bug_project_name,
            status: ValidationStatus::Consistent { bid: real_bid },
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    /*
       --
       --dataset-path=/home/lzz/dac26/hdl_fl_data/dataset
    */
    init_logger("oracle_validate");
    let args = Args::parse();

    // Configure Rayon thread pool
    if args.threads > 0 {
        rayon::ThreadPoolBuilder::new()
            .num_threads(args.threads)
            .build_global()
            .expect("Failed to build thread pool");
    }

    println!(
        "Using {} threads for parallel processing",
        rayon::current_num_threads()
    );

    // Collect all entries first
    let entries: Vec<_> = fs::read_dir(&args.dataset_path)?.collect::<Result<Vec<_>, _>>()?;

    println!("Processing {} bug projects...", entries.len());

    // Process entries in parallel
    let results: Vec<ValidationResult> = entries
        .into_par_iter()
        // debug
        // .filter(|entry| {
        //     entry
        //         .file_name()
        //         .to_str()
        //         .map_or(false, |name| name == "45")
        // })
        .map(|entry| process_bug_project(entry))
        .collect();

    // Process results and update files sequentially to avoid conflicts
    let mut consistent_count = 0;
    let mut mismatch_count = 0;
    let mut error_count = 0;
    let mut mismatched_projects = Vec::new();
    let mut error_projects = Vec::new();

    for result in results {
        match result.status {
            ValidationStatus::Success {
                original_bid,
                correct_bid,
            } => {
                println!(
                    "BID mismatch for bug {}: JSON has {}, but correct BID is {}",
                    result.bug_project_name, original_bid, correct_bid
                );
                mismatch_count += 1;
                mismatched_projects.push((
                    result.bug_project_name.clone(),
                    original_bid,
                    correct_bid,
                ));

                // Update the JSON file

                let path = PathBuf::from(&args.dataset_path).join(&result.bug_project_name);
                let oracle_info_path = path.join("oracle_info.json");
                if let Ok(oracle_info_content) = fs::read_to_string(&oracle_info_path) {
                    if let Ok(mut oracle_info) =
                        serde_json::from_str::<OracleInfo>(&oracle_info_content)
                    {
                        oracle_info.bid = correct_bid;
                        if let Ok(updated_json) = serde_json::to_string_pretty(&oracle_info) {
                            if let Err(e) = fs::write(&oracle_info_path, updated_json) {
                                warn!(
                                    "Failed to update oracle_info.json for {}: {}",
                                    result.bug_project_name, e
                                );
                            } else {
                                println!(
                                    "Updated oracle_info.json with correct BID: {}",
                                    correct_bid
                                );
                            }
                        }
                    }
                }
            }
            ValidationStatus::Consistent { bid } => {
                println!(
                    "BID is consistent for bug {}: {}",
                    result.bug_project_name, bid
                );
                consistent_count += 1;
            }
            ValidationStatus::Error(error) => {
                warn!(
                    "Error processing bug {}: {}",
                    result.bug_project_name, error
                );
                error_count += 1;
                error_projects.push((result.bug_project_name.clone(), error));
            }
        }
    }

    println!("\n=== Summary ===");
    println!("Consistent BIDs: {}", consistent_count);
    println!("Mismatched BIDs: {}", mismatch_count);
    println!("Errors: {}", error_count);
    println!(
        "Total processed: {}",
        consistent_count + mismatch_count + error_count
    );

    // Print all mismatched bug projects
    if !mismatched_projects.is_empty() {
        println!("\n=== Mismatched Bug Projects ===");
        println!("Project Name\t\tOriginal BID\tCorrect BID");
        println!("{}", "-".repeat(60));
        for (project_name, original_bid, correct_bid) in &mismatched_projects {
            println!("{}\t\t{}\t\t{}", project_name, original_bid, correct_bid);
        }

        println!("\n=== Mismatched Project Names Only ===");
        let project_names: Vec<_> = mismatched_projects
            .iter()
            .map(|(name, _, _)| name.clone())
            .collect();
        println!("[{}]", project_names.join(", "));
    } else {
        println!("\n=== No Mismatched Bug Projects Found ===");
    }

    // Print all error bug projects
    if !error_projects.is_empty() {
        println!("\n=== Error Bug Projects ===");
        println!("Project Name\t\tError Reason");
        println!("{}", "-".repeat(80));
        for (project_name, error_reason) in &error_projects {
            println!("{}\t\t{}", project_name, error_reason);
        }

        println!("\n=== Error Project Names Only ===");
        let error_project_names: Vec<_> = error_projects
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        println!("[{}]", error_project_names.join(", "));
    } else {
        println!("\n=== No Error Bug Projects Found ===");
    }

    Ok(())
}

fn get_block_mgr(base_path: &str) -> BlockManager<'static, DataFlowBlockParser> {
    let coverage_path =
        format!("{base_path}/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator");
    let rm_params_path = format!("{base_path}/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/rm_params.tree.json");
    let include_paths = vec![
        format!("{base_path}/vendor/lowrisc_ip/ip/prim/rtl/"),
        format!("{base_path}/vendor/lowrisc_ip/dv/sv/dv_utils/"),
    ];

    let source_path = format!("{base_path}/rtl");
    let top_module = "ibex_core";
    let top_scope = "TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core";
    let wave_files = read_coverage_files(coverage_path);
    let coverage_tracker =
        CompositeCoverageTracker::new(&wave_files, rm_params_path.clone().into());
    let param_tracker = ParameterCoverageReport::new(rm_params_path);
    let include_paths = include_paths
        .iter()
        .map(|s| PathBuf::from(s))
        .collect::<Vec<_>>();
    let mod_path = get_module_files(source_path);
    let block_manager = setup_block_mgr(
        &coverage_tracker,
        Some(param_tracker),
        &include_paths,
        &mod_path,
        top_module,
        top_scope,
    );
    block_manager
}
