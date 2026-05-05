use crate::spectrum::matrix::SpectrumMetric;
use crate::spectrum::sampler::CoverageSampler;
use crate::{
    Block, BlockManager, BlockParser, BugIDType, LocalizationChoiceBuilder, LocalizationResult,
    LocalizationResultBuilder, Localizer, NodeID, TimeAnnotation,
};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::ops::RangeInclusive;
use sv_parser::Locate;

pub mod matrix;
pub mod sampler;

/// input:
/// - suspicious signal at T (s, t)
/// - waveform: covered blocks at T
/// - sample N
/// output:
/// - matrix: N * linesTotal
/// - a{np}: M * e, then sum col(i) [1-x]
/// - a{nf}: M * (1-e), then sum col(i) [1-x]
/// - a{ep}: M * e, then sum col(i) x
/// - a{ef}: M * (1-e), then sum col(i) x

pub struct SBFLocalizer<'a, 'b, Parser, CS>
where
    Parser: BlockParser,
    CS: CoverageSampler + Send,
{
    bug_id: BugIDType,
    pub block_manager: &'b BlockManager<'a, Parser>,
    coverage_sampler: CS,
    spectrum_metric: SpectrumMetric,
    // lineno range, lineno in original file.
    bid_range: Vec<(u64, (String, RangeInclusive<usize>))>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SBFLInfo {
    score: f64,
}

impl<'a, 'b, Parser, CS> SBFLocalizer<'a, 'b, Parser, CS>
where
    Parser: BlockParser,
    CS: CoverageSampler + Send,
{
    pub fn new(
        bug_id: BugIDType,
        block_manager: &'b BlockManager<'a, Parser>,
        coverage_sampler: CS,
        spectrum_metric: SpectrumMetric,
    ) -> Self {
        let mut bid_range = Vec::new();
        block_manager
            .get_scopes()
            .iter()
            .map(|scope_name| block_manager.get_scope_blocks(scope_name))
            .flatten()
            .for_each(|(blocks, _)| {
                blocks.iter().for_each(|block| {
                    let ast_locates = block.get_covered_line_locates();
                    let lines: Vec<_> = ast_locates
                        .into_iter()
                        .filter_map(|locate| {
                            let ast_locate = Locate {
                                offset: locate.offset,
                                len: locate.len,
                                line: locate.line,
                            };
                            let lineno = block_manager.get_original_lineno_from_ast_locate(
                                block.get_scope(),
                                block.get_module_name(),
                                ast_locate,
                            );
                            if lineno.is_none() {
                                // FIXME: why some ast locate are missing?
                            }
                            // here lineno is the original file lineno.
                            lineno
                        })
                        .collect();

                    let ll = lines.iter().min().cloned().unwrap_or(0);
                    let rr = lines.iter().max().cloned().unwrap_or(0);
                    bid_range.push((block.get_bid(), (block.get_scope().to_string(), (ll..=rr))));
                })
            });

        Self {
            bug_id,
            block_manager,
            coverage_sampler,
            spectrum_metric,
            bid_range,
        }
    }

    // (lineno, scope), score
    pub fn localize(&self) -> Vec<((usize, String), f64)> {
        let spectrum_mats = self.coverage_sampler.sample();
        let mut loc_res = spectrum_mats
            .into_iter()
            .map(|(scope_name, spectrum_matrix)| {
                spectrum_matrix
                    .get_ranked_statements(self.spectrum_metric)
                    .into_iter()
                    .map(move |(lineno, score)| ((lineno, scope_name.clone()), score))
            })
            .flatten()
            .collect::<Vec<_>>();
        loc_res.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        loc_res
    }

    // (block, lineno, scope), score
    fn localize_blocks(&self) -> Vec<((Parser::Block<'a>, usize, String), f64)> {
        let global_loc_res = self.localize();

        let final_all_blocks = global_loc_res
            .into_iter()
            .map(|((lineno, scope_name), score)| {
                // convert lineno in original file to block
                let block = self
                    .bid_range
                    .iter()
                    .filter_map(|(bid, (s_name, range))| {
                        if range.contains(&lineno) && scope_name == *s_name {
                            Some(bid)
                        } else {
                            None
                        }
                    })
                    .map(|bid| {
                        self.block_manager
                            .get_block_by_bid(*bid)
                            .expect("Unknown bid")
                    })
                    .collect::<Vec<_>>()
                    .first()
                    .cloned();

                if block.is_some() {
                    Some(((block.unwrap(), lineno, scope_name), score))
                } else {
                    None
                }
            })
            .flatten()
            .collect::<Vec<_>>();
        final_all_blocks
    }
}

impl<'a, 'b, T, Parser, CS> Localizer<'a, T> for SBFLocalizer<'a, 'b, Parser, CS>
where
    Parser: BlockParser<Block<'a> = T>,
    CS: CoverageSampler + Send,
    T: Block<'a> + Sync + Send + Debug + 'static,
{
    fn get_bug_id(&self) -> BugIDType {
        self.bug_id.clone()
    }

    fn get_localized_modules(&self) -> Vec<(String, Option<(NodeID, Option<TimeAnnotation>)>)> {
        todo!()
    }

    fn get_localized_blocks(&self) -> Vec<(T, Option<(NodeID, Option<TimeAnnotation>)>)> {
        let results = self.localize_blocks();
        results
            .into_iter()
            .map(|((b, _, _), _score)| (b, None))
            .collect()
    }

    fn get_localization_results(&mut self) -> LocalizationResult {
        let results = self.localize_blocks();
        let choices = results
            .into_iter()
            .map(|((block, lineno, _scope), score)| {
                LocalizationChoiceBuilder::default()
                    .score(score)
                    .block_id(block.get_bid())
                    .module_name(block.get_module_name().to_string())
                    .line_number(lineno)
                    .build()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        LocalizationResultBuilder::default()
            .bug_id(self.get_bug_id())
            .choices(choices)
            .build()
            .unwrap()
    }
}
