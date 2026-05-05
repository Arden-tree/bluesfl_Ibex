use clap::Parser;
use log::{debug, error, info, warn};
use rig::completion::Usage;
use rig::pipeline::Op;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::Path;
use sv_analysis::token_price as get_token_price;
use sv_analysis::{init_logger, BugIDType, LocalizationChoice, LocalizationResult};

#[derive(Parser, Debug)]
#[command(name = "cal_metric")]
#[command(
    about = "Calculate localization precision by comparing predictions with ground truth",
    long_about = "A tool that calculates the precision of localization predictions by comparing them against ground truth data. \
                  Takes a JSON file containing localization predictions and compares them with oracle information from \
                  numbered directories containing oracle_info.json files. The tool matches predictions based on block_id \
                  and module_name to determine accuracy."
)]
struct Args {
    #[arg(
        short,
        long,
        help = "JSON file containing localization predictions, an array of predictions on different bugs"
    )]
    predictions: String,
    #[arg(
        short,
        long,
        help = "Root directory containing oracle ground truth data (numbered subdirectories with oracle_info.json)"
    )]
    oracle: String,
    #[arg(
        short,
        long,
        help = "Enable verbose output",
        action = clap::ArgAction::SetTrue
    )]
    verbose: bool,
    #[arg(long)]
    model_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OracleInfo {
    pub bid: u64,
    pub module_name: String,
    pub scope_name: String,
}

#[derive(Debug)]
pub struct GroundTruth {
    pub bug_id: BugIDType,
    pub module_name: String,
    pub block_id: u64,
}

#[derive(Debug)]
pub struct MetricResult {
    // number of bugs
    pub total_bugs: usize,
    pub top_1: usize,
    pub top_5: usize,
    pub top_10: usize,
    // any prediction is correct, correct_num += 1
    pub top_any: usize,
    pub avg_tokens: Option<f64>,
    pub avg_cost: Option<f64>,
    pub top_1_bids: Vec<BugIDType>,
    pub top_10_bids: Vec<BugIDType>,
}

impl Display for MetricResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Metric Result:\n\
             ----------------------------\n\
             Correct Predictions : {}\n\
             Total Bugs          : {}\n\
             Top-1 Accuracy      : {}\n\
             Top-5 Accuracy      : {}\n\
             Top-10 Accuracy      : {}\n\
             Top-any Accuracy     : {}",
            self.top_any, self.total_bugs, self.top_1, self.top_5, self.top_10, self.top_any
        )
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logger("cal_metric");

    /*
        --
        --predictions=./lik_full_bid_results.json
        --oracle=/home/lzz/dac26/hdl_fl_data/mutate_result
    */

    let args = Args::parse();

    info!("Starting localization precision calculation");
    info!("Prediction file: {}", args.predictions);
    info!("Oracle root: {}", args.oracle);

    let ground_truth = load_ground_truth(&args.oracle)?;

    // Using serde_json::Value for generic additional_info
    let predictions: Vec<LocalizationResult> = load_predictions(&args.predictions)?;

    info!("=== Starting Evaluation ===");
    let metric_res = calculate_metric(&predictions, &ground_truth, args.model_name);

    println!("{:#?}", metric_res);
    Ok(())
}

pub fn load_ground_truth(
    oracle_root: &str,
) -> Result<Vec<GroundTruth>, Box<dyn std::error::Error>> {
    let mut ground_truth = Vec::new();
    let root_path = Path::new(oracle_root);

    if !root_path.exists() {
        error!("Oracle root directory does not exist: {}", oracle_root);
        return Err(format!("Oracle root directory does not exist: {}", oracle_root).into());
    }

    info!("Reading oracle directory: {}", oracle_root);

    for entry in fs::read_dir(root_path)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // Get bug_id from folder name
            if let Some(folder_name) = path.file_name().and_then(|n| n.to_str()) {
                if let Ok(bug_id) = folder_name.parse::<BugIDType>() {
                    if bug_id == "init" {
                        continue
                    }

                    let oracle_file = path.join("oracle_info.json");

                    if oracle_file.exists() {
                        debug!("Processing oracle file: {:?}", oracle_file);
                        let oracle_content = fs::read_to_string(&oracle_file)?;
                        let oracle_info: OracleInfo = serde_json::from_str(&oracle_content)?;

                        ground_truth.push(GroundTruth {
                            bug_id,
                            module_name: oracle_info.module_name,
                            block_id: oracle_info.bid,
                        });
                    } else {
                        warn!("oracle_info.json not found in folder {}", folder_name);
                    }
                } else {
                    debug!("Skipping non-numeric folder: {}", folder_name);
                }
            }
        }
    }

    info!("Loaded {} ground truth entries", ground_truth.len());
    Ok(ground_truth)
}

