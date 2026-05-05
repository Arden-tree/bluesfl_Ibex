use crate::block::{AstRange, Block};
use crate::dataflow::NodeID;
use crate::{BlockType, LineOffset};
use anyhow::anyhow;
use log::warn;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::BuildHasher;
use std::path::Path;
use sv_parser::{
    preprocess, unwrap_node, AnsiPortDeclarationNet, Defines, Expression,
    Identifier, Iter, Locate, ModuleInstantiation, NodeEvent, PortDirection, RefNode, RefNodes,
    Statement, SyntaxTree,
};

const CHAR_CR: u8 = 0x0d;
const CHAR_LF: u8 = 0x0a;

pub fn merge_all2one<'a, T>(blocks: &[T]) -> anyhow::Result<T>
where
    T: Block<'a>,
{
    if blocks.is_empty() {
        anyhow::bail!("empty blocks when merge_all2one");
    }

    let mut merged_block = blocks[0].clone();

    for block in blocks.into_iter().skip(1) {
        for input_var in block.get_input_nodes() {
            merged_block.add_input_node(input_var.clone());
        }

        for output_var in block.get_output_nodes() {
            merged_block.add_output_node(output_var.clone());
        }

        for ctx in block.get_ctx() {
            merged_block.add_ctx(ctx);
        }

        for signal_decl in block.get_signal_decls() {
            merged_block.add_signal_decl(signal_decl);
        }
    }

    Ok(merged_block)
}

fn merge_block<'a, T, F>(b1: &T, b2: &T, merge_two: F) -> Option<T>
where
    T: Block<'a>,
    F: FnOnce(u64, &str, BlockType, &T, &T) -> T,
{
    let bid = b1.get_bid();
    if b1.get_block_type() == b2.get_block_type() && b1.get_module_name() == b2.get_module_name() {
        return match &b1.get_block_type() {
            _t @ (BlockType::ModuleInput | BlockType::ModuleOutput) => {
                // Some(merge_two(bid, &b1.module_name, t.clone(), b1, b2))
                None
            }
            BlockType::Always(_) => None,
            BlockType::Assign => {
                for inp in &b1.get_input_nodes() {
                    // FIXME: NodeID check equal is different with &str; any other places has this bug?
                    if b2
                        .get_output_nodes()
                        .iter()
                        .any(|node| node.get_text() == inp.get_text())
                    {
                        return Some(merge_two(
                            bid,
                            b1.get_module_name(),
                            BlockType::Assign,
                            b1,
                            b2,
                        ));
                    }
                }
                for inp in &b2.get_input_nodes() {
                    if b1
                        .get_output_nodes()
                        .iter()
                        .any(|node| node.get_text() == inp.get_text())
                    {
                        return Some(merge_two(
                            bid,
                            b1.get_module_name(),
                            BlockType::Assign,
                            b1,
                            b2,
                        ));
                    }
                }
                None
            }
        };
    }
    None
}

pub fn merge_all<'a, T, F>(mut res: Vec<T>, f: F) -> Vec<T>
where
    T: Block<'a>,
    F: FnOnce(u64, &str, BlockType, &T, &T) -> T + Clone,
{
    loop {
        let mut merged = Vec::new();
        let mut used = vec![false; res.len()];
        let mut changed = false;

        for i in 0..res.len() {
            if used[i] {
                continue;
            }

            let mut merged_once = false;
            for j in (i + 1)..res.len() {
                if used[j] {
                    continue;
                }

                if let Some(blk) = merge_block(&res[i], &res[j], f.clone()) {
                    merged.push(blk);
                    used[i] = true;
                    used[j] = true;
                    merged_once = true;
                    changed = true;
                    break; // Only merge once per element
                }
            }

            if !merged_once {
                merged.push(res[i].clone());
            }
        }

        if !changed {
            break;
        }

        res = merged;
    }
    res
}

// TODO: update api arg -> RefNode

pub fn get_identifier_locate(node: RefNode) -> Option<Locate> {
    match unwrap_node!(node, SimpleIdentifier, EscapedIdentifier) {
        Some(RefNode::SimpleIdentifier(x)) => Some(x.nodes.0),
        Some(RefNode::EscapedIdentifier(x)) => Some(x.nodes.0),
        _ => None,
    }
}

