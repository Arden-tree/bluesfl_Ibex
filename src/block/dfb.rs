use crate::block::utils::{
    get_dut_name_from_instantiation, get_identifier_str, get_module_name_from_instantiation,
    get_port_connections, get_port_direction, get_ref_node_code_str, merge_all,
};
use crate::block::{
    AstRange, Block, BlockParser, BlockType, CircuitType, CoverState, ScopeBlocks, SuspiciousTrace,
};
use crate::dataflow::{DataFlowAnalyzer, NodeID};
use crate::{
    get_module_name, get_node_ast_range, get_pos_from_offset, ParameterCoverageReport,
    TimeAnnotation,
};
use derive_builder::Builder;
use log::warn;
use serde::Serialize;
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use sv_parser::{
    unwrap_node, AlwaysConstruct, AnsiPortDeclarationNet, Locate, NetAssignment, NodeEvent,
    PortDirection, RefNode, SyntaxTree,
};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DataFlowBlock {
    bid: u64,
    module_name: String,
    scope: String,
    block_type: BlockType,
    nodes_dataflow: HashMap<NodeID, Vec<NodeID>>,
    pub(crate) sig_decls: HashSet<String>,
    pub(crate) inputs: HashSet<NodeID>,
    pub(crate) outputs: HashSet<NodeID>,
    pub(crate) suspicious_variables: HashSet<NodeID>,
    pub(crate) suspicious_trace: Option<SuspiciousTrace>,
    pub(crate) ctx: Vec<String>,
    ast_ranges: Vec<AstRange>,
}

impl<'a> Block<'a> for DataFlowBlock {
    fn get_block_type(&self) -> &BlockType {
        &self.block_type
    }

    fn get_bid(&self) -> u64 {
        self.bid
    }

    fn get_module_name(&self) -> &str {
        &self.module_name
    }

    fn get_scope(&self) -> &str {
        &self.scope
    }

    fn get_input_nodes(&self) -> HashSet<&NodeID> {
        self.inputs.iter().collect()
    }

    fn get_output_nodes(&self) -> HashSet<&NodeID> {
        self.outputs.iter().collect()
    }

    fn get_node_dataflow(&self, node: NodeID) -> HashSet<NodeID> {
        self.nodes_dataflow
            .get(&node)
            .into_iter()
            .map(|id| id.clone())
            .flatten()
            .collect()
    }

    fn get_suspicious_trace(&self) -> Option<&SuspiciousTrace> {
        self.suspicious_trace.as_ref()
    }

    fn get_ctx(&self) -> Vec<&str> {
        self.ctx.iter().map(|x| x.as_str()).collect()
    }

    fn get_signal_decls(&self) -> Vec<&str> {
        self.sig_decls.iter().map(|x| x.as_str()).collect()
    }

    fn add_input_node(&mut self, node: NodeID) {
        self.inputs.insert(node);
    }

    fn add_output_node(&mut self, node: NodeID) {
        self.outputs.insert(node);
    }

    fn add_ctx(&mut self, ctx: &str) {
        self.ctx.push(ctx.to_string());
    }

    fn add_signal_decl(&mut self, signal_decl: &str) {
        self.sig_decls.insert(signal_decl.to_string());
    }

    fn add_suspicious_trace(&mut self, node: NodeID, vars: Vec<(NodeID, Option<TimeAnnotation>)>) {
        self.suspicious_trace = Some((node, vars));
    }

    fn get_ast_covered_ranges(&self) -> Vec<AstRange> {
        self.ast_ranges.clone()
    }
}

impl DataFlowBlock {
    pub fn new(bid: u64, module_name: &str, scope: &str, btype: BlockType) -> DataFlowBlock {
        let suspicious_variables = HashSet::new();
        // TODO: remove this
        // suspicious_variables.insert("adder_result_o".to_string());
        Self {
            bid,
            module_name: module_name.to_string(),
            scope: scope.to_string(),
            block_type: btype,
            nodes_dataflow: HashMap::new(),
            sig_decls: HashSet::new(),
            inputs: HashSet::new(),
            outputs: HashSet::new(),
            suspicious_variables,
            suspicious_trace: None,
            ctx: vec![],
            ast_ranges: vec![],
        }
    }
}

