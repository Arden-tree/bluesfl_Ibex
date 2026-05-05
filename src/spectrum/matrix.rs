use clap::ValueEnum;

/// Represents the spectrum matrix where:
/// - rows represent test cases
/// - columns represent program statements
/// - matrix[i][j] = 1 if statement j is covered by test i, 0 otherwise
pub type CoverageMatrix = Vec<Vec<u8>>;

pub struct SpectrumMatrix {
    pub matrix: CoverageMatrix,
    pub test_results: Vec<bool>,
}

/// Available suspiciousness metrics
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum SpectrumMetric {
    Tarantula,
    Ochiai,
    Jaccard,
    Dstar,
    GP19,
    Barinel,
    Crosstab,
    Zoltar,
    Ample,
}

#[derive(Debug, Clone, Copy)]
pub struct CoverageStats {
    pub a_ef: f64, // executed by failed tests
    pub a_ep: f64, // executed by passed tests
    pub a_nf: f64, // not executed by failed tests
    pub a_np: f64, // not executed by passed tests
}

impl CoverageStats {
    /// Create new coverage stats from counts
    pub fn new(a_ef: f64, a_ep: f64, a_nf: f64, a_np: f64) -> Self {
        Self {
            a_ef,
            a_ep,
            a_nf,
            a_np,
        }
    }
}

impl SpectrumMatrix {
    pub fn calculate_suspiciousness(&self, metric: SpectrumMetric) -> Vec<f64> {
        let num_statements = self.matrix.get(0).map(|row| row.len()).unwrap_or(0);
        let mut scores = Vec::with_capacity(num_statements);

        for statement_idx in 0..num_statements {
            let stats = self.calculate_coverage_stats(statement_idx);
            let score = self.calculate_metric_score(stats, metric);
            scores.push(score);
        }

        scores
    }

    fn calculate_coverage_stats(&self, statement_idx: usize) -> CoverageStats {
        let mut a_ef = 0.0; // executed by failed tests
        let mut a_ep = 0.0; // executed by passed tests
        let mut a_nf = 0.0; // not executed by failed tests
        let mut a_np = 0.0; // not executed by passed tests

        for (test_idx, test_result) in self.test_results.iter().enumerate() {
            if let Some(test_row) = self.matrix.get(test_idx) {
                if let Some(&coverage) = test_row.get(statement_idx) {
                    match (coverage == 1, *test_result) {
                        (true, false) => a_ef += 1.0,  // executed by failed test
                        (true, true) => a_ep += 1.0,   // executed by passed test
                        (false, false) => a_nf += 1.0, // not executed by failed test
                        (false, true) => a_np += 1.0,  // not executed by passed test
                    }
                }
            }
        }

        CoverageStats::new(a_ef, a_ep, a_nf, a_np)
    }