pub fn get_identifier_str<'a>(tree: &'a SyntaxTree, identifier: &Identifier) -> Option<&'a str> {
    let id_ref_node = unwrap_node!(identifier, Identifier)?;
    let id = get_identifier_locate(id_ref_node)?;
    let id = tree.get_str(&id)?;
    Some(id)
}

pub fn get_ref_node_code_str<'a>(tree: &'a SyntaxTree, ref_node: RefNode) -> Option<&'a str> {
    let code = tree.get_str(RefNodes(vec![ref_node]))?;
    Some(code)
}

pub fn get_port_direction(ref_node: &AnsiPortDeclarationNet) -> Option<PortDirection> {
    if let Some(node) = unwrap_node!(ref_node, PortDirection) {
        return match node {
            RefNode::PortDirection(x) => Some(x.clone()),
            _ => None,
        };
    }
    None
}

fn extract_lvalue(ref_node: RefNode) -> Option<RefNode> {
    match ref_node {
        // NetLvalue maybe recursive, so we use more bottom, NetLvalueIdentifier
        RefNode::NetLvalueIdentifier(lvalue) => Some(RefNode::from(lvalue)),
        RefNode::VariableLvalue(lvalue) => Some(RefNode::from(lvalue)),
        RefNode::NonrangeVariableLvalue(lvalue) => Some(RefNode::from(lvalue)),
        RefNode::VariableIdentifier(lvalue) => Some(RefNode::from(lvalue)),
        _ => None,
    }
}

fn extract_identifier_or_literal(ref_node: RefNode) -> Vec<RefNode> {
    let mut flag = true;
    let mut res = vec![];
    for event in ref_node.into_iter().event() {
        match event {
            NodeEvent::Enter(RefNode::ConstantSelect(_)) => {
                flag = false;
            }
            NodeEvent::Leave(RefNode::ConstantSelect(_)) => {
                flag = true;
            }
            NodeEvent::Enter(RefNode::Select(_)) => {
                flag = false;
            }
            NodeEvent::Leave(RefNode::Select(_)) => {
                flag = true;
            }
            NodeEvent::Enter(ident) => {
                if flag {
                    if let RefNode::Identifier(_) = ident {
                        res.push(ident);
                    } else if let RefNode::PrimaryLiteral(_) = ident {
                        res.push(ident);
                    }
                }
            }
            _ => {}
        }
    }
    res
}

pub fn get_left_values_from_statement(ref_node: &Statement) -> Vec<RefNode> {
    let mut res = vec![];
    for lvalue_ref_node in ref_node {
        if let Some(lvalue) = extract_lvalue(lvalue_ref_node) {
            for ident in lvalue {
                if let RefNode::Identifier(_) = ident {
                    res.push(ident);
                }
            }
        }
    }
    res
}

pub fn get_left_values_in_assignment(ref_node: RefNode) -> Vec<RefNode> {
    let mut res = vec![];
    for lvalue_ref_node in ref_node {
        if let Some(lvalue) = extract_lvalue(lvalue_ref_node) {
            res.extend(extract_identifier_or_literal(lvalue));
        }
    }
    res
}

pub fn get_right_values_in_expression(ref_node: RefNode) -> Vec<RefNode> {
    let mut res = vec![];
    for rvalue_ref_node in ref_node {
        if let RefNode::Expression(expr) = rvalue_ref_node {
            res.extend(get_identifiers_from_expression(expr));
            break;
        }
    }
    res
}

pub fn get_identifiers_from_expression(expr: &Expression) -> Vec<RefNode> {
    let res = extract_identifier_or_literal(RefNode::Expression(expr));
    res
}

pub fn get_right_values_from_statement(ref_node: &Statement) -> Vec<RefNode> {
    let mut res = vec![];
    for rvalue_ref_node in ref_node {
        if let RefNode::Expression(expr) = rvalue_ref_node {
            res.extend(get_identifiers_from_expression(expr));
            break;
        }
    }
    res
}