#[derive(Builder)]
pub struct DataFlowBlockParser {
    #[builder(default)]
    max_bid: Cell<u64>,
    #[builder(setter(into, strip_option), default)]
    param_coverage_tracker: Option<ParameterCoverageReport>,
}

fn merge_two(
    bid: u64,
    module_name: &str,
    t: BlockType,
    b1: &DataFlowBlock,
    b2: &DataFlowBlock,
) -> DataFlowBlock {
    assert_eq!(b1.module_name, b2.module_name);
    assert_eq!(b1.scope, b2.scope);
    assert_eq!(b1.block_type, b2.block_type);
    assert_eq!(b1.block_type, t);
    let scope = b1.get_scope();
    let mut blk = DataFlowBlock::new(bid, module_name, scope, t.clone());
    blk.inputs.extend(b1.inputs.clone());
    blk.inputs.extend(b2.inputs.clone());
    blk.outputs.extend(b1.outputs.clone());
    blk.outputs.extend(b2.outputs.clone());
    blk.ctx.extend(b1.ctx.clone());
    blk.ctx.extend(b2.ctx.clone());
    blk.sig_decls.extend(b1.sig_decls.clone());
    blk.sig_decls.extend(b2.sig_decls.clone());
    blk.nodes_dataflow.extend(b1.nodes_dataflow.clone());
    blk.nodes_dataflow.extend(b2.nodes_dataflow.clone());
    blk.ast_ranges.extend(b1.ast_ranges.clone());
    blk.ast_ranges.extend(b2.ast_ranges.clone());
    blk
}

impl BlockParser for DataFlowBlockParser {
    type Block<'a> = DataFlowBlock;

    /// Note that last_input/output_vars should be in format `port-name.[var-name, ..., ]`, one port may have multiple variables
    /// Or maybe we need to refactor ModuleInput/ModuleOutput construction. split each with one port.
    fn parse_module<'a, 'b>(
        &self,
        tree: &'a SyntaxTree,
        scope: &str,
        last_port_connections: HashMap<NodeID, Vec<NodeID>>,
        module_code_mapping: &HashMap<String, String>,
    ) -> Vec<Self::Block<'b>> {
        // collect sig decls statements in current syntax tree
        let mut sig_decls = self.collect_sig_decls(tree);

        let mut res = vec![];
        let module_name = get_module_name(tree).unwrap();
        let code_content = module_code_mapping.get(&module_name).unwrap().as_str();

