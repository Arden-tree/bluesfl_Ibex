use crate::block::{AstRange, ScopeBlocks};
use crate::{
    get_last_ast_locate_from_tree, get_module_name, get_pos_from_offset, save_data_to_json, Block,
    BlockParser, BlockType,
};
use rayon::iter::ParallelIterator;
use rayon::prelude::IntoParallelRefIterator;
use serde_json::json;
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use sv_parser::{parse_sv, Define, Locate, SyntaxTree};

pub struct BlockManager<'a, Parser: BlockParser> {
    // (scope_name, blocks, tree)
    scope_block_mapping: ScopeBlocks<'a, Parser>,
    // (module_name, code_context)
    module_code_mapping: HashMap<String, String>,
    scope_max_line: HashMap<String, usize>,
    scope_module_name: HashMap<String, String>,
}

impl<'a, Parser> BlockManager<'a, Parser>
where
    Parser: BlockParser,
{
    pub fn new(
        module_tree_mapping: HashMap<String, Arc<SyntaxTree>>,
        module_code_mapping: HashMap<String, String>,
        top_module: &str,
        top_scope: &str,
        parser: Parser,
    ) -> Self {
        let block_mapping = parser.parse(
            module_tree_mapping.clone(),
            module_code_mapping.clone(),
            top_module,
            top_scope,
        );
        let mut scope_module_name = HashMap::new();
        let scope_max_line = block_mapping
            .iter()
            .map(|(sc, (blocks, tree))| {
                let last_locate = get_last_ast_locate_from_tree(&tree).unwrap();
                let (_, offset) = tree.get_origin(&last_locate).unwrap();
                let module_name = blocks.first().map(|b| b.get_module_name()).unwrap();
                let code_content = module_code_mapping.get(module_name).unwrap().to_string();
                let origin_max_lineno = get_pos_from_offset(&code_content, offset)
                    .map(|(_col, row)| row)
                    .unwrap();
                scope_module_name.insert(sc.to_string(), module_name.to_string());
                (sc.to_string(), origin_max_lineno)
            })
            .collect::<HashMap<_, _>>();
        Self {
            scope_block_mapping: block_mapping,
            module_code_mapping,
            scope_max_line,
            scope_module_name,
        }
    }

    pub fn get_scopes(&self) -> Vec<&str> {
        self.scope_block_mapping
            .keys()
            .map(|s| s.as_str())
            .collect::<Vec<&str>>()
    }

    pub fn get_scope_blocks(
        &self,
        scope_name: &str,
    ) -> Option<&(Vec<<Parser as BlockParser>::Block<'a>>, Arc<SyntaxTree>)> {
        self.scope_block_mapping.get(scope_name)
    }

    pub fn get_scope_max_lineno(&self, scope_name: &str) -> Option<usize> {
        self.scope_max_line.get(scope_name).copied()
    }

    /// Note that we assume there is only one module instantiation in a file.
    pub fn get_scope_module_name(&self, scope_name: &str) -> Option<String> {
        self.scope_module_name.get(scope_name).cloned()
    }

    pub fn get_driven_block(&self, scope_name: &str, signal: &str) -> Option<Parser::Block<'a>> {
        let (blocks, _) = self.scope_block_mapping.get(scope_name)?;
        for blk in blocks {
            if blk
                .get_output_nodes()
                .into_iter()
                .map(|node| node.get_text())
                .filter(|s| *s == signal)
                .count()
                > 0
            {
                return Some(blk.clone());
            }
        }
        None
    }

    pub fn get_block_by_bid(&self, bid: u64) -> Option<Parser::Block<'a>> {
        self.scope_block_mapping
            .values()
            .map(|(vars, _)| vars)
            .flatten()
            .find(|block| block.get_bid() == bid)
            .cloned()
    }

    pub fn get_block_snippet<S: AsRef<str>>(
        &self,
        scope_name: &str,
        signals: &[S],
    ) -> Vec<(String, u64, String)> {
        signals
            .iter()
            .filter_map(|signal| {
                if let Some(blk) = self.get_driven_block(&scope_name, &signal.as_ref()) {
                    Some((signal.as_ref().to_owned(), blk))
                } else {
                    None
                }
            })
            .map(|(signal, block)| (signal, block.get_bid(), block.get_ctx().join("\n")))
            .collect::<Vec<_>>()
    }

    pub fn get_modules_covered(&self) -> String {
        let mod_names = self
            .scope_block_mapping
            .keys()
            .cloned()
            // TODO: remove this
            .filter(|name| name == "ibex_alu")
            .collect::<Vec<_>>();
        format!("{:?}", mod_names)
    }

    pub fn get_signal_values(&self, _bid: u64, _time: u64) -> String {
        todo!();
    }

    pub fn get_suspicious_ports(&self, scope_name: &str) -> String {
        self.scope_block_mapping
            .get(scope_name)
            .map(|(blocks, _)| {
                blocks
                    .iter()
                    .find_map(|block| {
                        if matches!(block.get_block_type(), &BlockType::ModuleOutput) {
                            Some(block.get_suspicious_trace().map_or(vec![], |trace| {
                                (&trace.1).iter().map(|(node, _t)| node).collect::<Vec<_>>()
                            }))
                        } else {
                            None
                        }
                    })
                    .map_or_else(Vec::new, |vars| {
                        vars.into_iter()
                            .map(|node| node.get_text().to_string())
                            .collect::<Vec<String>>()
                    })
            })
            .map_or_else(|| String::new(), |res| format!("{:?}", res))
    }

    pub fn get_port_blocks(&self, scope_name: &str) -> Vec<Parser::Block<'a>> {
        self.scope_block_mapping
            .get(scope_name)
            .map(|(blocks, _)| {
                blocks
                    .iter()
                    .filter(|block| {
                        matches!(
                            block.get_block_type(),
                            &BlockType::ModuleOutput | &BlockType::ModuleInput
                        )
                    })
                    .map(|block| block.clone())
                    .collect::<Vec<_>>()
            })
            .map_or_else(|| vec![], |res| res)
    }

    pub fn get_original_lineno_from_ast_locate(
        &self,
        scope_name: &str,
        module_name: &str,
        ast_locate: Locate,
    ) -> Option<usize> {
        let (_, tree) = self.scope_block_mapping.get(scope_name)?;
        let (_, offset) = tree.get_origin(&ast_locate)?;
        let code_content = self.module_code_mapping.get(module_name)?.to_string();
        get_pos_from_offset(&code_content, offset).map(|(_col, row)| row)
    }

    pub fn get_belong_block_from_ast_locate(
        &self,
        scope_name: &str,
        ast_locate: &Locate,
    ) -> Option<Parser::Block<'a>> {
        self.scope_block_mapping
            .get(scope_name)
            .and_then(|(blocks, _)| {
                blocks
                    .iter()
                    .filter(|block| {
                        let cnt = block
                            .get_ast_covered_ranges()
                            .iter()
                            .filter(|ast_range| {
                                ast_range
                                    .check_cover(&AstRange::new(ast_locate.offset, ast_locate.len))
                            })
                            .count();
                        cnt == 1
                    })
                    .nth(0)
                    .cloned()
            })
    }

    pub fn get_belong_block_from_ast_lineno(
        &self,
        scope_name: &str,
        ast_lineno: u32,
    ) -> Option<Parser::Block<'a>> {
        self.scope_block_mapping
            .get(scope_name)
            .and_then(|(blocks, _)| {
                blocks
                    .iter()
                    .filter(|block| block.get_covered_ast_lines().contains(&ast_lineno))
                    .nth(0)
                    .cloned()
            })
    }

    pub fn get_belong_block_from_original_lineno(
        &self,
        scope_name: &str,
        original_lineno: u32,
    ) -> Option<Parser::Block<'a>> {
        self.scope_block_mapping
            .get(scope_name)
            .and_then(|(blocks, _)| {
                blocks
                    .iter()
                    .filter(|block| {
                        block
                            .get_covered_original_lines()
                            .contains(&original_lineno)
                    })
                    .nth(0)
                    .cloned()
            })
    }

    pub fn dump_blocks_distribution(&self, output_path: &str) -> Result<(), Box<dyn Error>> {
        let data = self
            .scope_block_mapping
            .iter()
            .map(|(scope, (blocks, _))| {
                blocks.iter().map(|block| {
                    let block_size = block.get_ctx().join("\n").split('\n').count();
                    json!({
                        "bid": block.get_bid(),
                        "scope": scope.clone(),
                        "btype": block.get_block_type(),
                        "block_size": block_size
                    })
                })
            })
            .flatten()
            .collect::<Vec<_>>();
        save_data_to_json(&data, &format!("{}/blocks.json", output_path))?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct SendSyntaxTree(pub SyntaxTree);

unsafe impl Send for SendSyntaxTree {}

impl AsRef<SyntaxTree> for SendSyntaxTree {
    fn as_ref(&self) -> &SyntaxTree {
        &self.0
    }
}

pub fn get_block_manager<'a, P, I, Parser>(
    mod_files: I,
    defines: &HashMap<String, Option<Define>>,
    includes: &Vec<PathBuf>,
    top_module: &str,
    top_scope: &str,
    parser: Parser,
) -> BlockManager<'a, Parser>
where
    Parser: BlockParser,
    P: AsRef<Path>,
    I: IntoIterator<Item = P>,
{
    let mod_files = mod_files
        .into_iter()
        .map(|p| p.as_ref().to_path_buf())
        .collect::<Vec<_>>();
    // Single pass: parse each file once and collect both syntax trees and source code
    let parse_start = std::time::Instant::now();
    eprintln!("[PERF] Starting parse_sv for {} files...", mod_files.len());
    let parsed = mod_files
        .par_iter()
        .filter_map(|path| {
            let file_parse_start = std::time::Instant::now();
            let result = parse_sv(path, defines, includes, false, false);
            let file_parse_elapsed = file_parse_start.elapsed();
            match result {
                Ok((tree, _)) => {
                    let module_name = get_module_name(&tree);
                    if let Some(module_name) = module_name {
                        eprintln!("[PERF] parse_sv {:?} OK in {:.2}s", path.file_name().unwrap(), file_parse_elapsed.as_secs_f64());
                        let code_content = fs::read_to_string(path).ok();
                        Some((module_name, Arc::new(tree), code_content))
                    } else {
                        None
                    }
                }
                Err(e) => {
                    eprintln!("[PERF] parse_sv {:?} FAILED in {:.2}s: {}", path.file_name().unwrap(), file_parse_elapsed.as_secs_f64(), e);
                    None
                }
            }
        })
        .collect::<Vec<_>>();
    let mappings = parsed
        .iter()
        .map(|(name, tree, _)| (name.clone(), Arc::clone(tree)))
        .collect::<HashMap<_, _>>();

    let file_code_mapping = parsed
        .into_iter()
        .filter_map(|(name, _, code)| code.map(|content| (name, content)))
        .collect::<HashMap<_, _>>();
    eprintln!("[PERF] parse_sv total: {:.2}s for {} files", parse_start.elapsed().as_secs_f64(), mappings.len());
    let block_mgr_start = std::time::Instant::now();
    let mgr = BlockManager::new(mappings, file_code_mapping, top_module, top_scope, parser);
    eprintln!("[PERF] BlockManager::new (dataflow analysis): {:.2}s", block_mgr_start.elapsed().as_secs_f64());
    mgr
}

#[cfg(test)]
mod tests {
    use crate::block::mgr::SendSyntaxTree;
    use crate::get_module_files;
    use rayon::prelude::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use sv_parser::parse_sv;

    #[test]
    fn test_rayon() {
        let project_path = "/home/lzz/exp_wkdir/ibex_test/ibex/rtl";
        let mod_files = get_module_files(project_path);
        let res = mod_files
            .par_iter()
            .map(|path| {
                let defines = vec![("RVFI".to_string(), None)]
                    .into_iter()
                    .collect::<HashMap<_, _>>();
                let includes: Vec<PathBuf> = vec![
                    "/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/ip/prim/rtl/".into(),
                    "/home/lzz/exp_wkdir/ibex_test/ibex/vendor/lowrisc_ip/dv/sv/dv_utils".into(),
                ];
                let (tree, _) = parse_sv(&path, &defines, &includes, false, false)
                    .expect(format!("Failed to parse {:?}", path).as_str());
                SendSyntaxTree(tree)
            })
            .collect::<Vec<_>>();
        println!("{}", res.len());
    }
}