pub fn get_module_name(tree: &SyntaxTree) -> Option<String> {
    for node in tree {
        match node {
            RefNode::ModuleDeclarationNonansi(x) => {
                let id = unwrap_node!(x, Identifier)?;
                if let RefNode::Identifier(identifier) = id {
                    let id = get_identifier_str(tree, identifier)?;
                    return Some(id.to_string());
                }
            }
            RefNode::ModuleDeclarationAnsi(x) => {
                let id = unwrap_node!(x, Identifier)?;
                if let RefNode::Identifier(identifier) = id {
                    let id = get_identifier_str(tree, identifier)?;
                    return Some(id.to_string());
                }
            }
            _ => (),
        }
    }
    None
}

pub fn get_submodules(tree: &SyntaxTree) -> anyhow::Result<HashSet<String>> {
    let mut res = HashSet::new();
    for node in tree {
        if let Some(RefNode::ModuleInstantiation(inst)) = unwrap_node!(node, ModuleInstantiation) {
            if let Some(RefNode::Identifier(identifier)) = unwrap_node!(inst, Identifier) {
                if let Some(submodule_name) = get_identifier_str(tree, identifier) {
                    res.insert(submodule_name.to_string());
                }
            }
        }
    }
    Ok(res)
}

pub fn get_module_name_from_instantiation(
    tree: &SyntaxTree,
    ref_node: &ModuleInstantiation,
) -> Option<String> {
    if let Some(RefNode::ModuleIdentifier(identifier)) = unwrap_node!(ref_node, ModuleIdentifier) {
        get_identifier_str(tree, &identifier.nodes.0).map(String::from)
    } else {
        None
    }
}

pub fn get_dut_name_from_instantiation(
    tree: &SyntaxTree,
    ref_node: &ModuleInstantiation,
) -> Option<String> {
    if let Some(RefNode::HierarchicalInstance(instance)) =
        unwrap_node!(ref_node, HierarchicalInstance)
    {
        if let Some(RefNode::NameOfInstance(identifier)) = unwrap_node!(instance, NameOfInstance) {
            get_identifier_str(tree, &identifier.nodes.0.nodes.0).map(String::from)
        } else {
            None
        }
    } else {
        None
    }
}

/// Return port connections, port -> List[identifiers]
pub fn get_port_connections(
    tree: &SyntaxTree,
    ref_node: &ModuleInstantiation,
) -> HashMap<NodeID, Vec<NodeID>> {
    let mut res: HashMap<NodeID, Vec<NodeID>> = HashMap::new();

    for node in ref_node {
        if let RefNode::NamedPortConnectionIdentifier(conn) = node {
            // let port_identifier = &conn.nodes.2;
            // let port_name: NodeID = get_identifier_str(tree, &port_identifier.nodes.0).unwrap();
            let ident_ref_node = unwrap_node!(conn, Identifier).unwrap();
            let port_name = NodeID::new_identifier(tree, ident_ref_node).unwrap();
            let right_values =
                if let Some(RefNode::Expression(expr)) = unwrap_node!(conn, Expression) {
                    get_identifiers_from_expression(expr)
                } else {
                    vec![]
                };
            let right_values: Vec<NodeID> = right_values
                .into_iter()
                .filter_map(|x| {
                    if let RefNode::Identifier(_) = x {
                        Some(NodeID::new_identifier(tree, x).unwrap())
                    } else if let RefNode::PrimaryLiteral(_) = x {
                        Some(NodeID::new_literal(tree, x).unwrap())
                    } else {
                        None
                    }
                })
                .collect();
            res.insert(port_name, right_values);
        }
    }

    // if the port is connected with `.port_name,` there is no named connections.
    // in this condition, it simply uses the same variable with the port.
    // so we add itself to the vec,
    let ports = res.keys().cloned().collect::<Vec<NodeID>>();
    ports.iter().for_each(|port| {
        let vars = res.get_mut(port).unwrap();
        if vars.is_empty() {
            vars.push(port.clone());
        }
    });
    res
}

pub fn collect_locates(ref_node: RefNode) -> Vec<Locate> {
    ref_node
        .into_iter()
        .filter_map(|node| {
            if let RefNode::Locate(locate) = node {
                Some(locate.clone())
            } else {
                None
            }
        })
        .collect()
}