pub fn load_predictions(prediction_file: &str) -> anyhow::Result<Vec<LocalizationResult>> {
    info!("Loading predictions from: {}", prediction_file);
    let content = fs::read_to_string(prediction_file)?;
    let predictions: Vec<LocalizationResult> = serde_json::from_str(&content)?;
    info!("Loaded {} predictions", predictions.len());
    Ok(predictions)
}

pub fn calculate_metric(
    predictions: &[LocalizationResult],
    ground_truth: &[GroundTruth],
    model_name: Option<String>,
) -> MetricResult {
    // Map bug_id -> (oracle_module_name, oracle_block_id)
    let mut gt_map: HashMap<BugIDType, (String, u64)> = HashMap::new();
    for gt in ground_truth {
        gt_map.insert(gt.bug_id.clone(), (gt.module_name.clone(), gt.block_id));
    }

    // Group predictions by bug_id
    let mut pred_map: HashMap<BugIDType, Vec<(Option<Usage>, Option<f64>, LocalizationChoice)>> =
        HashMap::new();
    for pred in predictions {
        pred_map.entry(pred.bug_id.clone()).or_default().extend(
            pred.choices
                .clone()
                .into_iter()
                .map(|choice| {
                    (
                        pred.token_usage.map(|usage| usage),
                        pred.token_price,
                        choice,
                    )
                })
                .collect::<Vec<_>>(),
        );
    }

    let mut top1_hits = 0;
    let mut top5_hits = 0;
    let mut top10_hits = 0;
    let mut top_any_hits = 0;
    let mut top_1_bids = vec![];
    let mut top_10_bids = vec![];

    let total_bugs = gt_map.len();
    let mut total_tokens = 0.;
    let mut total_cost = 0.;

    for (bug_id, (oracle_module, oracle_block)) in gt_map.iter() {
        let mut choices = match pred_map.get(bug_id) {
            Some(c) => c.clone(),
            None => vec![],
        };
        // Sort choices by score descending
        choices.sort_by(|(_, _, a), (_, _, b)| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Check top-1
        if choices
            .get(0)
            .map_or(false, |(token_usage, token_price, c)| {
                if c.module_name.as_deref() == Some(oracle_module)
                    && c.block_id == Some(*oracle_block)
                {
                    if let Some(usage) = token_usage {
                        total_tokens += usage.total_tokens as f64;

                        match token_price {
                            None => {
                                if let Some(model_name) = model_name.clone() {
                                    if let Some(token_price) = get_token_price(&usage, &model_name)
                                    {
                                        total_tokens += usage.total_tokens as f64;
                                        total_cost += token_price;
                                    }
                                }
                            }
                            Some(price) => {
                                total_cost += *price;
                            }
                        }
                    }
                    top_1_bids.push(bug_id.clone());
                    true
                } else {
                    false
                }
            })
        {
            top1_hits += 1;
        }

        // Check top-5
        if choices.iter().take(5).any(|(_, _, c)| {
            c.module_name.as_deref() == Some(oracle_module) && c.block_id == Some(*oracle_block)
        }) {
            top5_hits += 1;
        }

        // Check top-10
        if choices.iter().take(10).any(|(_, _, c)| {
            if c.module_name.as_deref() == Some(oracle_module) && c.block_id == Some(*oracle_block)
            {
                top_10_bids.push(bug_id.clone());
                true
            } else {
                false
            }
        }) {
            top10_hits += 1;
        }

        // Check top_any
        if choices.iter().any(|(_, _, c)| {
            c.module_name.as_deref() == Some(oracle_module) && c.block_id == Some(*oracle_block)
        }) {
            top_any_hits += 1;
        }
    }

    let avg_tokens = if top1_hits > 0 {
        Some(total_tokens / top1_hits as f64)
    } else {
        None
    };
    let avg_cost = if top1_hits > 0 {
        Some(total_cost / top1_hits as f64)
    } else {
        None
    };

    assert_eq!(top_1_bids.len(), top1_hits);

    MetricResult {
        total_bugs,
        top_1: top1_hits,
        top_5: top5_hits,
        top_10: top10_hits,
        top_any: top_any_hits,
        avg_tokens,
        avg_cost,
        top_1_bids,
        top_10_bids,
    }
}