        for node in tree {
            if let Some(mut block) = self.new_block(tree, &module_name, scope, node) {
                // check block covered?

                if self.param_coverage_tracker.is_some() {
                    match self.check_block_cover_state(
                        module_name.as_str(),
                        tree,
                        code_content,
                        &block,
                    ) {
                        CoverState::Covered => {}
                        _ => continue,
                    }
                }

                let var_nodes = block
                    .get_input_nodes()
                    .into_iter()
                    .chain(block.get_output_nodes())
                    .map(|x| x.clone())
                    .collect::<HashSet<_>>();
                for v in var_nodes {
                    if let Some(decl) = sig_decls.get_mut(v.get_text()) {
                        block.add_signal_decl(decl)
                    }
                }

                // fill in last level variables
                // we need to refactor ModuleInput construction, each block contains only one port.
                // Module block only contain 1 input var and 1 outpu var.
                if matches!(block.get_block_type(), BlockType::ModuleInput) {
                    let port_connections = last_port_connections.iter().find(|(port, _)| {
                        block
                            .outputs
                            .iter()
                            .any(|n| n.get_text() == port.get_text())
                    });
                    if let Some((port, vars)) = port_connections {
                        vars.iter().for_each(|s| {
                            block.add_input_node(s.clone());
                            if let Some(vars) = block.nodes_dataflow.get_mut(port) {
                                vars.push(s.clone());
                            } else {
                                block.nodes_dataflow.insert(port.clone(), vec![s.clone()]);
                            }
                        });
                    }
                } else if matches!(block.get_block_type(), BlockType::ModuleOutput) {
                    let port_connections = last_port_connections.iter().find(|(port, _)| {
                        block.inputs.iter().any(|n| n.get_text() == port.get_text())
                    });
                    if let Some((port, vars)) = port_connections {
                        vars.iter().for_each(|s| {
                            block.add_output_node(s.clone());
                            if let Some(vars) = block.nodes_dataflow.get_mut(s) {
                                vars.push(port.clone());
                            } else {
                                block.nodes_dataflow.insert(s.clone(), vec![port.clone()]);
                            }
                        });
                    }
                }
                res.push(block);
            }
        }
        let merged = merge_all(res, merge_two);
        merged
    }

    fn parse<'a>(
        &self,
        module_tree_mapping: HashMap<String, Arc<SyntaxTree>>,
        module_code_mapping: HashMap<String, String>,
        top_module: &str,
        top_scope: &str,
    ) -> ScopeBlocks<'a, Self> {
        let top_tree = module_tree_mapping
            .get(top_module)
            .expect(format!("Unknown top module name: {}", top_module).as_str());
        let top_scope = top_scope.to_string();
        // self.parse_module(top_tree, top_scope, &[], &[]);
        let mut queue = VecDeque::new();
        queue.push_back((top_scope, top_tree, HashMap::new()));

        let mut ret = HashMap::new();
        while !queue.is_empty() {
            let (cur_scope, cur_tree, last_port_connections) = queue.pop_front().unwrap();
            let blocks = self.parse_module(
                cur_tree,
                &cur_scope,
                last_port_connections,
                &module_code_mapping,
            );

            // FIXME: due to some module is instantiated by parameter, so some module DUT may have the same name.
            ret.insert(cur_scope.clone(), (blocks, cur_tree.clone()));

            let mut cur_scope_prefix: Vec<String> = vec![];
            for event in cur_tree.into_iter().event() {
                match event {
                    NodeEvent::Enter(RefNode::GenerateBlockIdentifier(node)) => {
                        let ident = get_identifier_str(cur_tree, &node.nodes.0).unwrap();
                        cur_scope_prefix.push(ident.to_string());
                    }
                    NodeEvent::Leave(RefNode::GenerateBlock(_)) => {
                        cur_scope_prefix.pop();
                    }
                    NodeEvent::Enter(RefNode::ModuleInstantiation(inst)) => {
                        let module_name = get_module_name_from_instantiation(cur_tree, &inst)
                            .expect("Can't get module name from an instantiation");

                        let dut_name = get_dut_name_from_instantiation(cur_tree, &inst)
                            .expect("Can't get DUT name from an instantiation");

                        let prefix = cur_scope_prefix.join(".");

                        // TODO: replace scope with Vec<String> in the future
                        let next_scope = cur_scope.clone()
                            + &{
                                if prefix.len() == 0 {
                                    format!(".{dut_name}")
                                } else {
                                    format!(".{prefix}.{dut_name}")
                                }
                            };
                        let next_tree = module_tree_mapping.get(&module_name);
                        if let None = next_tree {
                            warn!("We met an Unknown submodule {} (dut_name={})", module_name, dut_name);
                            continue;
                        }
                        let next_tree = next_tree.unwrap();
                        let port_connections = get_port_connections(cur_tree, &inst);
                        queue.push_back((next_scope, next_tree, port_connections));
                    }
                    NodeEvent::Leave(_) => {}
                    _ => {}
                }
            }
        }
        ret
    }

    fn new_block<'a>(
        &'a self,
        tree: &SyntaxTree,
        module_name: &str,
        scope: &str,
        ref_node: RefNode<'a>,
    ) -> Option<Self::Block<'a>> {
        match ref_node {
            RefNode::AnsiPortDeclarationNet(decl) => {
                if let Some(port_direction) = get_port_direction(decl) {
                    return match port_direction {
                        PortDirection::Input(_) => {
                            Some(self.new_module_input_block(tree, module_name, scope, decl))
                        }
                        PortDirection::Output(_) => {
                            Some(self.new_module_output_block(tree, module_name, scope, decl))
                        }
                        _ => None,
                    };
                }
            }
            RefNode::AlwaysConstruct(always_construct) => {
                return Some(self.new_always_block(tree, module_name, scope, always_construct))
            }
            RefNode::NetAssignment(assign) => {
                return Some(self.new_assign_block(tree, module_name, scope, assign))
            }
            _ => {}
        }
        None
    }

    fn get_next_bid(&self) -> u64 {
        let v = self.max_bid.get();
        self.max_bid.set(v + 1);
        v + 1
    }
}