pub fn merge_locates(locates: &mut [Locate]) -> Option<Locate> {
    locates.sort_by_key(|loc| loc.offset);
    locates.iter().fold(None, |acc, locate| {
        if let Some(acc) = acc {
            if acc.offset + acc.len != locate.offset {
                None
            } else {
                Some(Locate {
                    offset: acc.offset,
                    line: acc.line,
                    len: locate.len + acc.len,
                })
            }
        } else {
            Some(locate.clone())
        }
    })
}

fn get_module_name_with_line_from_text(
    contents: &str,
    module_regex: Regex,
) -> Option<(String, u32)> {
    for (line_number, line) in contents.lines().enumerate() {
        if let Some(captures) = module_regex.captures(&line) {
            let module_name = &captures[1];
            return Some((module_name.to_string(), line_number as u32));
        }
    }
    None
}

#[deprecated]
pub fn parse_line_offset<P: AsRef<Path>, U: AsRef<Path>, V: BuildHasher>(
    module_path: P,
    defines: &Defines<V>,
    includes: &[U],
) -> anyhow::Result<LineOffset> {
    let contents = fs::read_to_string(&module_path)?;
    let module_regex = Regex::new(r"^(\s*module\s+\w+)")?;
    let mod_lines = get_module_name_with_line_from_text(&contents, module_regex);
    if mod_lines.is_none() {
        warn!(
            "There is no module named in '{}'",
            module_path.as_ref().display()
        );
        return Ok(0);
    }
    let (pre_text, _) = preprocess(module_path.as_ref(), defines, includes, false, false)?;

    let res = mod_lines.and_then(|(mod_name, lineno)| {
        let module_regex = Regex::new(&format!("^({})", mod_name)).unwrap();
        let sec_lines = get_module_name_with_line_from_text(pre_text.text(), module_regex);
        sec_lines.and_then(|(_, sec_lineno)| Some(sec_lineno - lineno))
    });
    res.ok_or_else(|| anyhow!("Failed to parse module name"))
}

pub fn get_pos_from_offset(src: &str, print_pos: usize) -> Option<(usize, usize)> {
    let mut pos = 0;
    let mut row = 1;
    let mut last_lf = None;
    while pos < src.len() {
        if src.as_bytes()[pos] == CHAR_LF {
            row += 1;
            last_lf = Some(pos);
        }

        if print_pos == pos {
            let column = if let Some(last_lf) = last_lf {
                pos - last_lf
            } else {
                pos + 1
            };
            let mut next_crlf = pos;
            while next_crlf < src.len() {
                if src.as_bytes()[next_crlf] == CHAR_CR || src.as_bytes()[next_crlf] == CHAR_LF {
                    break;
                }
                next_crlf += 1;
            }

            return Some((column, row));
        }

        pos += 1;
    }
    None
}

pub fn get_node_ast_range<'a, T: Into<RefNodes<'a>>>(nodes: T) -> Option<AstRange> {
    let mut beg = None;
    let mut end = 0;
    for n in Iter::new(nodes.into()) {
        if let RefNode::Locate(x) = n {
            if beg.is_none() {
                beg = Some(x.offset);
            }
            end = x.offset + x.len;
        }
    }
    beg.map(|beg| AstRange {
        offset: beg,
        len: end - beg,
    })
}

