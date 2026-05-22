use crate::coverage::LineCoverage;
use crate::{BlockType, CoverageTracker, TimeAnnotation};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs::read;
use std::path::Path;

#[derive(Clone)]
pub struct ParameterCoverageReport {
    // (module_name -> covered lines)
    lines_coverage: HashMap<String, Vec<LineCoverage>>,
}

impl ParameterCoverageReport {
    pub fn new<P: AsRef<Path>>(json_path: P) -> Self {
        let ctx = read(&json_path)
            .expect(format!("Unable to read json file {}. Please generate parameter removed AST using verilator our provided.", json_path.as_ref().display()).as_str());
        let json_data: Value = serde_json::from_slice(&ctx).unwrap();

        let param_lines_coverage = json_data
            .as_object()
            .and_then(|map| {
                map.get("modulesp")
                    .and_then(|modules| modules.as_array())
                    .and_then(|modules| {
                        let res = modules
                            .into_iter()
                            .filter_map(|module| {
                                module.as_object().and_then(|map| {
                                    let ret = map
                                        .get("type")
                                        .and_then(|ty| ty.as_str())
                                        .and_then(|ty| Some(ty == "MODULE"));
                                    let ret = ret.unwrap_or(false);
                                    if ret {
                                        Some(map)
                                    } else {
                                        None
                                    }
                                })
                            })
                            .filter_map(|module| Self::parse_module(module))
                            // .flatten()
                            .collect::<Vec<_>>();
                        Some(res)
                    })
            })
            .map(|lines_coverage| lines_coverage.into_iter().collect::<HashMap<_, _>>())
            .unwrap_or_default();

        let param_lines_coverage = param_lines_coverage
            .into_iter()
            // .collect::<Vec<_>>()
            .collect::<HashMap<_, _>>();

        Self {
            lines_coverage: param_lines_coverage,
        }
    }

    fn parse_module(map: &Map<String, Value>) -> Option<(String, Vec<LineCoverage>)> {
        let coverages = Self::collect_stmts_from_array(map.get("stmtsp"))?;

        let origin_name = map.get("origName").and_then(|name| name.as_str())?;
        let dead = map.get("dead").and_then(|d| d.as_bool())?;

        (!dead).then(|| (origin_name.to_string(), coverages))
    }

    fn parse_stmt(stmt: &Value) -> Option<Vec<LineCoverage>> {
        stmt.as_object().map(|map| {
            // Collect any sub-statements
            let mut results = map
                .iter()
                .map(|(_, v)| Self::collect_stmts_from_array(Some(v)).unwrap_or_default())
                .flatten()
                .collect::<Vec<_>>();

            // Process the current statement if it's an assignment
            if let Some(cur_stmts) = Self::process_assignment(map) {
                results.extend(cur_stmts);
            }

            results
        })
    }

    // Helper function to collect and flatten statements from an array
    fn collect_stmts_from_array(value: Option<&Value>) -> Option<Vec<LineCoverage>> {
        value.and_then(|v| v.as_array()).map(|stmts| {
            stmts
                .iter()
                .filter_map(|stmt| Self::parse_stmt(stmt))
                .flatten()
                .collect()
        })
    }

    // Helper function to process assignment statements
    fn process_assignment(map: &Map<String, Value>) -> Option<Vec<LineCoverage>> {
        map.get("type")
            .and_then(|typ| typ.as_str())
            .filter(|typ| *typ == "ASSIGNW" || *typ == "ASSIGN")
            .and_then(|_| {
                map.get("lhsp")
                    .and_then(|lhs| lhs.as_array())
                    .map(|lhs_array| {
                        lhs_array
                            .iter()
                            .filter_map(|left_hand| {
                                left_hand
                                    .as_object()
                                    .and_then(|obj| obj.get("loc"))
                                    .and_then(|loc| loc.as_str())
                                    .map(|loc_str| Self::parse_location(loc_str))
                            })
                            .flatten()
                            .collect()
                    })
            })
    }

    // Helper function to parse location string into LineCoverage objects
    fn parse_location(loc: &str) -> Vec<LineCoverage> {
        let tokens: Vec<&str> = loc.split(',').collect();
        assert_eq!(tokens.len(), 3);

        let parse_line_number = |token: &str| {
            token
                .split(':')
                .next()
                .and_then(|num| num.parse::<u32>().ok())
        };

        if let (Some(line_start), Some(line_end)) =
            (parse_line_number(tokens[1]), parse_line_number(tokens[2]))
        {
            (line_start..=line_end)
                .map(|lineno| LineCoverage::new("", "", 0, lineno, lineno, 1))
                .collect()
        } else {
            Vec::new()
        }
    }

    pub fn has_module_data(&self, module_name: &str) -> bool {
        self.lines_coverage.contains_key(module_name)
    }

    pub fn check_covered(&self, module_name: Option<&str>, lineno: u32) -> Option<usize> {
        let ret = module_name.and_then(|module_name| {
            self.lines_coverage
                .get(module_name)
                .and_then(|line_coverage| {
                    line_coverage
                        .iter()
                        .find(|&line_coverage| line_coverage.line == lineno)
                        .map(|line_coverage| line_coverage.count)
                })
        });
        ret
    }
}

impl CoverageTracker for ParameterCoverageReport {
    fn check_line_covered(
        &self,
        _btype: Option<BlockType>,
        _scope_name: Option<&str>,
        module_name: Option<&str>,
        _time: Option<TimeAnnotation>,
        lineno: u32,
    ) -> Option<usize> {
        self.check_covered(module_name, lineno)
    }

    fn get_covered_module_files(&self) -> Vec<String> {
        self.lines_coverage.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_ex_block_rm_param() {
        let file_path = "tests/test_files/ex_block_rm_params.tree.json";
        let top_module = "t_cover_param";
        let report = ParameterCoverageReport::new(file_path);
        let lines_coverages = report.lines_coverage;
        let module_name = "t_cover_param";
        let lines = vec![77, 79];
        assert_eq!(lines_coverages.len(), 1);
        assert!(lines_coverages.contains_key(&module_name.to_string()));
        let reports = lines_coverages.get(module_name).unwrap();
        assert_eq!(reports.len(), lines.len());
        reports.iter().for_each(|report| {
            let lineno = report.line;
            assert!(lines.contains(&lineno));
        })
    }

    #[test]
    fn test_param_example() {
        let file_path = "tests/test_files/param_example_rm_params.tree.json";
        let top_module = "t_cover_param";
        let report = ParameterCoverageReport::new(file_path);
        let lines_coverages = report.lines_coverage;
        let module_name = "t_cover_param";
        let lines = vec![11, 17, 19, 22];
        println!("{:#?}", lines_coverages);

        assert_eq!(lines_coverages.len(), 1);
        assert!(lines_coverages.contains_key(&module_name.to_string()));
        let reports = lines_coverages.get(module_name).unwrap();
        assert_eq!(reports.len(), lines.len());
        reports.iter().for_each(|report| {
            let lineno = report.line;
            assert!(lines.contains(&lineno));
        })
    }
}
