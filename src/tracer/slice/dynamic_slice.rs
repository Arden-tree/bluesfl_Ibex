use crate::block::CircuitType;
use crate::coverage::CoverageTracker;
use crate::dataflow::NodeID;
use crate::tracer::TimeAnnotation;
use crate::{Block, BlockManager, BlockParser, BlockType, Tracer};
use anyhow::anyhow;
use async_trait::async_trait;
use log::warn;
use rayon::prelude::*;
use std::collections::HashMap;
use sv_parser::Locate;

pub struct DynamicSlicing<'a, 'b, Parser, CT>
where
    Parser: BlockParser,
    CT: CoverageTracker + Send,
{
    time_step: TimeAnnotation,
    pub block_manager: &'b BlockManager<'a, Parser>,
    coverage_tracker: CT,
    pub pos_visited: HashMap<(u64, NodeID), usize>,
}

impl<'a, 'b, Parser, CT> DynamicSlicing<'a, 'b, Parser, CT>
where
    Parser: BlockParser,
    CT: CoverageTracker + Send,
{
    pub fn new(
        time_step: TimeAnnotation,
        block_manager: &'b BlockManager<'a, Parser>,
        coverage_tracker: CT,
    ) -> DynamicSlicing<'a, 'b, Parser, CT> {
        DynamicSlicing {
            time_step,
            block_manager,
            coverage_tracker,
            pos_visited: HashMap::new(),
        }
    }
}

