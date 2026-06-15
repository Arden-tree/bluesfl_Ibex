use crate::block::Block;
use crate::utils::extract_signal_suffix;
use crate::{get_last_scope, BlockType, CircuitType};
use async_trait::async_trait;
use log::{debug, info, warn};
use std::collections::{HashSet, VecDeque};
use std::fmt::{Debug, Formatter};

pub mod llm;
pub mod slice;
mod utils;
use crate::dataflow::NodeID;
pub use utils::save_trace_to_json;

pub type SuspiciousScore = i64;
pub type TimeAnnotation = i64;

pub enum EarlyStop {
    Module(Vec<(String, NodeID, Option<TimeAnnotation>)>),
    Block,
    None,
}

impl Debug for EarlyStop {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            EarlyStop::Module(data) => {
                write!(f, "EarlyStop::Module({:?})", data)
            }
            EarlyStop::Block => {
                write!(f, "EarlyStop::Block")
            }
            EarlyStop::None => {
                write!(f, "EarlyStop::None")
            }
        }
    }
}

#[async_trait(?Send)]
pub trait Tracer<'a, T>
where
    T: Block<'a> + Sync + Send,
{
    fn get_scope_blocks(&self, scope_name: &str) -> anyhow::Result<&[T]>;
    fn get_scopes(&self) -> Vec<&str>;
    fn reset_visit_tracking(&mut self) {}
    // get current block covered ast lines (ast_lineno, count)
    fn get_block_covered_ast_lines(&self, block: &T, time: TimeAnnotation) -> Vec<(u32, usize)>;
    fn get_next_scopes(&self, scope_name: &str) -> Vec<&str> {
        self.get_scopes()
            .iter()
            .filter(|scope| scope.starts_with(scope_name) && scope.len() > scope_name.len())
            .map(|s| *s)
            .collect::<Vec<&str>>()
    }
    fn get_node_id_from_output_port(
        &self,
        scope_name: &str,
        sig: &str,
    ) -> anyhow::Result<Vec<NodeID>> {
        let res = self
            .get_scope_blocks(scope_name)?
            .into_iter()
            .filter(|block| matches!(block.get_block_type(), BlockType::ModuleOutput))
            .map(|block| block.get_input_nodes())
            .flatten()
            .filter(|node| node.get_text() == sig)
            .map(|node| node.clone())
            .collect::<Vec<_>>();
        Ok(res)
    }

    fn get_node_id_from_internal(
        &self,
        scope_name: &str,
        sig: &str,
    ) -> anyhow::Result<Vec<NodeID>> {
        let mut res = self
            .get_scope_blocks(scope_name)?
            .into_iter()
            .filter(|block| !matches!(block.get_block_type(), BlockType::ModuleOutput))
            .map(|block| block.get_output_nodes())
            .flatten()
            .filter(|node| node.get_text() == sig)
            .map(|node| node.clone())
            .collect::<Vec<_>>();

        let sub_modules_output = self
            .get_next_scopes(scope_name)
            .iter()
            .map(|sub_scope| self.get_scope_blocks(sub_scope))
            .flatten()
            .flatten()
            .into_iter()
            .map(|block| block.get_output_nodes())
            .flatten()
            .filter(|node| node.get_text() == sig)
            .map(|node| node.clone())
            .collect::<Vec<_>>();
        res.extend(sub_modules_output);
        Ok(res)
    }

    /// return next_scope with blocks whose output contains sig
    /// the reason we use Vec to wrap (scope, blocks)
    /// there may be some parameter controlled blocks or module instantiations, which cannot be processed by our approach;
    /// this cause:
    ///     1. in the cur_scope, many block's output contain the same sig;
    ///     2. in the module instantiation, many instantiation's output port contain the same sig;
    fn get_block(&self, scope_name: &str, sig: &NodeID) -> anyhow::Result<Vec<(String, Vec<T>)>> {
        // TODO: I'm not sure whether to return a list of blocks, as `parameter` configuration may cause that two modules that have the same output signal.

        let scope_blocks = self
            .get_scope_blocks(scope_name)?
            .into_iter()
            .filter(|block| {
                block
                    .get_output_nodes()
                    .iter()
                    .any(|node| node.get_text() == sig.get_text())
                    && !matches!(block.get_block_type(), BlockType::ModuleOutput)
            })
            .map(|block| block.clone())
            .collect::<Vec<_>>();
        if !scope_blocks.is_empty() {
            if scope_blocks.len() > 1 {
                info!("Found multiple blocks having the same output variable. There maybe parameter controlled module instantiation.");
            }
            return Ok(vec![(scope_name.to_string(), scope_blocks)]);
        }

        // We not only search in current scope, but also in the next scope.
        // Because the output variable names of output block in next scope module are in current scope.
        let res_in_next_scope_blocks = self
            .get_next_scopes(scope_name)
            .into_iter()
            .map(|next_scope| (next_scope.to_string(), self.get_scope_blocks(next_scope)))
            .filter_map(|(next_scope, blocks_result)| {
                if blocks_result.is_ok() {
                    Some((next_scope, blocks_result.unwrap()))
                } else {
                    None
                }
            })
            .map(|(next_scope, blocks)| {
                let mut res = vec![];
                for block in blocks.into_iter() {
                    if matches!(block.get_block_type(), BlockType::ModuleOutput)
                        && (block
                            .get_output_nodes()
                            .iter()
                            .any(|node| node.get_text() == sig.get_text())
                        || block
                            .get_input_nodes()
                            .iter()
                            .any(|node| node.get_text() == sig.get_text()))
                    {
                        res.push(block.clone());
                    }
                }
                (next_scope, res)
            })
            .collect::<Vec<_>>();
        Ok(res_in_next_scope_blocks)
    }

    fn get_block_result(&self, scope_name: &str, sig: &NodeID) -> Vec<(String, Vec<T>)> {
        let original_scope = scope_name.to_string();
        let mut scope_name = original_scope.clone();
        let result = loop {
            if let Ok(blocks) = self.get_block(&scope_name, sig) {
                break blocks;
            } else {
                info!(
                    "No blocks found for scope: {}, we will check last level scope further.",
                    scope_name
                );
                if let Some(last_scope) = get_last_scope(&scope_name) {
                    scope_name = last_scope.to_string();
                } else {
                    break vec![];
                }
            }
        };

        if result.is_empty() {
            warn!(
                "No blocks found for sig='{}' in scope='{}' after port_connections search",
                sig.get_text(), original_scope
            );
        }
        result
    }

    /// Fallback search: when normal scope-based block lookup fails due to signal renaming
    /// at module boundaries (common in Chisel-generated Verilog), extract the meaningful
    /// suffix of the signal name and search all scopes for blocks with matching outputs.
    fn find_block_by_signal_suffix(&self, original_scope: &str, sig: &NodeID) -> Vec<(String, Vec<T>)> {
        let sig_text = sig.get_text();
        let suffix = extract_signal_suffix(sig_text);

        if suffix.is_empty() || suffix.len() < 4 {
            // Suffix too short to be meaningful, skip
            return vec![];
        }

        info!(
            "Fallback suffix search: signal='{}', suffix='{}', original_scope='{}'",
            sig_text, suffix, original_scope
        );

        let mut results = Vec::new();
        for scope in self.get_scopes() {
            // Skip the scope we already searched
            if scope == original_scope {
                continue;
            }
            if let Ok(blocks) = self.get_scope_blocks(scope) {
                let matching: Vec<T> = blocks
                    .iter()
                    .filter(|block| {
                        // Only match against non-ModuleOutput blocks (actual logic blocks)
                        // or ModuleOutput blocks in sub-scopes
                        block
                            .get_output_nodes()
                            .iter()
                            .any(|node| node.get_text().ends_with(suffix))
                    })
                    .cloned()
                    .collect();
                if !matching.is_empty() {
                    info!(
                        "Suffix match found: scope='{}', suffix='{}', blocks={}",
                        scope, suffix, matching.len()
                    );
                    results.push((scope.to_string(), matching));
                    // Return the first match to avoid explosion
                    break;
                }
            }
        }
        results
    }

    /// `get_block` to process scope from upper to lower: top -> sub_module by OutputModule Block
    /// `get_driven_signals_in_block` to process scope from lower to upper: sub_module -> top by InputModule Block
    /// 0: next_scope,
    /// 1: timestamp
    /// 2: driven nodes: if it is None, means is sig's local_sig have been visited
    fn get_driven_signals_in_block(
        &mut self,
        block: &T,
        btype: &BlockType,
        sig: NodeID,
        time: Option<TimeAnnotation>,
    ) -> (Option<TimeAnnotation>, Option<Vec<NodeID>>);
    /// return: next_scope, next_vars_with_time, early_stop
    /// if early_stop, then the vars in next_vars will not be traced continually, but themselves will contain in the final trace.
    async fn get_driven_signals_fixpoint(
        &mut self,
        block: &T,
        cur_scope: &str,
        time: Option<TimeAnnotation>,
        sig: &NodeID,
    ) -> Option<(
        String,
        Option<Vec<(NodeID, Option<TimeAnnotation>)>>,
        EarlyStop,
    )> {
        // ModuleOutput blocks may be in a child scope (e.g., ...backend.wbu)
        // while cur_scope is the parent (e.g., ...backend). Only assert for
        // Always/Assign blocks which should always match.
        if matches!(block.get_block_type(), BlockType::Always(_) | BlockType::Assign) {
            let bs = block.get_scope();
            assert!(
                bs == cur_scope || cur_scope.starts_with(&format!("{}.", bs)),
                "block scope '{}' should match or be ancestor of cur_scope '{}'",
                bs, cur_scope
            );
        }
        let btype = block.get_block_type();
        let (next_time, vars) = match btype.clone() {
            BlockType::Always(_) | BlockType::Assign => {
                // reach always fix point:
                // 1. COMB: needs reach fix point
                //  a = b;
                //  c = a;
                //  if we trace `c`, we need to get [`a`, `b`]
                // 2. SEQ: no need fix point
                //  a@t <= b@t-1;
                //  c@t <= a@t-1;
                //  if we trace `c`, only `a`@(t-1) is required; `a`@t is not same with `a`@(t-1)
                let mut res = HashSet::new();
                let (next_time, vars) =
                    self.get_driven_signals_in_block(block, btype, sig.clone(), time);
                if vars.is_none() {
                    (next_time, None)
                } else {
                    let vars = vars.unwrap();
                    res.extend(vars);
                    let mut cross_module_deps: Vec<NodeID> = Vec::new();
                    loop {
                        let len = res.len();
                        let mut tmp = vec![];
                        for v in &res {
                            let (v_next_time, vars) =
                                self.get_driven_signals_in_block(block, btype, v.clone(), time);
                            if let Some(vars) = vars {
                                if v_next_time == time {
                                    tmp.extend(vars);
                                }
                            } else if matches!(btype, BlockType::Always(CircuitType::SEQ)) {
                                // Cross-module fallback for Chisel-generated pipeline registers:
                                // When a SEQ block's driven signal is a cross-module wire
                                // (e.g., _exu_io_out_bits_commits_0), search child scopes for
                                // ModuleOutput blocks that produce this wire.
                                // ModuleOutput.output_nodes = parent-side wire names
                                // ModuleOutput.input_nodes = child-side port names
                                // These cross-module signals go to a separate set — they cannot
                                // be resolved within the current block's fixpoint, but should be
                                // returned for the outer trace loop to resolve via get_block_result.
                                for next_scope in self.get_next_scopes(block.get_scope()) {
                                    if let Ok(sub_blocks) = self.get_scope_blocks(next_scope) {
                                        for b in sub_blocks {
                                            if matches!(b.get_block_type(), BlockType::ModuleOutput)
                                                && b.get_output_nodes().iter()
                                                    .any(|n| n.get_text() == v.get_text())
                                            {
                                                for n in b.get_input_nodes() {
                                                    if let NodeID::Identifier(_, _) = n {
                                                        cross_module_deps.push((*n).clone());
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        res.extend(tmp);
                        if res.len() == len {
                            break;
                        }
                    }
                    let vars: Vec<_> = res.into_iter().collect();
                    // vars.sort_by(|a, b| a.cmp(b));

                    // FIX: After reaching fixpoint, remove vars that contains in left-hand of current block.
                    let mut vars = vars
                        .into_iter()
                        // FIXME: there may be a bug. I found that some vars in fixpoint not exist in output_nodes.
                        .filter(|v| {
                            block
                                .get_output_nodes()
                                .iter()
                                // Here use text compare may remove some right-values.
                                .all(|ov| ov.get_text() != v.get_text())
                        })
                        .collect::<Vec<_>>();
                    // Append cross-module deps from SEQ fallback (resolved via ModuleOutput blocks).
                    // These are child-scope signal names that the outer trace loop should resolve
                    // through get_block_result(), not through fixpoint iteration.
                    if !cross_module_deps.is_empty() {
                        warn!("SEQ cross-module: added {} deps to fixpoint result: {:?}",
                            cross_module_deps.len(),
                            cross_module_deps.iter().map(|n| n.get_text()).collect::<Vec<_>>());
                        vars.extend(cross_module_deps);
                    }
                    (next_time, Some(vars))
                }
            }
            _ => {
                let vars = block
                    .get_input_nodes()
                    .into_iter()
                    .map(|node| (*node).clone())
                    .collect::<Vec<_>>();
                (time, Some(vars))
            }
        };

        // // remove duplicated nodes that appears also in the output nodes to avoid reenter this block again
        // let output_vars = block.get_output_nodes();
        // let vars = vars
        //     .into_iter()
        //     // remove local output that is in the right hand
        //     .filter(|node| {
        //         // Must use get_text to cmp, not cmp node directly, as all node in vars are in right hand
        //         // but output_vars are all in left hand, so NodeID is different.
        //         output_vars
        //             .iter()
        //             .all(|o_node| o_node.get_text() != node.get_text())
        //             || matches!(block.get_block_type(), BlockType::ModuleOutput)
        //     })
        //     .filter(|node| matches!(node, NodeID::Identifier(_, _)))
        //     .into_iter()
        //     .collect();

        let next_vars_with_time = vars.map(|nodes| {
            nodes
                .iter()
                .map(|node| (node.clone(), next_time.clone()))
                .collect::<Vec<_>>()
        });

        let next_scope = if matches!(block.get_block_type(), BlockType::ModuleInput) {
            // ModuleInput: signal goes from parent to child.
            // Use block's scope (where port is declared), not cur_scope (BFS search scope).
            let port_scope = block.get_scope();
            if let Some(last_scope) = get_last_scope(port_scope) {
                last_scope
            } else {
                port_scope
            }
        } else if matches!(block.get_block_type(), BlockType::ModuleOutput) {
            // ModuleOutput: signal goes from child to parent, but the input_nodes
            // are child-internal signals. Continue tracing in the child module's scope.
            block.get_scope()
        } else {
            cur_scope
        };

        // default setting is not early_stop.
        let early_stop = EarlyStop::None;
        Some((next_scope.to_string(), next_vars_with_time, early_stop))
    }

    fn map_str_output_sig_to_output_node(
        &self,
        scope_name: &str,
        sig: &str,
        time: TimeAnnotation,
    ) -> Option<(String, TimeAnnotation, NodeID, T)> {
        // Find candidate nodes
        let mut sig_nodes = self.get_node_id_from_output_port(scope_name, sig).unwrap();
        if sig_nodes.is_empty() {
            warn!(
                "No signal `{}` found in output ports of scope {}. Trying internal vars now.",
                sig, scope_name
            );
            sig_nodes.extend(self.get_node_id_from_internal(scope_name, sig).unwrap());
        }

        let sig_node = sig_nodes.first()?;
        if sig_nodes.len() > 1 {
            warn!(
                "Multiple output signals `{}` in scope {}. Using the first.",
                sig, scope_name
            );
        }

        let blocks = self
            .get_scope_blocks(scope_name)
            .expect("Unknown scope name")
            .into_iter()
            .filter(|block| {
                block
                    .get_input_nodes()
                    .iter()
                    .any(|node| node.get_text() == sig)
                    && matches!(block.get_block_type(), BlockType::ModuleOutput)
            })
            .map(|block| block.clone())
            .collect::<Vec<_>>();

        let block = blocks.first()?.clone();

        Some((scope_name.to_string(), time, sig_node.clone(), block))
    }

    async fn trace(
        &mut self,
        scope_name: &str,
        sig: &str,
        time: Option<TimeAnnotation>,
        time_bound: Option<TimeAnnotation>,
        trace_limit: Option<usize>,
        scope_prefix_filter: Option<Vec<String>>,
    ) -> Vec<(T, Option<TimeAnnotation>)> {
        let mut res: Vec<(NodeID, T, Option<TimeAnnotation>)> = Vec::new();
        let mut sig_que = VecDeque::new();
        let mut sig_nodes = self.get_node_id_from_output_port(scope_name, sig).unwrap();
        if sig_nodes.is_empty() {
            warn!(
                "No signal `{}` found in output ports of scope {}. Try to find internal vars now.",
                sig, scope_name
            );
            sig_nodes.extend(self.get_node_id_from_internal(scope_name, sig).unwrap());
        }
        sig_nodes.iter().for_each(|node| {
            sig_que.push_back((scope_name.to_string(), node.clone(), time));
        });

        let mut ignored_nodes: HashSet<(String, NodeID, Option<TimeAnnotation>)> = HashSet::new();

        while !sig_que.is_empty() {
            if let Some(limit) = trace_limit {
                if res.len() > limit {
                    break;
                }
            }
            // debug!("res len: {}", res.len());
            let (cur_scope, head_sig, cur_time) = sig_que.pop_front().unwrap();
            let key = (cur_scope.clone(), head_sig.clone(), cur_time.clone());
            if ignored_nodes.contains(&key) {
                continue;
            }
            if cur_time.is_some() {
                if time_bound.is_none() {
                    if cur_time.unwrap() < 0 {
                        continue;
                    }
                } else {
                    if cur_time.unwrap() < time_bound.unwrap() {
                        continue;
                    }
                }
            }

            // FIXME: get_block ignored covered block.
            let get_block_result = self.get_block_result(&cur_scope, &head_sig);
            warn!("TRACE: get_block_result for scope={}, sig={}, results={}", cur_scope, head_sig.get_text(),
                get_block_result.iter().map(|(s, blocks)| format!("{}:{}", s, blocks.len())).collect::<Vec<_>>().join(", "));

            for (block_scope, block) in get_block_result
                .iter()
                .map(|(scope, blocks)| blocks.iter().map(|block| (scope.clone(), block)))
                .flatten()
                .collect::<Vec<_>>()
            {
                if let Some(mod_files) = &scope_prefix_filter {
                    if mod_files
                        .iter()
                        .all(|prefix| !block.get_scope().starts_with(prefix))
                    {
                        continue;
                    }
                }

                debug!("head_sig: {}, cur_time: {:?}", head_sig, cur_time);
                let fg = !res.iter().any(|(sig, b, t)| {
                    b.get_bid() == block.get_bid()
                        && *t == cur_time
                        && sig.get_text() == head_sig.get_text()
                });
                if fg {
                    debug!(
                        "block: {}, cur_time: {:?} not visited",
                        block.get_bid(),
                        cur_time
                    );
                    warn!("TRACE: block bid={} block.scope={} passed_scope={}", block.get_bid(), block.get_scope(), cur_scope);
                    if let Some((next_scope, Some(next_vars_with_time), early_stop)) = self
                        .get_driven_signals_fixpoint(block, &cur_scope, cur_time, &head_sig)
                        .await
                    {
                        next_vars_with_time.iter().for_each(|(node, next_time)| {
                            debug!(
                                "line: {}, name: {}, time: {:?}",
                                node.get_locate().line,
                                node.get_text(),
                                next_time
                            )
                        });
                        match early_stop {
                            EarlyStop::Module(bound_nodes) => {
                                // this module is suspicious, but the internal module should be checked.
                                // but the trace on `head_sig` should be ended at this module's input ports.
                                ignored_nodes.extend(bound_nodes);
                                next_vars_with_time.iter().for_each(|(node, next_time)| {
                                    sig_que.push_back((
                                        next_scope.clone(),
                                        node.clone(),
                                        next_time.clone(),
                                    ))
                                });
                            }
                            EarlyStop::Block => {
                                // this response is termination, so the next input variables for this block will be ignored.
                            }
                            EarlyStop::None => {
                                next_vars_with_time.iter().for_each(|(node, next_time)| {
                                    sig_que.push_back((
                                        next_scope.clone(),
                                        node.clone(),
                                        next_time.clone(),
                                    ))
                                });
                            }
                        }
                        let mut block_new = block.clone();
                        block_new.add_suspicious_trace(head_sig.clone(), next_vars_with_time);
                        debug!(
                            "block: {}, cur_time: {:?} append and visited",
                            block_new.get_bid(),
                            cur_time
                        );
                        res.push((head_sig.clone(), block_new, cur_time));
                    }
                } else {
                    debug!(
                        "block: {}, cur_time: {:?} visited",
                        block.get_bid(),
                        cur_time
                    );
                }
            }
        }

        // remove redundant (b, t)
        let mut unique_res: Vec<(T, Option<TimeAnnotation>)> = vec![];
        res.into_iter().for_each(|(_node, block, time)| {
            if !unique_res
                .iter()
                .any(|(b, t)| block.get_bid() == b.get_bid() && *t == time)
            {
                unique_res.push((block, time));
            }
        });
        unique_res
    }
}