    /// Calculate suspiciousness score using the selected metric
    fn calculate_metric_score(&self, stats: CoverageStats, metric: SpectrumMetric) -> f64 {
        let CoverageStats {
            a_ef,
            a_ep,
            a_nf,
            a_np,
        } = stats;

        match metric {
            SpectrumMetric::Tarantula => {
                let failed_ratio = if a_ef + a_nf > 0.0 {
                    a_ef / (a_ef + a_nf)
                } else {
                    0.0
                };
                let passed_ratio = if a_ep + a_np > 0.0 {
                    a_ep / (a_ep + a_np)
                } else {
                    0.0
                };

                if failed_ratio + passed_ratio > 0.0 {
                    failed_ratio / (failed_ratio + passed_ratio)
                } else {
                    0.0
                }
            }

            SpectrumMetric::Ochiai => {
                let denominator = ((a_ef + a_nf) * (a_ef + a_ep)).sqrt();
                if denominator > 0.0 {
                    a_ef / denominator
                } else {
                    0.0
                }
            }

            SpectrumMetric::Jaccard => {
                let denominator = a_ef + a_nf + a_ep;
                if denominator > 0.0 {
                    a_ef / denominator
                } else {
                    0.0
                }
            }

            SpectrumMetric::Dstar => {
                let star = 2.0; // D* parameter, commonly set to 2
                let denominator = a_ep + a_nf;
                if denominator > 0.0 {
                    a_ef.powf(star) / denominator
                } else if a_ef > 0.0 {
                    f64::INFINITY
                } else {
                    0.0
                }
            }

            SpectrumMetric::GP19 => {
                let total_failed = a_ef + a_nf;
                if total_failed > 0.0 {
                    a_ef * (1.0 + 1.0 / (2.0 * a_ep + a_ef))
                } else {
                    0.0
                }
            }

            SpectrumMetric::Barinel => {
                let h = a_ef + a_ep; // total executions of statement
                let p = a_ef / (a_ef + a_nf).max(1.0); // probability of failure when executed

                if h > 0.0 {
                    1.0 - p
                } else {
                    0.0
                }
            }

            SpectrumMetric::Crosstab => {
                let n11 = a_ef;
                let n10 = a_ep;
                let n01 = a_nf;
                let n00 = a_np;
                let n = n11 + n10 + n01 + n00;

                if n > 0.0 {
                    let expected = (n11 + n10) * (n11 + n01) / n;
                    if expected > 0.0 {
                        (n11 - expected).abs() / expected.sqrt()
                    } else {
                        0.0
                    }
                } else {
                    0.0
                }
            }

            SpectrumMetric::Zoltar => {
                let denominator = a_ef + a_nf + a_ep + (10000.0 * a_nf * a_ep / a_ef);
                if denominator > 0.0 && a_ef > 0.0 {
                    a_ef / denominator
                } else {
                    0.0
                }
            }

            SpectrumMetric::Ample => {
                let total_failed = a_ef + a_nf;
                let total_passed = a_ep + a_np;

                if total_failed > 0.0 && total_passed > 0.0 {
                    (a_ef / total_failed - a_ep / total_passed).abs()
                } else if total_failed > 0.0 {
                    a_ef / total_failed
                } else {
                    0.0
                }
            }
        }
    }

    /// Get ranked list of statements by suspiciousness (highest first)
    pub fn get_ranked_statements(&self, metric: SpectrumMetric) -> Vec<(usize, f64)> {
        let scores = self.calculate_suspiciousness(metric);
        let mut ranked: Vec<(usize, f64)> = scores.into_iter().enumerate().collect();

        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked
    }
}

impl SpectrumMatrix {
    pub fn new(matrix: CoverageMatrix, test_results: Vec<bool>) -> Result<Self, String> {
        if matrix.len() != test_results.len() {
            return Err("Number of test results must match number of matrix rows".to_string());
        }

        Ok(Self {
            matrix,
            test_results,
        })
    }

    pub fn num_tests(&self) -> usize {
        self.matrix.len()
    }

    pub fn num_statements(&self) -> usize {
        self.matrix.get(0).map(|row| row.len()).unwrap_or(0)
    }

    pub fn get_coverage(&self, test_idx: usize, statement_idx: usize) -> Option<bool> {
        self.matrix
            .get(test_idx)?
            .get(statement_idx)
            .map(|&val| val == 1)
    }