impl DataFlowBlockParser {
    pub fn collect_sig_decls(&self, tree: &SyntaxTree) -> HashMap<String, String> {
        let mut sig_decls = HashMap::new();
        // println!("{:#?}", self.syntax_tree);
        for node in tree {
            match node {
                RefNode::AnsiPortDeclarationNet(_)
                | RefNode::NetDeclaration(_)
                | RefNode::DataDeclaration(_) => {
                    let decl_code = get_ref_node_code_str(tree, node.clone());
                    if let Some(RefNode::Identifier(identifier)) =
                        decl_code.and(unwrap_node!(node, Identifier))
                    {
                        let var_name = get_identifier_str(tree, identifier).unwrap();
                        let decl_code = decl_code.unwrap().to_string();
                        // trim and add \n
                        let trimmed = decl_code.trim();
                        let trimmed = if trimmed.ends_with('\n') {
                            trimmed.to_string()
                        } else {
                            format!("{}\n", trimmed)
                        };
                        sig_decls.insert(var_name.to_string(), trimmed);
                    }
                }
                _ => {}
            }
        }
        sig_decls
    }

    // TODO: move to utils
    fn get_original_lineno_from_ast_locate(
        tree: &SyntaxTree,
        ast_locate: Locate,
        code_content: &str,
    ) -> Option<usize> {
        let (_, offset) = tree.get_origin(&ast_locate)?;
        get_pos_from_offset(code_content, offset).map(|(_col, row)| row)
    }

