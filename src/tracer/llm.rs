use crate::agent::block::block_checker::BlockChecker;
use crate::agent::block::block_reranker::BlockReranker;
use crate::agent::token_price;
use crate::block::Block;
use crate::dataflow::NodeID;
use crate::localizer::Localizer;
use crate::slice::dynamic_slice::DynamicSlicing;
use crate::tracer::{EarlyStop, TimeAnnotation};
use crate::{
    top_k_items, BlockManager, BlockParser, BlockType, BugIDType, CoverageTracker,
    LocalizationChoiceBuilder, LocalizationResult, LocalizationResultBuilder, ModuleChecker,
    Tracer,
};
use async_trait::async_trait;
use futures::executor::block_on;
use log::{debug, error, info, trace, warn};
use rig::completion::Usage;
use std::collections::HashSet;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// # Step1: use llm to reduce dynamic tracing
///     - make sure the buggy code is included with high probability
///     - tracing number is relatively low
/// - [x] mod_checker
/// - [ ] case | if | ?
/// # Step2: use llm to summarize from tracing
///     - get top-k suspicious blocks
/// # Both Steps share:
///     - module knowledge augmentation
pub struct LLMAidTracer<'a, 'b, Parser, CT, MC, BC, BR, T>
where
    Parser: BlockParser,
    CT: CoverageTracker + Send,
    MC: ModuleChecker + Send,
    BC: BlockChecker<'a, Parser::Block<'a>> + Send,
    BR: BlockReranker<'a, Parser::Block<'a>> + Send,
{
    bug_id: BugIDType,
    model_name: String,
    total_token_cost: Arc<Mutex<Usage>>,
    dynamic_tracer: DynamicSlicing<'a, 'b, Parser, CT>,
    module_checker: MC,
    block_checker: BC,
    block_reranker: BR,
    // block_manager: &'b BlockManager<'a, Parser>,
    time_bound: Option<TimeAnnotation>,
    trace_limit: Option<usize>,
    suspicious_modules: Vec<((NodeID, Option<TimeAnnotation>), String)>,
    suspicious_blocks: Vec<((NodeID, Option<TimeAnnotation>), T)>,
    /// When llm recognize a suspicious module, it will first mark  suspicious blocks using this.
    /// Then when llm tracing over Assign/Always blocks, it will first to check whether it is enabled by this.
    /// If yes, llm will to check this block whether suspicious and add it to `suspicious_blocks`.
    block_checker_enable: HashSet<(u64, TimeAnnotation)>,
    early_stop: bool,
    // modules_knowledge: HashMap<String, String>,
    test_info: String,
    vote_top_k: usize,
    vote_total: usize,
}

