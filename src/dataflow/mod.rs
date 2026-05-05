use crate::block::utils::{
    collect_locates, get_identifiers_from_expression,
    get_left_values_in_assignment, get_right_values_in_expression, merge_locates,
};
use crate::get_pos_from_offset;
use anyhow::anyhow;
use log::warn;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::fs::read_to_string;
use std::hash::Hash;
use sv_parser::{
    unwrap_node, AlwaysConstruct, Locate, NetAssignment, NodeEvent, RefNode, RefNodes, SyntaxTree,
};

/// input: ast ref_node, var name
/// output: Vec[HashMap<sv_parser::Identifier, vars>]

#[derive(Debug, Clone, Default, Eq, PartialEq, Hash, Serialize, Deserialize, Ord, PartialOrd)]
pub struct NodeLocate {
    pub offset: usize,
    pub line: u32,
    pub original_line: u32,
    pub len: usize,
}

impl NodeLocate {
    fn new(locate: Locate, original_line: u32) -> Self {
        Self {
            offset: locate.offset,
            line: locate.line,
            original_line,
            len: locate.len,
        }
    }
}

// impl From<Locate> for NodeLocate {
//     fn from(value: Locate) -> Self {
//         Self {
//             offset: value.offset,
//             line: value.line,
//             len: value.len,
//         }
//     }
// }

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize, Ord, PartialOrd)]
pub enum NodeID {
    Identifier(NodeLocate, String),
    Literal(NodeLocate, String),
}

impl Display for NodeID {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let json_values = serde_json::to_string(self).unwrap();
        write!(f, "{}", json_values)
    }
}

impl NodeID {
    pub fn from_ref_node(tree: &SyntaxTree, value: RefNode) -> Result<Self, anyhow::Error> {
        match value {
            RefNode::Identifier(_) => Self::new_identifier(tree, value),
            RefNode::PrimaryLiteral(_) => Self::new_literal(tree, value),
            _ => anyhow::bail!("Not valid RefNode type for NodeID"),
        }
    }

    pub fn new_identifier(tree: &SyntaxTree, value: RefNode) -> Result<Self, anyhow::Error> {
        let identifier = unwrap_node!(value.clone(), Identifier)
            .ok_or(anyhow!("Can not found Identifier in this RefNode"))?;
        let locates = collect_locates(identifier);
        if locates.is_empty() {
            anyhow::bail!("No Locate for NodeID");
        }
        let locate = locates.get(0).unwrap().clone();
        let original_lineno = NodeID::get_original_lineno(tree, locates)?;
        Ok(NodeID::Identifier(
            NodeLocate::new(locate, original_lineno),
            tree.get_str(RefNodes(vec![value]))
                .unwrap()
                .trim()
                .to_string(),
        ))
    }

    pub fn new_literal(tree: &SyntaxTree, value: RefNode) -> Result<Self, anyhow::Error> {
        let literal = unwrap_node!(value.clone(), PrimaryLiteral)
            .ok_or(anyhow!("Can not found PrimaryLiteral in this RefNode"))?;
        let mut locates = collect_locates(literal);
        if let Some(locate) = merge_locates(&mut locates) {
            let original_lineno = NodeID::get_original_lineno(tree, locates)?;
            Ok(NodeID::Literal(
                NodeLocate::new(locate, original_lineno),
                tree.get_str(RefNodes(vec![value]))
                    .unwrap()
                    .trim()
                    .to_string(),
            ))
        } else {
            anyhow::bail!("Not valid locates to merge")
        }
    }

    pub fn get_original_lineno(
        tree: &SyntaxTree,
        locates: Vec<Locate>,
    ) -> Result<u32, anyhow::Error> {
        let locate = locates.get(0).ok_or(anyhow!("Locates len == 0"))?.clone();
        // Note that, the offset returned by get_origin is the offset after original src file macro expansion.
        let (path, offset) = tree
            .get_origin(&locate)
            .ok_or(anyhow!("Cannot tree.get_origin"))?;
        let code = read_to_string(path)?;

        let original_lineno = match get_pos_from_offset(&code, offset) {
            Some((_ , original_lineno)) => original_lineno,
            None => {
                // FIXME: get_origin may be buggy when file A use a macro defined in file B. locate -> original file is correct.
                // but the offset is wrong. it looks like the returned offset by get_origin is the one expanded one of file B.
                warn!(
                    "Cannot get pos from code at path = {:?}, offset = {}, len code = {}, locate_str = {:?}",
                    path,
                    offset,
                    code.len(),
                    tree.get_str(&locates)
                );
                0
            }
        };

        // except the original files that are located in macros folders, locate mapping are correct.
        // if !path.starts_with("/home/lzz/exp_wkdir/ibex_test/ibex/vendor") {
        //     let oracle_str = tree.get_str(&vec![locate]).unwrap();
        //     let real_str = &code[offset..offset + locate.len].trim();
        //     // looks strange here :(
        //     let shift_str = &code[offset + 1..offset + 1 + locate.len].trim();
        //     assert!(
        //         oracle_str.starts_with(real_str) || oracle_str.starts_with(shift_str),
        //         "left: {}, right: {}",
        //         tree.get_str(&locates).unwrap(),
        //         {
        //             println!(
        //                 "get pos from code at path = {:?}, offset = {}, len code = {}, locate_str = {:?}",
        //                 path,
        //                 offset,
        //                 code.len(),
        //                 oracle_str,
        //             );
        //             real_str
        //         }
        //     );
        // }

        Ok(original_lineno as u32)
    }

