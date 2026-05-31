pub mod dfb;
pub mod mgr;
pub mod utils;

use crate::dataflow::NodeID;
use crate::{NodeLocate, TimeAnnotation};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display};
use std::sync::Arc;
use sv_parser::{AlwaysConstruct, AlwaysKeyword, RefNode, SyntaxTree};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum CircuitType {
    SEQ,
    COMB,
}

impl From<AlwaysConstruct> for CircuitType {
    fn from(always: AlwaysConstruct) -> Self {
        match always.nodes.0 {
            AlwaysKeyword::AlwaysComb(_) => CircuitType::COMB,
            AlwaysKeyword::AlwaysFf(_) | AlwaysKeyword::AlwaysLatch(_) => CircuitType::SEQ,
            // Plain `always` — in synthesized Chisel/RTL code, `always @(posedge clk)`
            // is sequential. Default to SEQ since most plain `always` blocks are clocked.
            AlwaysKeyword::Always(_) => CircuitType::SEQ,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum BlockType {
    ModuleInput,
    ModuleOutput,
    Always(CircuitType),
    Assign,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum CoverState {
    Covered,
    Uncovered,
}

impl Display for BlockType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            BlockType::ModuleInput => "ModuleInput".to_string(),
            BlockType::ModuleOutput => "ModuleOutput".to_string(),
            BlockType::Always(CircuitType::COMB) => "AlwaysComb".to_string(),
            BlockType::Always(CircuitType::SEQ) => "AlwaysSeq".to_string(),
            BlockType::Assign => "Assign".to_string(),
        };
        write!(f, "{}", str)
    }
}

impl BlockType {
    pub fn is_seq(&self) -> bool {
        match self {
            BlockType::Always(ctype) => {
                matches!(ctype, CircuitType::SEQ)
            }
            _ => false,
        }
    }
}

type ScopeBlocks<'a, Parser> =
    HashMap<String, (Vec<<Parser as BlockParser>::Block<'a>>, Arc<SyntaxTree>)>;

type SuspiciousTrace = (NodeID, Vec<(NodeID, Option<TimeAnnotation>)>);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AstRange {
    pub offset: usize,
    pub len: usize,
}

impl AstRange {
    pub fn new(offset: usize, len: usize) -> Self {
        AstRange { offset, len }
    }

    pub fn check_cover(&self, other: &AstRange) -> bool {
        other.offset >= self.offset && other.offset + other.len <= self.offset + self.len
    }
}

pub trait Block<'a>: Clone + Debug + Serialize {
    fn get_block_type(&self) -> &BlockType;
    fn get_bid(&self) -> u64;
    fn get_module_name(&self) -> &str;
    fn get_scope(&self) -> &str;
    fn get_input_nodes(&self) -> HashSet<&NodeID>;
    fn get_output_nodes(&self) -> HashSet<&NodeID>;
    fn get_node_dataflow(&self, node: NodeID) -> HashSet<NodeID>;
    fn get_suspicious_trace(&self) -> Option<&SuspiciousTrace>;
    fn get_ctx(&self) -> Vec<&str>;
    fn get_signal_decls(&self) -> Vec<&str>;
    fn add_input_node(&mut self, node: NodeID);
    fn add_output_node(&mut self, node: NodeID);
    fn add_ctx(&mut self, ctx: &str);
    fn add_signal_decl(&mut self, signal_decl: &str);
    fn add_suspicious_trace(&mut self, node: NodeID, vars: Vec<(NodeID, Option<TimeAnnotation>)>);
    fn get_covered_ast_lines(&self) -> Vec<u32> {
        self.get_output_nodes()
            .into_iter()
            .map(|node| node.get_locate().line)
            .collect()
    }
    fn get_covered_original_lines(&self) -> Vec<u32> {
        self.get_output_nodes()
            .into_iter()
            .map(|node| node.get_locate().original_line)
            .collect()
    }
    fn get_covered_line_locates(&self) -> Vec<NodeLocate> {
        self.get_output_nodes()
            .into_iter()
            .map(|node| node.get_locate().clone())
            .collect()
    }
    fn get_ast_covered_ranges(&self) -> Vec<AstRange>;
}

pub trait BlockParser {
    type Block<'a>: Block<'a>;
    fn parse_module<'a, 'b>(
        &self,
        tree: &'a SyntaxTree,
        scope: &str,
        last_port_connections: HashMap<NodeID, Vec<NodeID>>,
        module_code_mapping: &HashMap<String, String>,
    ) -> Vec<Self::Block<'b>>;
    fn parse<'a>(
        &self,
        module_tree_mapping: HashMap<String, Arc<SyntaxTree>>,
        module_code_mapping: HashMap<String, String>,
        top_module: &str,
        top_scope: &str,
    ) -> ScopeBlocks<'a, Self>;
    fn new_block<'a>(
        &'a self,
        tree: &SyntaxTree,
        module_name: &str,
        scope: &str,
        ref_node: RefNode<'a>,
    ) -> Option<Self::Block<'a>>;
    fn get_next_bid(&self) -> u64;
}