#[async_trait(?Send)]
impl<'a, 'b, T, Parser, CT> Tracer<'a, T> for DynamicSlicing<'a, 'b, Parser, CT>
where
    T: Block<'a> + Sync + Send,
    Parser: BlockParser<Block<'a> = T>,
    CT: CoverageTracker + Send + Sync,
{
    fn get_scope_blocks(&self, scope_name: &str) -> anyhow::Result<&[T]> {
        let res = self
            .block_manager
            .get_scope_blocks(scope_name)
            .ok_or_else(|| anyhow!("Scope {} not found", scope_name))?;
        Ok((*res).0.as_slice())
    }

    fn get_scopes(&self) -> Vec<&str> {
        self.block_manager.get_scopes()
    }
    fn get_block_covered_ast_lines(&self, block: &T, time: TimeAnnotation) -> Vec<(u32, usize)> {
        let btype = block.get_block_type();
        let covered_lines = block
            .get_covered_line_locates()
            .into_par_iter()
            .filter_map(|locate| {
                // TODO: convert lineno in AST to original lineno
                let ast_locate = Locate {
                    offset: locate.offset,
                    len: locate.len,
                    line: locate.line,
                };
                let lineno_option = self.block_manager.get_original_lineno_from_ast_locate(
                    block.get_scope(),
                    block.get_module_name(),
                    ast_locate,
                );
                if lineno_option.is_none() {
                    warn!("Cannot get_original_lineno_from_ast_locate@scope={}, module={}, ast_locate={:?}", block.get_scope(), block.get_module_name(), ast_locate);
                    None
                } else {
                    Some((
                        // (ast line will be used in following node location,
                        locate.line,
                        // original lineno will be used to check line coverage)
                        lineno_option.unwrap() as u32,
                    ))
                }
            })
            .filter_map(|(lineno, original_lineno)| {
                // get covered lineno in block's scope at time T.
                self.coverage_tracker
                    .check_line_covered(
                        Some(btype.clone()),
                        Some(block.get_scope()),
                        Some(block.get_module_name()),
                        Some(time),
                        original_lineno,
                    )
                    .map(|count| (lineno, count))
            })
            .collect::<Vec<_>>();
        covered_lines
    }
    fn get_driven_signals_in_block(
        &mut self,
        block: &T,
        btype: &BlockType,
        sig: NodeID,
        time: Option<TimeAnnotation>,
    ) -> (Option<TimeAnnotation>, Option<Vec<NodeID>>) {
        // Paper-aligned IntraBlockAnalysis (Algorithm 1, lines 13-21):
        // - COMB: coverage check at time t, propagate driven signals at time t
        // - SEQ:  coverage check at time t-1, propagate driven signals at time t-1
        //         if not covered at t-1 → register holds value, return ({sig}, t-1)

        // Compute next_time FIRST (needed for coverage check)
        let next_time = if matches!(btype, BlockType::Always(CircuitType::SEQ)) {
            Some(time.unwrap() - self.time_step)  // t-1 for SEQ
        } else {
            time  // t for COMB/Assign
        };

        // FIXME: only when a sig exist, we maintain it and repeat use sig@t-1. If this circuit not exist, we should ignore it.
        let covered_lines = if matches!(btype, BlockType::Assign) {
            // For Assign blocks, all lines are considered covered (no coverage data needed)
            block
                .get_covered_line_locates()
                .into_par_iter()
                .map(|locate| (locate.line, 1usize))
                .collect::<Vec<_>>()
        } else {
            block
                .get_covered_line_locates()
                .into_par_iter()
                .map(|locate| {
                    // TODO: convert lineno in AST to original lineno
                    let ast_locate = Locate {
                        offset: locate.offset,
                        len: locate.len,
                        line: locate.line,
                    };
                    let lineno = self
                        .block_manager
                        .get_original_lineno_from_ast_locate(
                            block.get_scope(),
                            block.get_module_name(),
                            ast_locate,
                        )
                        .unwrap();
                    (
                        // (ast line will be used in following node location,
                        locate.line,
                        // original lineno will be used to check line coverage)
                        lineno as u32,
                    )
                })
                .filter_map(|(lineno, original_lineno)| {
                    // Paper-aligned: check coverage at next_time
                    // COMB: next_time = t (same time), SEQ: next_time = t-1
                    self.coverage_tracker
                        .check_line_covered(
                            Some(btype.clone()),
                            Some(block.get_scope()),
                            Some(block.get_module_name()),
                            next_time,
                            original_lineno,
                        )
                        .map(|count| (lineno, count))
                })
                .collect::<Vec<_>>()
        };

        // sig locate may not in this block

        // find which output node that has the same name with `sig` is covered
        let local_sigs = block
            .get_output_nodes()
            .into_iter()
            .filter(|&node| node.get_text() == sig.get_text())
            // In fact, here should be only one position is covered.
            // but for parameter controlled context, we cannot know which one is actually covered.
            // so we use filter to collect all, and consider them are covered;
            .filter(|&node| {
                covered_lines
                    .iter()
                    .any(|(line, count)| node.get_locate().line == *line && *count > 0)
            })
            .collect::<Vec<_>>();

        let vars = if local_sigs.is_empty() && !covered_lines.is_empty() {
            // Only when this line is really instantiated in the final circuit
            Some(if matches!(btype, BlockType::Always(CircuitType::SEQ)) {
                // for seq, no local sig is covered, return sig again
                vec![sig.clone()]
            } else {
                // for comb, no local sig is covered, return empty
                vec![]
            })
        } else {
            let res = local_sigs
                .into_iter()
                .map(|node| {
                    // (bid, node line) can only be visited for one time;
                    let key = (block.get_bid(), node.clone());
                    let is_first_visit = self
                        .pos_visited
                        .entry(key.clone())
                        .and_modify(|count| *count += 1)
                        .or_insert(1)
                        == &1;

                    is_first_visit.then(|| {
                        block
                            .get_node_dataflow(node.clone())
                            .into_iter()
                            .filter(|node| matches!(node, NodeID::Identifier(_, _)))
                            .collect::<Vec<_>>()
                    })
                })
                .collect::<Vec<_>>();
            if res.iter().all(|x| x.is_none()) {
                None
            } else {
                Some(res.into_iter().filter_map(|v| v).flatten().collect())
            }
        };

        // Fallback for Always(SEQ): when coverage data is missing, pipeline
        // registers still have deterministic dataflow (reg <= wire).
        // Ignore coverage and do a pure dataflow lookup.
        let vars = if vars.is_none() && matches!(btype, BlockType::Always(CircuitType::SEQ)) {
            let direct_deps: Vec<NodeID> = block
                .get_output_nodes()
                .into_iter()
                .filter(|node| node.get_text() == sig.get_text())
                .flat_map(|node| block.get_node_dataflow(node.clone()).into_iter())
                .filter(|node| matches!(node, NodeID::Identifier(_, _)))
                .collect();
            if direct_deps.is_empty() {
                warn!("SEQ fallback: no dataflow deps for sig='{}'", sig.get_text());
                None
            } else {
                // Filter out control/perf signals — they cause tracing to diverge.
                // Pipeline registers should trace data, not control or perf counters.
                let filtered: Vec<NodeID> = direct_deps.iter().filter(|dep| {
                    let t = dep.get_text();
                    !t.ends_with("_valid") && !t.ends_with("_ready")
                        && t != "valid" && t != "ready"
                        && !t.contains("perfCnt") && !t.contains("perfCntCond")
                        && !t.contains("perfCntCond_") && !t.starts_with("_GEN")
                        && !t.ends_with("__bore")
                }).cloned().collect();
                let chosen = if filtered.is_empty() { direct_deps.clone() } else { filtered };
                warn!("SEQ fallback: found {} deps for sig='{}', selected {}: {:?}",
                    direct_deps.len(), sig.get_text(), chosen.len(),
                    chosen.iter().map(|n| n.get_text()).collect::<Vec<_>>());
                Some(chosen)
            }
        } else {
            vars
        };

        (next_time, vars)
    }
}