impl<'a, 'b, Parser, CT, MC, BC, BR, T> LLMAidTracer<'a, 'b, Parser, CT, MC, BC, BR, T>
where
    Parser: BlockParser<Block<'a> = T>,
    CT: CoverageTracker + Send,
    MC: ModuleChecker + Send,
    BC: BlockChecker<'a, Parser::Block<'a>> + Send,
    BR: BlockReranker<'a, Parser::Block<'a>> + Send,
    T: Block<'a> + Sync + Send + Debug + 'static,
{
    pub fn new<I: ToString>(
        bug_id: BugIDType,
        model_name: &str,
        test_info: I,
        time_step: TimeAnnotation,
        time_bound: Option<TimeAnnotation>,
        trace_limit: Option<usize>,
        block_manager: &'b BlockManager<'a, Parser>,
        coverage_tracker: CT,
        module_checker: MC,
        block_checker: BC,
        block_reranker: BR,
        early_stop: bool,
        vote_top_k: usize,
        vote_total: usize,
        total_token_cost: Arc<Mutex<Usage>>,
    ) -> Self {
        let dynamic_tracer = DynamicSlicing::new(time_step, block_manager, coverage_tracker);
        Self {
            bug_id,
            model_name: model_name.to_string(),
            total_token_cost,
            dynamic_tracer,
            module_checker,
            block_checker,
            block_reranker,
            // block_manager,
            time_bound,
            trace_limit,
            suspicious_modules: Vec::new(),
            suspicious_blocks: Vec::new(),
            block_checker_enable: HashSet::new(),
            early_stop,
            // modules_knowledge: HashMap::from([("".to_string(), r#""#.to_string())]),
            test_info: test_info.to_string(),
            vote_top_k,
            vote_total,
        }
    }

    fn add_suspicious_module(
        &mut self,
        suspicious_node: (NodeID, Option<TimeAnnotation>),
        module_name: String,
    ) {
        if self
            .suspicious_modules
            .iter()
            .any(|(v, m)| *v == suspicious_node && *m == module_name)
        {
            return;
        }
        self.suspicious_modules.push((suspicious_node, module_name));
    }

    fn add_suspicious_block(
        &mut self,
        suspicious_node: (NodeID, Option<TimeAnnotation>),
        block: T,
    ) {
        if self
            .suspicious_blocks
            .iter()
            .any(|(v, m)| *v == suspicious_node && m.get_bid() == block.get_bid())
        {
            return;
        }
        debug!(
            "add suspicious block {} to `self.suspicious_blocks`",
            block.get_bid()
        );
        self.suspicious_blocks.push((suspicious_node, block));
    }

    pub async fn rerank(&mut self) -> Vec<(((NodeID, Option<TimeAnnotation>), T), f64)> {
        let create_unranked = || {
            self.suspicious_blocks
                .clone()
                .into_iter()
                .map(|data| (data, 1.0))
                .collect()
        };

        if self.suspicious_blocks.len() < 2 {
            warn!(
                "Suspicious block len={}, so ignore rerank.",
                self.suspicious_blocks.len()
            );
            return create_unranked();
        }

        match self
            .block_reranker
            .rerank(self.suspicious_blocks.as_ref(), &self.test_info)
            .await
        {
            Ok(reranked_blocks) => reranked_blocks,
            Err(err) => {
                error!("Error when reranking: {}", err);
                create_unranked()
            }
        }
    }
}