pub fn get_last_ast_locate_from_tree(tree: &SyntaxTree) -> Option<Locate> {
    let mut last_locate = None;
    for node in tree.into_iter() {
        match node {
            RefNode::Locate(locate) => {
                if last_locate.is_none() {
                    last_locate = Some(locate.clone());
                } else {
                    last_locate.as_mut().map(|v| *v = locate.clone());
                }
            }
            _ => continue,
        }
    }
    last_locate
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use sv_parser::{parse_sv, unwrap_locate};

    fn get_syntax_tree(path: &str) -> SyntaxTree {
        let defines = vec![("RVFI".to_string(), None)]
            .into_iter()
            .collect::<HashMap<_, _>>();
        let includes: Vec<PathBuf> = vec![
            "/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/ip/prim/rtl/".into(),
            "/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/dv/sv/dv_utils".into(),
        ];
        let (tree, _) = parse_sv(&path, &defines, &includes, false, false).unwrap();
        tree
    }

    #[test]
    fn test_parse_line_offset() {
        let path = "/home/lzz/exp_wkdir/ibex_test/ibex/rtl/ibex_if_stage.sv";
        let defines = vec![("RVFI".to_string(), None)]
            .into_iter()
            .collect::<HashMap<_, _>>();
        let includes: Vec<PathBuf> = vec![
            "/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/ip/prim/rtl/".into(),
            "/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/dv/sv/dv_utils".into(),
        ];
        let res = parse_line_offset(path, &defines, &includes).unwrap();
        assert_eq!(res, 517)
    }

    #[test]
    fn test_get_submodules() {
        let path = "/home/lzz/exp_wkdir/ibex_test/ibex/rtl/ibex_ex_block.sv";
        let tree = get_syntax_tree(path);
        let submodules = get_submodules(&tree).unwrap();
        println!("{:?}", submodules);
    }

    #[test]
    fn test_get_module_name() {
        let path = "/home/lzz/exp_wkdir/ibex_test/ibex/rtl/ibex_ex_block.sv";
        let tree = get_syntax_tree(path);
        for node in &tree {
            if let RefNode::ModuleInstantiation(inst) = node {
                let module_name = get_module_name_from_instantiation(&tree, &inst);
                println!("submodule_name: {:?}", module_name);
            }
        }
    }

    #[test]
    fn test_get_dut_name() {
        let path = "/home/lzz/exp_wkdir/ibex_test/ibex/rtl/ibex_ex_block.sv";
        let tree = get_syntax_tree(path);
        for node in &tree {
            if let RefNode::ModuleInstantiation(inst) = node {
                {
                    let module_name = get_dut_name_from_instantiation(&tree, &inst);
                    println!("submodule dut name: {:?}", module_name);
                }
            }
        }
    }

    #[test]
    fn test_get_port_connections() {
        let path = "/home/lzz/exp_wkdir/ibex_test/ibex/rtl/ibex_ex_block.sv";
        let tree = get_syntax_tree(path);
        for node in &tree {
            if let RefNode::ModuleInstantiation(inst) = node {
                {
                    let connections = get_port_connections(&tree, &inst);
                    println!("connections: {:#?}", connections);
                }
            }
        }
    }

    #[test]
    fn test_get_port_connections_not_named() {
        let path = "/home/lzz/exp_wkdir/ibex_test/ibex/rtl/ibex_top_tracing.sv";
        let tree = get_syntax_tree(path);
        for node in &tree {
            if let RefNode::ModuleInstantiation(inst) = node {
                {
                    let connections = get_port_connections(&tree, &inst);
                    println!("connections: {:#?}", connections);
                }
            }
        }
    }

    #[test]
    fn test_get_original_lineno() -> anyhow::Result<()> {
        let path = "/home/lzz/exp_wkdir/ibex_test/ibex/rtl/ibex_if_stage.sv";
        let defines = vec![("RVFI".to_string(), None)]
            .into_iter()
            .collect::<HashMap<_, _>>();
        let includes: Vec<PathBuf> = vec![
            "/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/ip/prim/rtl/".into(),
            "/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/dv/sv/dv_utils".into(),
        ];

        let (tree, _) = parse_sv(path, &defines, &includes, false, true)
            .expect(format!("Failed to parse {:?}", path).as_str());

        let src_content = fs::read_to_string(path)?;
        println!("src_content len: {}", src_content.len());

        for node in &tree {
            match node {
                RefNode::Identifier(identifer) => {
                    let name = get_identifier_str(&tree, &identifer);
                    if let Some(name) = name {
                        if name == "pc_id_o" {
                            let locate = unwrap_locate!(node).unwrap();
                            let loc = tree.get_origin(locate);
                            println!("{:?}", loc);
                            if let Some((_, offset)) = loc {
                                let name = &src_content[offset..offset + locate.len];
                                println!(
                                    "name: {}, lineno: {:?}",
                                    name,
                                    get_pos_from_offset(&src_content, offset)
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }
}