    pub fn get_locate(&self) -> &NodeLocate {
        match self {
            NodeID::Identifier(locate, _) => locate,
            NodeID::Literal(locate, _) => locate,
        }
    }

    pub fn get_text(&self) -> &str {
        match self {
            NodeID::Identifier(_, text) => text,
            NodeID::Literal(_, text) => text,
        }
        .trim()
    }
}

pub struct DataFlowAnalyzer<'a> {
    tree: &'a SyntaxTree,
}

impl<'a> DataFlowAnalyzer<'a> {
    pub fn new(tree: &'a SyntaxTree) -> Self {
        DataFlowAnalyzer { tree }
    }
    pub fn get_vars(&self, ref_node: RefNode, var_name: &str) -> Vec<(NodeID, Vec<NodeID>)> {
        let mut res = vec![];
        for node in ref_node {
            match node {
                RefNode::NetAssignment(assign) => {
                    if let Some((l_node_id, r_vars)) =
                        self.get_vars_from_assignment(assign, var_name)
                    {
                        res.push((l_node_id, r_vars));
                    }
                }
                RefNode::AlwaysConstruct(always_construct) => {
                    if let Some(vars) = self.get_vars_from_always(always_construct, var_name) {
                        res.extend(vars);
                    }
                }
                _ => {}
            }
        }
        res
    }