impl<'a, 'b, T, Parser, CT, MC, BC, BR> LLMAidTracer<'a, 'b, Parser, CT, MC, BC, BR, T>
where
    T: Block<'a> + Sync + Send + Debug + 'static,
    Parser: BlockParser<Block<'a> = T>,
    CT: CoverageTracker + Send + Sync,
    MC: ModuleChecker + Send + 'static,
    BC: BlockChecker<'a, Parser::Block<'a>> + Send + 'static,
    BR: BlockReranker<'a, Parser::Block<'a>> + Send + 'static,
{
    async fn llm_voting_for_ports(
        &self,
        block: &T,
        port_blocks: &Vec<(T, TimeAnnotation)>,
        local_sig: &NodeID,
        time: Option<TimeAnnotation>,
    ) -> Option<Vec<(NodeID, TimeAnnotation)>> {
        let appendix_info = format!(
            r#"
                    This module is called `{}` from a RISCV core written in SystemVerilog.
                    {}
                    "#,
            block.get_module_name(),
            self.test_info
        );

        let total_count = self.vote_total as f64;
        let mut not_dive_count = 0.0;

        let mut vars_set = vec![];

        let (tx, mut rx) = mpsc::channel(total_count as usize);

        for i in 0..(total_count as usize) {
            let mut mc_checker = self.module_checker.clone();
            let tx_clone = tx.clone();
            let port_blocks = port_blocks.clone();
            let appendix_info = appendix_info.clone();
            let sig_name = local_sig.get_text().to_string();
            let sig_clone = local_sig.clone();
            let module_name = block.get_module_name().to_string();
            tokio::spawn(async move {
                let response = mc_checker
                    .determine(&port_blocks, sig_clone, time.unwrap(), &appendix_info)
                    .await;
                info!(
                    "module_checker for local_sig={} at module={}, i={} response={:?}",
                    sig_name, module_name, i, response
                );
                if let Err(err) = tx_clone.send(response).await {
                    error!("module_checker response error: {:?}", err);
                }
            });
        }

        drop(tx);

        while let Some(response) = rx.recv().await {
            if let Ok(response) = response {
                if let Some(vars) = response {
                    not_dive_count += 1.0;
                    // next_time = Some(t);
                    vars_set.push(vars);
                }
            };
        }

        let suspicious_vars = if not_dive_count / total_count > 0.5 {
            let vars = vars_set.into_iter().flatten().collect::<Vec<_>>();

            // W.I.P: I'm not sure whether need to remove duplicated nodes that is earlier than more later ones.
            let mut unique_vars = vec![];
            for v in vars {
                if unique_vars
                    .iter()
                    .any(|(key, time)| *key == v.0 && v.1 < *time)
                {
                    continue;
                }
                unique_vars.push(v);
            }

            let vars = top_k_items(unique_vars, self.vote_top_k);
            let ret = vars
                .into_iter()
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            (!ret.is_empty()).then_some(ret)
        } else {
            None
        };
        suspicious_vars
    }

    async fn llm_voting_for_blocks(
        &mut self,
        block: &T,
        port_nodes: &Vec<(NodeID, TimeAnnotation)>,
        df_nodes: &Vec<(NodeID, Option<TimeAnnotation>)>,
        local_sig: &NodeID,
        time: Option<TimeAnnotation>,
    ) -> Option<Vec<(NodeID, TimeAnnotation)>> {
        let appendix_info = format!(
            r#"
            Current module is `{}` from a RISCV core written in SystemVerilog.
            {}
            "#,
            block.get_module_name(),
            self.test_info
        );

        let module_knowledge = "";
        // self
        // .modules_knowledge
        // .get(&block.get_module_name().to_string())
        // .cloned()
        // .unwrap_or_default();

        let total_count = self.vote_total as f64;
        let mut not_dive_count = 0.0;
        let mut suspicious_count = 0.0;
        let mut terminate_count = 0.0;

        let mut vars_set = vec![];

        let (tx, mut rx) = mpsc::channel(total_count as usize);

        let df_nodes = df_nodes
            .into_iter()
            .map(|(node, t)| (node.clone(), t.unwrap()))
            .collect::<Vec<_>>();

        for i in 0..(total_count as usize) {
            let mut bc_checker = self.block_checker.clone();
            let block = block.clone();
            let tx_clone = tx.clone();
            let df_nodes = df_nodes.clone();
            let port_nodes = port_nodes.clone();
            let appendix_info = appendix_info.clone();
            let sig_name = local_sig.get_text().to_string();
            let sig_clone = local_sig.clone();
            let module_name = block.get_module_name().to_string();
            let module_knowledge = module_knowledge.to_string();
            let historical_suspicious_blocks = self
                .suspicious_blocks
                .iter()
                .map(|(_, b)| b.clone())
                .collect();
            tokio::spawn(async move {
                warn!("LLM vote task {} starting for sig={}", i, sig_name);
                let result = bc_checker
                    .determine(
                        &block,
                        &port_nodes,
                        &df_nodes,
                        sig_clone,
                        time.unwrap(),
                        &appendix_info,
                        &module_knowledge,
                        &historical_suspicious_blocks,
                    )
                    .await;
                warn!("LLM vote task {} got response: {:?}", i, result);
                if let Err(err) = tx_clone.send(result).await {
                    error!("block_checker response error: {:?}", err);
                }
            });
        }

        drop(tx);

        while let Some(response) = rx.recv().await {
            if let Ok((vars, suspicious, terminate)) = response {
                warn!("LLM vote response: vars={:?}, suspicious={}, terminate={}", vars.as_ref().map(|v| v.len()), suspicious, terminate);
                if let Some(vars) = vars {
                    not_dive_count += 1.0;
                    // next_time = Some(t);
                    vars_set.push(vars);
                }
                if suspicious {
                    suspicious_count += 1.0;
                }
                if terminate {
                    terminate_count += 1.0;
                }
            } else {
                warn!("LLM vote response ERROR: {:?}", response);
            };
        }
        warn!(
            "LLM voting result: not_dive={}, suspicious={}, terminate={}, total={}",
            not_dive_count, suspicious_count, terminate_count, total_count
        );

        if suspicious_count / total_count >= 0.5 {
            self.add_suspicious_block((local_sig.clone(), time), block.clone());
        }

        if terminate_count / total_count > 0.5 {
            return None;
        }

        let suspicious_vars = {
            let vars = vars_set.into_iter().flatten().collect::<Vec<_>>();

            // W.I.P: I'm not sure whether need to remove duplicated nodes that is earlier than more later ones.
            let mut unique_vars = vec![];
            for v in vars {
                if unique_vars
                    .iter()
                    .any(|(key, time)| *key == v.0 && v.1 < *time)
                {
                    continue;
                }
                unique_vars.push(v);
            }

            let vars = top_k_items(unique_vars, self.vote_top_k);
            let ret = vars
                .into_iter()
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            (!ret.is_empty()).then_some(ret)
        };
        suspicious_vars
    }

    async fn llm_request_for_ports(
        &mut self,
        cur_scope: &str,
        block: &T,
        port_blocks: &Vec<(T, TimeAnnotation)>,
        sig: &NodeID,
        local_sig: &NodeID,
        time: Option<TimeAnnotation>,
        local_trace: Vec<(T, TimeAnnotation)>,
    ) -> Option<(
        String,
        Option<Vec<(NodeID, Option<TimeAnnotation>)>>,
        EarlyStop,
    )> {
        let next_scope = cur_scope;
        let suspicious_vars = self
            .llm_voting_for_ports(block, port_blocks, local_sig, time)
            .await;
        // let suspicious_vars: Option<Vec<(NodeID, TimeAnnotation)>> = None;

        if suspicious_vars.is_none() {
            // dive
            trace!(
                "[DIVE] module {}, suspicious vars: {:?}@{:?}",
                block.get_module_name(),
                sig,
                time
            );
            let res = self
                .dynamic_tracer
                .get_driven_signals_fixpoint(block, cur_scope, time, sig)
                .await;
            let local_trace = local_trace
                .iter()
                .map(|(b, t)| (b.get_bid(), t.clone()))
                .collect::<Vec<_>>();
            self.block_checker_enable.extend(local_trace);
            self.add_suspicious_module((sig.clone(), time), block.get_module_name().to_string());
            let early_stop = if self.early_stop {
                // set ignore bound. when tracer meets these nodes, it will ignore expansion for them.
                let ignored_nodes: Vec<(String, NodeID, Option<TimeAnnotation>)> = port_blocks
                    .iter()
                    .map(|(b, t)| {
                        (
                            cur_scope.to_string(),
                            (*b.get_output_nodes().iter().collect::<Vec<_>>()[0]).clone(),
                            Some(*t),
                        )
                    })
                    .collect();
                EarlyStop::Module(ignored_nodes)
            } else {
                EarlyStop::None
            };
            res.map(|(next_scope, next_vars, _early_stop)| (next_scope, next_vars, early_stop))
        } else {
            // not dive
            trace!(
                "[NOT DIVE] module {}, suspicious vars: {:?}@{:?} -> {:?}",
                block.get_module_name(),
                sig,
                time,
                suspicious_vars
            );
            let suspicious_vars = suspicious_vars.map(|vars| {
                vars.into_iter()
                    .map(|(node, t)| (node, Some(t)))
                    .collect::<Vec<_>>()
            });
            Some((next_scope.to_string(), suspicious_vars, EarlyStop::None))
        }
    }

    async fn llm_request_for_blocks(
        &mut self,
        cur_scope: &str,
        block: &T,
        // next_scope comes from dyn trace, it will help llm to return next_scope, the first return arg
        next_scope: &str,
        port_nodes: &Vec<(NodeID, TimeAnnotation)>,
        df_nodes: &Vec<(NodeID, Option<TimeAnnotation>)>,
        sig: &NodeID,
        time: Option<TimeAnnotation>,
    ) -> Option<(
        String,
        Option<Vec<(NodeID, Option<TimeAnnotation>)>>,
        EarlyStop,
    )> {
        // for Assign/Always block, local_sig has the same name with extern sig.
        let local_sig = sig;
        let suspicious_vars = self
            .llm_voting_for_blocks(block, port_nodes, df_nodes, local_sig, time)
            .await;
        // let suspicious_vars: Option<Vec<(NodeID, TimeAnnotation)>> = None;

        let res = self
            .dynamic_tracer
            .get_driven_signals_fixpoint(block, cur_scope, time, sig)
            .await;

        if suspicious_vars.is_none() {
            // Paper alignment (Section 3.4): Blues constructs the full
            // instruction execution path via pure dataflow BFS (Algorithm 1).
            // The LLM's terminate decision only means "done inspecting this
            // block" — it does NOT stop the path construction. Always continue
            // dataflow tracing so the LLM can inspect deeper blocks (e.g.,
            // tracing from fetch → ALU as in paper Figure 6).
            res
        } else {
            // not dive
            trace!(
                "[NOT DIVE] block {} in module {}, suspicious vars: {:?}@{:?} -> {:?}",
                block.get_bid(),
                block.get_module_name(),
                sig,
                time,
                suspicious_vars
            );
            let suspicious_vars = suspicious_vars.map(|vars| {
                vars.into_iter()
                    .map(|(node, t)| (node, Some(t)))
                    .collect::<Vec<_>>()
            });

            // When LLM returns variables that are ModuleInput ports of the current scope,
            // map them through port_connections to parent scope signals. This enables
            // cross-module tracing (e.g., WBU input → Backend pipeline register → EXU output).
            let mapped_vars = suspicious_vars.as_ref().and_then(|vars| {
                warn!("LLM: checking ModuleInput mapping for {} vars in scope={}", vars.len(), next_scope);
                let scope_blocks = match self.get_scope_blocks(cur_scope) {
                    Ok(b) => b,
                    Err(e) => { warn!("LLM: get_scope_blocks failed: {}", e); return None; }
                };
                let input_blocks: Vec<_> = scope_blocks
                    .iter()
                    .filter(|b| matches!(b.get_block_type(), BlockType::ModuleInput))
                    .collect();
                warn!("LLM: found {} ModuleInput blocks in scope {}", input_blocks.len(), cur_scope);
                if input_blocks.is_empty() {
                    return None;
                }
                // Debug: log first few input block output names
                for (i, ib) in input_blocks.iter().take(5).enumerate() {
                    let out_names: Vec<&str> = ib.get_output_nodes().iter().take(3).map(|n| n.get_text()).collect();
                    warn!("LLM: ModuleInput block[{}] bid={} outputs={:?}", i, ib.get_bid(), out_names);
                }
                let mut mapped = Vec::new();
                let mut any_mapped = false;
                for (node, t) in vars {
                    let node_text = node.get_text();
                    let mut found = false;
                    for ib in &input_blocks {
                        // Check if this ModuleInput block's output node matches the LLM signal
                        if ib.get_output_nodes().iter().any(|on| on.get_text() == node_text) {
                            // Map through dataflow: ModuleInput output → parent scope input signals
                            let parent_vars: Vec<_> = ib.get_input_nodes()
                                .into_iter()
                                .cloned()
                                .map(|pn| (pn, t.clone()))
                                .collect();
                            if !parent_vars.is_empty() {
                                mapped.extend(parent_vars);
                                found = true;
                                any_mapped = true;
                            }
                        }
                    }
                    if !found {
                        mapped.push((node.clone(), t.clone()));
                    }
                }
                if any_mapped {
                    // Jump to parent scope for the mapped signals
                    let parent_scope = crate::tracer::get_last_scope(next_scope)
                        .unwrap_or(next_scope)
                        .to_string();
                    warn!(
                        "LLM: mapped {} vars through ModuleInput ports, jumping to parent scope={}",
                        mapped.len(), parent_scope
                    );
                    Some((parent_scope, mapped))
                } else {
                    None
                }
            });

            if let Some((parent_scope, mapped_vars)) = mapped_vars {
                Some((parent_scope, Some(mapped_vars), EarlyStop::None))
            } else {
                Some((next_scope.to_string(), suspicious_vars, EarlyStop::None))
            }
        }
    }

    async fn get_local_trace(
        &mut self,
        block: &T,
        local_sig: &NodeID,
        time: Option<TimeAnnotation>,
    ) -> Vec<(T, Option<TimeAnnotation>)> {
        // We only consider the input ports that have dataflow to `sig`. static slicing here
        // restrict modules under current scope
        let sub_scopes = vec![block.get_scope().to_string()];

        let local_trace = {
            // pos_visited used by this tracer should be split with the following one.
            let old = self.dynamic_tracer.pos_visited.clone();
            // self.dynamic_tracer.pos_visited = HashMap::new();
            let ret = self
                .dynamic_tracer
                .trace(
                    block.get_scope(),
                    local_sig.get_text(),
                    time,
                    self.time_bound,
                    self.trace_limit,
                    Some(sub_scopes),
                )
                .await;
            self.dynamic_tracer.pos_visited = old;
            ret
        };
        local_trace
    }

    /// Shallow trace for ModuleChecker: finds input port blocks with signal values
    /// without deep expansion. Uses a small trace_limit to avoid CSR perfCnts explosion.
    async fn get_local_trace_with_limit(
        &mut self,
        block: &T,
        local_sig: &NodeID,
        time: Option<TimeAnnotation>,
        limit: Option<usize>,
    ) -> Vec<(T, Option<TimeAnnotation>)> {
        let sub_scopes = vec![block.get_scope().to_string()];
        let old = self.dynamic_tracer.pos_visited.clone();
        let ret = self
            .dynamic_tracer
            .trace(
                block.get_scope(),
                local_sig.get_text(),
                time,
                self.time_bound,
                limit,  // shallow limit for ModuleChecker
                Some(sub_scopes),
            )
            .await;
        self.dynamic_tracer.pos_visited = old;
        ret
    }

    /// Lightweight port_blocks builder for ModuleChecker.
    /// Gets ModuleInput blocks directly from block_manager without running a full trace.
    /// This enables ModuleChecker to decide dive/not_dive BEFORE expensive expansion.
    fn build_port_blocks_for_module_check(
        &self,
        block: &T,
        time: Option<TimeAnnotation>,
    ) -> Vec<(T, TimeAnnotation)> {
        self.dynamic_tracer
            .block_manager
            .get_port_blocks(block.get_scope())
            .into_iter()
            .filter(|b| matches!(b.get_block_type(), BlockType::ModuleInput))
            .map(|b| (b, time.unwrap()))
            .collect()
    }

    fn filter_input_ports_from_trace(
        &self,
        cur_scope: &str,
        local_trace: &Vec<(T, Option<TimeAnnotation>)>,
    ) -> Vec<(T, TimeAnnotation)> {
        let visited_ports = local_trace
            .iter()
            .filter(|(block, _)| matches!(block.get_block_type(), BlockType::ModuleInput))
            .map(|(b, t)| (b.clone(), t.unwrap()))
            .collect::<Vec<_>>();

        let port_blocks = self
            .dynamic_tracer
            .block_manager
            .get_port_blocks(cur_scope)
            .iter()
            .map(|block| {
                visited_ports
                    .iter()
                    .filter(|(vb, _)| vb.get_bid() == block.get_bid())
                    .collect::<Vec<_>>()
            })
            .flatten()
            .map(|(block, time)| (block.clone(), time.clone()))
            .collect::<Vec<_>>();
        port_blocks
    }
}