    fn check_block_cover_state<'a>(
        &self,
        module_name: &str,
        tree: &SyntaxTree,
        code_content: &str,
        block: &DataFlowBlock,
    ) -> CoverState {
        match block.get_block_type() {
            BlockType::ModuleInput | BlockType::ModuleOutput => CoverState::Covered,
            BlockType::Always(_) => {
                // TODO: fix?
                CoverState::Covered
            }
            BlockType::Assign => {
                // If no coverage tracker is available, treat as covered
                let tracker = match self.param_coverage_tracker.as_ref() {
                    Some(t) => t,
                    None => return CoverState::Covered,
                };
                // If tracker has no data for this module, treat as covered
                if !tracker.has_module_data(module_name) {
                    return CoverState::Covered;
                }
                let covered_lines = block
                    .get_covered_line_locates()
                    .into_iter()
                    .filter_map(|locate| {
                        // TODO: convert lineno in AST to original lineno
                        let ast_locate = Locate {
                            offset: locate.offset,
                            len: locate.len,
                            line: locate.line,
                        };
                        let lineno_option = Self::get_original_lineno_from_ast_locate(tree, ast_locate, code_content);
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
                        self.param_coverage_tracker
                            .as_ref()
                            .and_then(|tracker| {
                                tracker
                                    .check_covered(
                                        Some(module_name),
                                        original_lineno,
                                    )
                                    .map(|count| (lineno, count))
                            })
                    })
                    .collect::<Vec<_>>();
                if covered_lines.len() > 0 {
                    CoverState::Covered
                } else {
                    CoverState::Uncovered
                }
            }
        }
    }

    fn new_module_input_block<'a>(
        &self,
        tree: &SyntaxTree,
        module_name: &str,
        scope: &str,
        ref_node: &'a AnsiPortDeclarationNet,
    ) -> <DataFlowBlockParser as BlockParser>::Block<'a> {
        let mut blk = DataFlowBlock::new(
            self.get_next_bid(),
            module_name,
            scope,
            BlockType::ModuleInput,
        );
        if let Some(RefNode::PortIdentifier(port_identifier)) =
            unwrap_node!(ref_node, PortIdentifier)
        {
            if let Some(RefNode::Identifier(identifier)) = unwrap_node!(port_identifier, Identifier)
            {
                let node = NodeID::new_identifier(tree, RefNode::Identifier(identifier)).unwrap();
                blk.add_output_node(node);

                let code =
                    get_ref_node_code_str(tree, RefNode::AnsiPortDeclarationNet(ref_node)).unwrap();
                blk.add_ctx(code);
            }
        }
        get_node_ast_range(ref_node).map(|ast_range: AstRange| {
            blk.ast_ranges.push(ast_range);
        });
        blk
    }
    fn new_module_output_block<'a>(
        &self,
        tree: &SyntaxTree,
        module_name: &str,
        scope: &str,
        ref_node: &'a AnsiPortDeclarationNet,
    ) -> <DataFlowBlockParser as BlockParser>::Block<'a> {
        let mut blk = DataFlowBlock::new(
            self.get_next_bid(),
            module_name,
            scope,
            BlockType::ModuleOutput,
        );
        if let Some(RefNode::Identifier(identifier)) = unwrap_node!(ref_node, Identifier) {
            let node = NodeID::new_identifier(tree, RefNode::Identifier(identifier)).unwrap();
            blk.add_input_node(node);

            let code =
                get_ref_node_code_str(tree, RefNode::AnsiPortDeclarationNet(ref_node)).unwrap();
            blk.add_ctx(code);
        }
        get_node_ast_range(ref_node).map(|ast_range: AstRange| {
            blk.ast_ranges.push(ast_range);
        });
        blk
    }
    fn new_always_block<'a>(
        &self,
        tree: &SyntaxTree,
        module_name: &str,
        scope: &str,
        ref_node: &'a AlwaysConstruct,
    ) -> <DataFlowBlockParser as BlockParser>::Block<'a> {
        let ctype = CircuitType::from(ref_node.clone());
        let mut blk = DataFlowBlock::new(
            self.get_next_bid(),
            module_name,
            scope,
            BlockType::Always(ctype),
        );

        let dataflow_analyzer = DataFlowAnalyzer::new(tree);
        let all =
            dataflow_analyzer.get_all_from_always(unwrap_node!(ref_node, AlwaysConstruct).unwrap());
        all.iter().for_each(|(node, vars)| {
            blk.add_output_node(node.clone());
            blk.nodes_dataflow.insert(node.clone(), vars.clone());
            vars.iter().for_each(|var| {
                blk.add_input_node(var.clone());
            });
        });

        let code = get_ref_node_code_str(tree, RefNode::AlwaysConstruct(ref_node)).unwrap();
        blk.add_ctx(code);
        get_node_ast_range(ref_node).map(|ast_range: AstRange| {
            blk.ast_ranges.push(ast_range);
        });
        blk
    }
    fn new_assign_block<'a>(
        &self,
        tree: &SyntaxTree,
        module_name: &str,
        scope: &str,
        ref_node: &'a NetAssignment,
    ) -> <DataFlowBlockParser as BlockParser>::Block<'a> {
        let mut blk =
            DataFlowBlock::new(self.get_next_bid(), module_name, scope, BlockType::Assign);
        if let Some(RefNode::NetLvalue(x)) = unwrap_node!(ref_node, NetLvalue) {
            for n in x {
                if let RefNode::Identifier(identifier) = n {
                    let node =
                        NodeID::new_identifier(tree, RefNode::Identifier(identifier)).unwrap();
                    blk.add_output_node(node)
                }
            }
        }
        if let Some(RefNode::Expression(expr)) = unwrap_node!(ref_node, Expression) {
            for n in expr {
                if let RefNode::Identifier(identifier) = n {
                    let node =
                        NodeID::new_identifier(tree, RefNode::Identifier(identifier)).unwrap();
                    blk.add_input_node(node)
                }
            }
        }

        let (left_nodes, right_nodes) =
            DataFlowAnalyzer::new(tree).get_all_from_assignment(ref_node);
        left_nodes.into_iter().for_each(|node| {
            blk.nodes_dataflow.insert(node, right_nodes.clone());
        });

        let code = format!(
            "assign {};",
            get_ref_node_code_str(tree, RefNode::NetAssignment(ref_node)).unwrap()
        );
        blk.add_ctx(&code);
        get_node_ast_range(ref_node).map(|ast_range: AstRange| {
            blk.ast_ranges.push(ast_range);
        });
        blk
    }
}
