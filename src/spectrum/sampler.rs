use crate::spectrum::matrix::{CoverageMatrix, SpectrumMatrix};
use crate::{Block, BlockManager, BlockParser, CoverageTracker, TimeAnnotation};

pub trait CoverageSampler {
    // generate SpectrumMatrix for each scope, the column of SpectrumMatrix is lineno at original file.
    fn sample(&self) -> Vec<(String, SpectrumMatrix)>;
}

pub struct IntervalCoverageSampler<'a, 'b, Parser, CT>
where
    Parser: BlockParser,
    CT: CoverageTracker + Send,
{
    interval: TimeAnnotation,
    time_step: TimeAnnotation,
    failed_time: TimeAnnotation,
    coverage_tracker: CT,
    block_manager: &'b BlockManager<'a, Parser>,
}

/// test failed at time T.
/// clk step = 2
/// interval = 2
/// we sample: [..., T - 2 * 2 * (2), T - 2 * 2 * (1), T]
impl<'a, 'b, Parser, CT> IntervalCoverageSampler<'a, 'b, Parser, CT>
where
    Parser: BlockParser,
    CT: CoverageTracker + Send,
{
    pub fn new(
        interval: TimeAnnotation,
        time_step: TimeAnnotation,
        failed_time: TimeAnnotation,
        coverage_tracker: CT,
        block_manager: &'b BlockManager<'a, Parser>,
    ) -> Self {
        Self {
            interval,
            time_step,
            failed_time,
            coverage_tracker,
            block_manager,
        }
    }

    fn generate_interval_sequence(
        t: TimeAnnotation,
        time_step: TimeAnnotation,
        interval: TimeAnnotation,
    ) -> Vec<TimeAnnotation> {
        let step = time_step * interval;
        (0..=t / step.max(1))
            .rev()
            .map(|i| t - step * i)
            .filter(|&x| x >= 0)
            .collect()
    }
}

impl<'a, 'b, Parser, CT> CoverageSampler for IntervalCoverageSampler<'a, 'b, Parser, CT>
where
    Parser: BlockParser,
    CT: CoverageTracker + Send,
{
    fn sample(&self) -> Vec<(String, SpectrumMatrix)> {
        let timestamps =
            Self::generate_interval_sequence(self.failed_time, self.time_step, self.interval);

        assert!(timestamps.last().is_some());
        assert_eq!(*timestamps.last().unwrap(), self.failed_time);

        let test_n = timestamps.len();
        let mut test_results = vec![false; test_n];
        test_results.fill(false);
        test_results.last_mut().map(|v| *v = true);

        self.block_manager
            .get_scopes()
            .iter()
            .map(|scope| {
                let max_lineno = self.block_manager.get_scope_max_lineno(scope).unwrap();

                let mut matrix: CoverageMatrix = (0..test_n)
                    .map(|_| (0..=max_lineno).map(|_| 0).collect())
                    .collect();
                for lineno in 0..=max_lineno {
                    for (timestamp_index, timestamp) in timestamps.iter().enumerate() {
                        let btype = self
                            .block_manager
                            .get_belong_block_from_original_lineno(scope, lineno as u32)
                            .map(|block| block.get_block_type().clone());

                        if btype.is_none() {
                            // warn!("[IntervalCoverageSampler]: cannot find block according to current lineno={} in scope={};", lineno, scope);
                            continue;
                        }
                        let btype = btype.unwrap();

                        let module_name = self.block_manager.get_scope_module_name(scope).unwrap();

                        let value = self
                            .coverage_tracker
                            .check_line_covered(
                                Some(btype),
                                Some(scope),
                                Some(&module_name),
                                Some(*timestamp),
                                lineno as u32,
                            )
                            .map(|v| v > 0)
                            .unwrap_or(false);

                        matrix[timestamp_index][lineno] = value as u8;
                    }
                }

                assert_eq!(test_results.len(), matrix.len());

                let spectrum_matrix = SpectrumMatrix {
                    matrix,
                    test_results: test_results.clone(),
                };
                (scope.to_string(), spectrum_matrix)
            })
            .collect()
    }
}