#[async_trait(?Send)]
impl<'a, 'b, T, Parser, CT, MC, BC, BR> Tracer<'a, T>
    for LLMAidTracer<'a, 'b, Parser, CT, MC, BC, BR, T>
where
    T: Block<'a> + Sync + Send + Debug + 'static,
    Parser: BlockParser<Block<'a> = T>,
    CT: CoverageTracker + Send + Sync,
    MC: ModuleChecker + Send + 'static,
    BC: BlockChecker<'a, Parser::Block<'a>> + Send + 'static,
    BR: BlockReranker<'a, Parser::Block<'a>> + Send + 'static,
{
    fn get_scope_blocks(&self, scope_name: &str) -> anyhow::Result<&[T]> {
        self.dynamic_tracer.get_scope_blocks(scope_name)
    }

    fn get_scopes(&self) -> Vec<&str> {
        self.dynamic_tracer.get_scopes()
    }
    fn get_block_covered_ast_lines(&self, _block: &T, _time: TimeAnnotation) -> Vec<(u32, usize)> {
        todo!("Not implemented")
    }
    fn get_driven_signals_in_block(
        &mut self,
        _block: &T,
        _btype: &BlockType,
        _sig: NodeID,
        _time: Option<TimeAnnotation>,
    ) -> (Option<TimeAnnotation>, Option<Vec<NodeID>>) {
        todo!()
    }

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
        // common knowledge
        // 1. module knowledge
        // 2. signal values in the block
        // let next_time = time;

        // Note the sig is an extern variable, so first map sig to local_sig

        match block.get_block_type() {
            BlockType::ModuleOutput => {
                // Paper-aligned (Algorithm 1): Blues traces backward through dataflow
                // without ModuleChecker. Coverage filtering happens in IntraBlockAnalysis.
                // The LLM navigates the resulting instruction execution path via tool-calls.
                info!("[TRACE] ModuleOutput: module={}, sig={}, time={:?} — tracing dataflow",
                    block.get_module_name(), sig.get_text(), time);
                self.dynamic_tracer
                    .get_driven_signals_fixpoint(block, cur_scope, time, sig)
                    .await
            }
            BlockType::Assign | BlockType::Always(_)
            // Add guard, block checker only enabled when mod_checker mark the bid in set.
            // if {
            //     self.block_checker_enable
            //         .contains(&(block.get_bid(), time.unwrap()))
            // }
            =>
                {
                    warn!("LLM get_driven_signals_fixpoint: block {} sig={} module={}", block.get_bid(), sig.get_text(), block.get_module_name());
                    // if | case
                    // [input]
                    // 1. sig -> local_sig -> dataflow[predicates, right values]
                    // 2. instruction info + module knowledge
                    // [output]
                    // if the conditions are incorrect for this instruction, we will include these predicates + right-values.
                    // else we will include right-values.

                    let (next_scope, next_vars, _) = {
                        // pos_visited used by this tracer should be split with the following one.
                        let old = self.dynamic_tracer.pos_visited.clone();
                        // self.dynamic_tracer.pos_visited = HashMap::new();
                        let ret = self
                            .dynamic_tracer
                            .get_driven_signals_fixpoint(
                                block,
                                // trace start from an internal signal
                                block.get_scope(),
                                time,
                                sig,
                            )
                            .await;
                        self.dynamic_tracer.pos_visited = old;
                        ret
                    }?;
                    warn!("LLM: fixpoint returned scope={}, vars={:?}", next_scope, next_vars.as_ref().map(|v| v.len()));
                    let sig_dataflow_vars = next_vars?
                        // FIXME: temp remove constants
                        .into_iter().filter(|(var, _)| !var.get_text().chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase()))
                        .collect::<Vec<_>>();
                    warn!("LLM: sig_dataflow_vars count={}", sig_dataflow_vars.len());
                    warn!("LLM: sig_dataflow_vars names={:?}", sig_dataflow_vars.iter().map(|(n, t)| (n.get_text(), t)).collect::<Vec<_>>());

                    let local_trace = self.get_local_trace(block, sig, time).await;
                    warn!("LLM: local_trace len={}", local_trace.len());
                    let port_blocks = self.filter_input_ports_from_trace(cur_scope, &local_trace)
                        .iter()
                        .map(|(b, t)| {
                            b.get_output_nodes().into_iter().map(|n| (n.clone(), t.clone()))
                        })
                        .flatten()
                        .collect::<Vec<_>>();
                    if port_blocks.is_empty() {
                        // maybe this output port is assigned by a constant.
                        warn!(
                        "No port blocks found for scope {}, block {:?}",
                        cur_scope, block
                    );
                    }

                    warn!("LLM: calling llm_request_for_blocks with port_blocks={}, df_vars={}", port_blocks.len(), sig_dataflow_vars.len());

                    let ret = self.llm_request_for_blocks(
                        cur_scope,
                        block,
                        &next_scope,
                        &port_blocks,
                        &sig_dataflow_vars,
                        sig,
                        time,
                    ).await;

                    info!("LLM's block_checker decision for [sig={}, module={}, time={:?}] is {:?}", sig.get_text(), block.get_module_name(), time, ret);
                    ret
                }
            _ => {
                let res = self
                    .dynamic_tracer
                    .get_driven_signals_fixpoint(block, cur_scope, time, sig)
                    .await;
                res
            }
        }
    }
}