    pub fn get_vars_from_assignment(
        &self,
        ref_node: &NetAssignment,
        var_name: &str,
    ) -> Option<(NodeID, Vec<NodeID>)> {
        let (left_nodes, right_nodes) = self.get_all_from_assignment(ref_node);
        let res = left_nodes
            .iter()
            .filter_map(|node| match node {
                NodeID::Identifier(_, name) => {
                    if name == var_name {
                        Some((node.clone(), right_nodes.clone()))
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .next()?;
        Some(res)
    }

    // There may be many positions having the same var_name, so we return a Vec of (NodeID, Vec<NodeID>)
    pub fn get_vars_from_always(
        &self,
        ref_node: &AlwaysConstruct,
        var_name: &str,
    ) -> Option<Vec<(NodeID, Vec<NodeID>)>> {
        let ref_node = unwrap_node!(ref_node, AlwaysConstruct)?;
        let all = self.get_all_from_always(ref_node);
        let res = all
            .iter()
            .filter_map(|(node_id, vars)| match node_id {
                NodeID::Identifier(_, name) => {
                    if name == var_name {
                        Some((node_id.clone(), vars.to_vec()))
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect();
        Some(res)
    }

    fn collect_predicates(stack: &Vec<(&str, Vec<NodeID>)>) -> Vec<NodeID> {
        stack.iter().map(|(_, ids)| ids.clone()).flatten().collect()
    }

    pub fn get_all_from_always(&self, cur_node: RefNode) -> Vec<(NodeID, Vec<NodeID>)> {
        let mut predicate: Vec<(&str, Vec<NodeID>)> = vec![];
        let mut case_declarations: Vec<Vec<NodeID>> = vec![];
        let mut ret: Vec<(NodeID, Vec<NodeID>)> = vec![];
        for node in cur_node.into_iter().event() {
            match node {
                NodeEvent::Enter(RefNode::CondPredicate(cond_predicate)) => {
                    if let Some(expr) = unwrap_node!(cond_predicate, Expression) {
                        match expr {
                            RefNode::Expression(expr) => {
                                let node_ids = get_identifiers_from_expression(expr)
                                    .into_iter()
                                    .filter_map(|node| NodeID::from_ref_node(self.tree, node).ok())
                                    .collect::<Vec<NodeID>>();
                                predicate.push(("cond", node_ids))
                            }
                            _ => {}
                        }
                    }
                }
                NodeEvent::Enter(RefNode::CaseExpression(case_expression)) => {
                    let node_ids = get_identifiers_from_expression(&case_expression.nodes.0)
                        .into_iter()
                        .filter_map(|node| NodeID::from_ref_node(self.tree, node).ok())
                        .collect::<Vec<NodeID>>();
                    predicate.push(("case", node_ids))
                }
                NodeEvent::Enter(RefNode::CaseItemExpression(case_item_expression)) => {
                    let node_ids = get_identifiers_from_expression(&case_item_expression.nodes.0)
                        .into_iter()
                        .filter_map(|node| NodeID::from_ref_node(self.tree, node).ok())
                        .collect::<Vec<NodeID>>();
                    predicate.push(("case", node_ids))
                }
                NodeEvent::Enter(RefNode::CaseItemDefault(default)) => {
                    // we ignore vars that in left assignments in this default branch, as these vars will be analyzed in blocking/non-blocking assignments.
                    let mut ignored_left_vars = vec![];
                    for node in &default.nodes.2 {
                        match node {
                            RefNode::BlockingAssignment(statement) => {
                                let left_node_ids = get_left_values_in_assignment(
                                    RefNode::BlockingAssignment(statement),
                                )
                                .into_iter()
                                .filter_map(|node| NodeID::from_ref_node(self.tree, node).ok())
                                .collect::<Vec<NodeID>>();
                                ignored_left_vars.extend(left_node_ids);
                            }
                            RefNode::NonblockingAssignment(statement) => {
                                let left_node_ids = get_left_values_in_assignment(
                                    RefNode::NonblockingAssignment(statement),
                                )
                                .into_iter()
                                .filter_map(|node| NodeID::from_ref_node(self.tree, node).ok())
                                .collect::<Vec<NodeID>>();
                                ignored_left_vars.extend(left_node_ids);
                            }
                            _ => {}
                        }
                    }

                    // We consider that if a default branch is covered. All var declarations before this case have all predicates in this case.
                    // it looks not robust.
                    while let Some(declarations) = case_declarations.pop() {
                        declarations
                            .iter()
                            // ignore vars that have default assignment statements.
                            .filter(|decl| {
                                ignored_left_vars
                                    .iter()
                                    .all(|iv| iv.get_text() != decl.get_text())
                            })
                            .for_each(|decl| {
                                ret.iter_mut()
                                    .filter_map(|(c, fs)| if c == decl { Some(fs) } else { None })
                                    .for_each(|fs| fs.extend(Self::collect_predicates(&predicate)));
                            })
                    }
                }
                NodeEvent::Enter(RefNode::BlockingAssignment(statement)) => {
                    let left_node_ids =
                        get_left_values_in_assignment(RefNode::BlockingAssignment(statement))
                            .into_iter()
                            .filter_map(|node| NodeID::from_ref_node(self.tree, node).ok())
                            .collect::<Vec<NodeID>>();
                    let right_node_ids =
                        get_right_values_in_expression(RefNode::BlockingAssignment(statement))
                            .into_iter()
                            .filter_map(|node| NodeID::from_ref_node(self.tree, node).ok())
                            .collect::<Vec<NodeID>>();
                    for node_id in left_node_ids {
                        let mut right_clone = right_node_ids.clone();
                        right_clone.extend(Self::collect_predicates(&predicate));
                        ret.push((node_id, right_clone));
                    }
                }
                NodeEvent::Enter(RefNode::NonblockingAssignment(statement)) => {
                    let left_node_ids =
                        get_left_values_in_assignment(RefNode::NonblockingAssignment(statement))
                            .into_iter()
                            .filter_map(|node| NodeID::from_ref_node(self.tree, node).ok())
                            .collect::<Vec<NodeID>>();
                    let right_node_ids =
                        get_right_values_in_expression(RefNode::NonblockingAssignment(statement))
                            .into_iter()
                            .filter_map(|node| NodeID::from_ref_node(self.tree, node).ok())
                            .collect::<Vec<NodeID>>();
                    for node_id in left_node_ids {
                        let mut right_clone = right_node_ids.clone();
                        right_clone.extend(Self::collect_predicates(&predicate));
                        ret.push((node_id, right_clone));
                    }
                }
                NodeEvent::Enter(RefNode::VariableDeclAssignment(statement)) => {
                    let left_node_ids =
                        get_left_values_in_assignment(RefNode::VariableDeclAssignment(statement))
                            .into_iter()
                            .filter_map(|node| NodeID::from_ref_node(self.tree, node).ok())
                            .collect::<Vec<NodeID>>();
                    let right_node_ids =
                        get_right_values_in_expression(RefNode::VariableDeclAssignment(statement))
                            .into_iter()
                            .filter_map(|node| NodeID::from_ref_node(self.tree, node).ok())
                            .collect::<Vec<NodeID>>();
                    for node_id in &left_node_ids {
                        let mut right_clone = right_node_ids.clone();
                        right_clone.extend(Self::collect_predicates(&predicate));
                        ret.push((node_id.clone(), right_clone));
                    }
                    case_declarations.push(left_node_ids);
                }
                NodeEvent::Leave(RefNode::CaseStatement(_)) => {
                    while let Some((label, _)) = predicate.last() {
                        if *label == "case" {
                            predicate.pop();
                        } else {
                            break;
                        }
                    }
                }
                NodeEvent::Leave(RefNode::ConditionalStatement(_)) => {
                    let (name, _) = predicate.pop().unwrap();
                    assert_eq!(name, "cond");
                }
                _ => {}
            }
        }
        ret
    }

    pub fn get_all_from_assignment(&self, ref_node: &NetAssignment) -> (Vec<NodeID>, Vec<NodeID>) {
        let mut left_nodes = vec![];
        if let Some(RefNode::NetLvalue(x)) = unwrap_node!(ref_node, NetLvalue) {
            for n in x {
                if let RefNode::Identifier(_) = n {
                    left_nodes.push(NodeID::new_identifier(self.tree, n).unwrap())
                }
            }
        }
        let mut right_nodes = vec![];
        if let Some(RefNode::Expression(expr)) = unwrap_node!(ref_node, Expression) {
            right_nodes.extend(get_identifiers_from_expression(expr))
        }
        let right_nodes = right_nodes
            .into_iter()
            .filter_map(|ref_node| NodeID::from_ref_node(self.tree, ref_node).ok())
            .collect::<Vec<_>>();

        (left_nodes, right_nodes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use sv_parser::parse_sv;
    use std::collections::HashMap;
    use crate::get_identifier_str;

    #[test]
    fn test_ast_traverse() {
        let path = "tests/test_files/if_case.sv";
        let defines = HashMap::new();
        let includes: Vec<PathBuf> = Vec::new();
        let (tree, _) = parse_sv(path, &defines, &includes, false, false).unwrap();
        println!("{:?}", tree);
        for node in &tree {
            match node {
                RefNode::Identifier(identifier) => {
                    if let Some(RefNode::Locate(locate)) = unwrap_node!(identifier, Locate) {
                        println!(
                            "name: {}, line: {}, offset: {}",
                            get_identifier_str(&tree, identifier).unwrap(),
                            locate.line,
                            locate.offset
                        );
                    }
                }
                _ => {}
            }
        }
    }

    #[test]
    fn test_dataflow_analyzer() {
        let path = "tests/test_files/if_case.sv";
        let defines = HashMap::new();
        let includes: Vec<PathBuf> = Vec::new();
        let (tree, _) = parse_sv(path, &defines, &includes, false, false).unwrap();
        let df_analyzer = DataFlowAnalyzer::new(&tree);
        for node in &tree {
            if let RefNode::AlwaysConstruct(always_construct) = node {
                let res = df_analyzer
                    .get_vars_from_always(&always_construct, "temp_out2")
                    .unwrap();
                assert_eq!(res.len(), 6);
                res.iter().for_each(|(left_node, src_nodes)| {
                    let src_nodes = src_nodes
                        .iter()
                        .map(|node| match node {
                            NodeID::Identifier(_, name) | NodeID::Literal(_, name) => (name, node),
                        })
                        .collect::<Vec<_>>();

                    println!("left_node: {:#?}, src_nodes: {:#?}", left_node, src_nodes);
                })
            }
        }
    }

    #[test]
    fn test_always_comb_assign() {
        let path = "/home/lzz/demos/vlt_coverage/calc_operations.sv";
        let defines = HashMap::new();
        let includes: Vec<PathBuf> = Vec::new();
        let (tree, _) = parse_sv(path, &defines, &includes, false, false).unwrap();
        println!("{:#?}", tree);
        let df_analyzer = DataFlowAnalyzer::new(&tree);
        for node in &tree {
            if let RefNode::AlwaysConstruct(always_construct) = node {
                // println!("{:#?}", node);
                let res = df_analyzer.get_all_from_always(node);
                println!("{:#?}", res);
            }
        }
    }
}