    pub fn add_test(&mut self, coverage: Vec<u8>, result: bool) -> Result<(), String> {
        if coverage.len() != self.num_statements() && self.num_statements() > 0 {
            return Err("Coverage vector length must match number of statements".to_string());
        }

        self.matrix.push(coverage);
        self.test_results.push(result);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spectrum_matrix_creation() {
        let matrix = vec![vec![1, 0, 1, 0], vec![1, 1, 0, 1], vec![0, 1, 1, 0]];
        let results = vec![false, true, false];

        let spectrum = SpectrumMatrix::new(matrix, results).unwrap();
        assert_eq!(spectrum.num_tests(), 3);
        assert_eq!(spectrum.num_statements(), 4);
    }

    #[test]
    fn test_fault_localization_tarantula() {
        let matrix = vec![
            vec![1, 0, 1, 0], // failed test
            vec![1, 1, 0, 1], // passed test
            vec![0, 1, 1, 0], // failed test
        ];
        let results = vec![false, true, false];
        let spectrum = SpectrumMatrix::new(matrix, results).unwrap();

        let scores = spectrum.calculate_suspiciousness(SpectrumMetric::Tarantula);

        assert_eq!(scores.len(), 4);
        // Statement 0: executed by 1 failed, 1 passed test -> moderate suspiciousness
        // Statement 2: executed by 2 failed, 0 passed tests -> high suspiciousness
        assert!(scores[2] > scores[0]); // Statement 2 should be more suspicious than 0
    }

    #[test]
    fn test_multiple_metrics() {
        let matrix = vec![vec![1, 0, 1], vec![0, 1, 1]];
        let results = vec![false, true];
        let spectrum = SpectrumMatrix::new(matrix, results).unwrap();

        let metrics = vec![
            SpectrumMetric::Tarantula,
            SpectrumMetric::Ochiai,
            SpectrumMetric::Dstar,
            SpectrumMetric::GP19,
        ];

        for metric in metrics {
            let scores = spectrum.calculate_suspiciousness(metric);
            assert_eq!(scores.len(), 3);
            // All scores should be non-negative
            assert!(scores.iter().all(|&s| s >= 0.0 || s.is_infinite()));
        }
    }

    #[test]
    fn test_ranking() {
        let matrix = vec![vec![1, 0, 1, 0], vec![1, 1, 0, 1], vec![0, 1, 1, 0]];
        let results = vec![false, true, false];
        let spectrum = SpectrumMatrix::new(matrix, results).unwrap();

        let ranked = spectrum.get_ranked_statements(SpectrumMetric::Ochiai);

        assert_eq!(ranked.len(), 4);
        // Should be sorted by suspiciousness (descending)
        for i in 1..ranked.len() {
            assert!(ranked[i - 1].1 >= ranked[i].1);
        }
    }

    #[test]
    fn test_all() {
        // Example spectrum matrix:
        // Test 1 (failed): covers statements 0, 2
        // Test 2 (passed): covers statements 0, 1, 3
        // Test 3 (failed): covers statements 1, 2
        let matrix = vec![
            vec![1, 0, 1, 0], // Test 1 (failed)
            vec![1, 1, 0, 1], // Test 2 (passed)
            vec![0, 1, 1, 0], // Test 3 (failed)
        ];
        let test_results = vec![false, true, false]; // false = failed, true = passed

        let spectrum = SpectrumMatrix::new(matrix, test_results).unwrap();

        // Test different metrics
        let metrics = vec![
            ("Tarantula", SpectrumMetric::Tarantula),
            ("Ochiai", SpectrumMetric::Ochiai),
            ("D*", SpectrumMetric::Dstar),
            ("GP19", SpectrumMetric::GP19),
            ("Jaccard", SpectrumMetric::Jaccard),
        ];

        for (name, metric) in metrics {
            println!("\n=== {} Metric ===", name);
            let ranked = spectrum.get_ranked_statements(metric);

            for (rank, (stmt_idx, score)) in ranked.iter().enumerate() {
                println!(
                    "Rank {}: Statement {} -> Suspiciousness: {:.6}",
                    rank + 1,
                    stmt_idx,
                    score
                );
            }
        }
    }

    use rand::Rng;
    use std::time::Instant;

    /// Generate random spectrum matrix for testing
    fn generate_random_spectrum(
        num_tests: usize,
        num_statements: usize,
        coverage_probability: f64,
    ) -> SpectrumMatrix {
        let mut rng = rand::thread_rng();

        // Generate random coverage matrix
        let matrix: CoverageMatrix = (0..num_tests)
            .map(|_| {
                (0..num_statements)
                    .map(|_| {
                        if rng.gen::<f64>() < coverage_probability {
                            1
                        } else {
                            0
                        }
                    })
                    .collect()
            })
            .collect();

        // Generate random test results (more likely to pass than fail)
        let test_results: Vec<bool> = (0..num_tests)
            .map(|_| rng.gen::<f64>() > 0.3) // 70% pass rate
            .collect();

        SpectrumMatrix::new(matrix, test_results).unwrap()
    }

    /// Run performance benchmark
    fn run_benchmark(num_tests: usize, num_statements: usize) {
        println!("\n🔬 Performance Benchmark");
        println!(
            "Matrix size: {} tests × {} statements",
            num_tests, num_statements
        );
        println!("Coverage probability: 60%");

        // Generate random data
        let generation_start = Instant::now();
        let spectrum = generate_random_spectrum(num_tests, num_statements, 0.6);
        let generation_time = generation_start.elapsed();

        println!("Data generation time: {:?}", generation_time);

        // Count actual failed/passed tests
        let failed_tests = spectrum.test_results.iter().filter(|&&r| !r).count();
        let passed_tests = spectrum.test_results.iter().filter(|&&r| r).count();
        println!(
            "Test results: {} failed, {} passed",
            failed_tests, passed_tests
        );

        // Test all metrics with timing
        let metrics = vec![
            ("Tarantula", SpectrumMetric::Tarantula),
            ("Ochiai", SpectrumMetric::Ochiai),
            ("D*", SpectrumMetric::Dstar),
            ("GP19", SpectrumMetric::GP19),
            ("Jaccard", SpectrumMetric::Jaccard),
            ("Barinel", SpectrumMetric::Barinel),
            ("Crosstab", SpectrumMetric::Crosstab),
            ("Zoltar", SpectrumMetric::Zoltar),
            ("Ample", SpectrumMetric::Ample),
        ];

        println!("\n📊 Metric Performance Results:");
        println!(
            "{:<12} {:<15} {:<15} {:<15}",
            "Metric", "Calc Time (μs)", "Rank Time (μs)", "Total (μs)"
        );
        println!("{:-<60}", "");

        for (name, metric) in metrics {
            // Time suspiciousness calculation
            let calc_start = Instant::now();
            let scores = spectrum.calculate_suspiciousness(metric);
            let calc_time = calc_start.elapsed();

            // Time ranking
            let rank_start = Instant::now();
            let _ranked = spectrum.get_ranked_statements(metric);
            let rank_time = rank_start.elapsed();

            let total_time = calc_time + rank_time;

            println!(
                "{:<12} {:<15.2} {:<15.2} {:<15.2}",
                name,
                calc_time.as_micros() as f64,
                rank_time.as_micros() as f64,
                total_time.as_micros() as f64
            );

            // Show top 5 most suspicious statements for first metric only
            if name == "Tarantula" {
                println!("\n🎯 Top 5 Most Suspicious Statements (Tarantula):");
                let ranked = spectrum.get_ranked_statements(metric);
                for (rank, (stmt_idx, score)) in ranked.iter().take(5).enumerate() {
                    println!(
                        "  Rank {}: Statement {} -> Score: {:.6}",
                        rank + 1,
                        stmt_idx,
                        score
                    );
                }
            }
        }
    }

    /// Example usage with timing
    #[test]
    fn test_run_benchmark() {
        println!("🚀 Spectrum-Based Fault Localization Demo\n");

        // Small example with fixed data
        println!("=== Small Example (Fixed Data) ===");
        let matrix = vec![
            vec![1, 0, 1, 0], // Test 1 (failed)
            vec![1, 1, 0, 1], // Test 2 (passed)
            vec![0, 1, 1, 0], // Test 3 (failed)
        ];
        let test_results = vec![false, true, false]; // false = failed, true = passed

        let spectrum = SpectrumMatrix::new(matrix, test_results).unwrap();

        // Test different metrics with timing
        let metrics = vec![
            ("Tarantula", SpectrumMetric::Tarantula),
            ("Ochiai", SpectrumMetric::Ochiai),
            ("D*", SpectrumMetric::Dstar),
            ("GP19", SpectrumMetric::GP19),
            ("Jaccard", SpectrumMetric::Jaccard),
        ];

        for (name, metric) in metrics {
            println!("\n=== {} Metric ===", name);
            let start_time = Instant::now();
            let ranked = spectrum.get_ranked_statements(metric);
            let elapsed_time = start_time.elapsed();

            for (rank, (stmt_idx, score)) in ranked.iter().enumerate() {
                println!(
                    "Rank {}: Statement {} -> Suspiciousness: {:.6}",
                    rank + 1,
                    stmt_idx,
                    score
                );
            }

            println!("⏱️  Time taken: {:?}", elapsed_time);
        }

        println!("\n{}", "=".repeat(60));
        run_benchmark(1000, 20000);

        println!("\n✅ Benchmark completed!");
    }
}