impl<'a, 'b, T, Parser, CT, MC, BC, BR> Localizer<'a, T>
    for LLMAidTracer<'a, 'b, Parser, CT, MC, BC, BR, T>
where
    T: Block<'a> + Sync + Send + Debug + 'static,
    Parser: BlockParser<Block<'a> = T>,
    CT: CoverageTracker + Send + Sync,
    MC: ModuleChecker + Send + 'static,
    BC: BlockChecker<'a, Parser::Block<'a>> + Send + 'static,
    BR: BlockReranker<'a, Parser::Block<'a>> + Send + 'static,
{
    fn get_bug_id(&self) -> BugIDType {
        self.bug_id.clone()
    }

    fn get_localized_modules(&self) -> Vec<(String, Option<(NodeID, Option<TimeAnnotation>)>)> {
        self.suspicious_modules
            .clone()
            .into_iter()
            .map(|(signal, name)| (name, Some(signal)))
            .collect()
    }

    fn get_localized_blocks(&self) -> Vec<(T, Option<(NodeID, Option<TimeAnnotation>)>)> {
        self.suspicious_blocks
            .clone()
            .into_iter()
            .map(|(signal, name)| (name, Some(signal)))
            .collect()
    }

    fn get_localization_results(&mut self) -> LocalizationResult {
        let reranked_blocks = block_on(self.rerank());

        let (token_usage, token_price) = {
            let usage_lock = self.total_token_cost.lock().unwrap();
            (
                usage_lock.clone(),
                token_price(&usage_lock, &self.model_name),
            )
        };
        println!(
            "Total Token Usage = \n{:#?} \nToken Price = ${}",
            token_usage,
            token_price.unwrap_or(0.)
        );

        let choices = reranked_blocks
            .clone()
            .into_iter()
            .map(|(((_sig, _time), block), score)| {
                LocalizationChoiceBuilder::default()
                    .score(score)
                    .block_id(block.get_bid())
                    .module_name(block.get_module_name().to_string())
                    .build()
                    .unwrap()
            })
            .collect::<Vec<_>>();

        LocalizationResultBuilder::default()
            .bug_id(self.bug_id.clone())
            .token_usage(token_usage)
            .token_price(token_price)
            .choices(choices)
            .build()
            .unwrap()
    }
}
