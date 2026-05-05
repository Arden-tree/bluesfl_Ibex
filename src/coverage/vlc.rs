#![allow(unused)]
use crate::block::CircuitType;
use crate::coverage::{CoverageTracker, LineCoverage};
use crate::{BlockType, LineOffset, TimeAnnotation};
use log::warn;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display, Formatter};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

const VL_CIK_COLUMN: &'static str = "n";
const VL_CIK_COMMENT: &'static str = "o";
const VL_CIK_FILENAME: &'static str = "f";
const VL_CIK_HIER: &'static str = "h";
const VL_CIK_LINENO: &'static str = "l";
const VL_CIK_LINESCOV: &'static str = "S";
const VL_CIK_PER_INSTANCE: &'static str = "P";
const VL_CIK_THRESH: &'static str = "s";
const VL_CIK_TYPE: &'static str = "t";
const VL_CIK_WEIGHT: &'static str = "w";

#[derive(Clone)]
pub struct VlcPoint {
    name_str: String,
}

impl VlcPoint {
    pub fn new(name_str: &str) -> VlcPoint {
        VlcPoint {
            name_str: name_str.to_string(),
        }
    }

    pub fn filename(&self) -> String {
        self.key_extract(VL_CIK_FILENAME).to_string()
    }

    pub fn comment(&self) -> String {
        self.key_extract(VL_CIK_COMMENT).to_string()
    }

    pub fn r#type(&self) -> String {
        self.key_extract(VL_CIK_TYPE).to_string()
    }

    pub fn thresh(&self) -> String {
        self.key_extract(VL_CIK_THRESH).to_string() // Could return an empty string
    }

    pub fn linescov(&self) -> String {
        self.key_extract(VL_CIK_LINESCOV).to_string()
    }

    pub fn lineno(&self) -> u32 {
        self.key_extract(VL_CIK_LINENO).parse::<u32>().unwrap_or(0) // Default to 0 if parsing fails
    }

    pub fn column(&self) -> u32 {
        self.key_extract(VL_CIK_COLUMN).parse::<u32>().unwrap_or(0) // Default to 0 if parsing fails
    }

    pub fn hierarchy(&self) -> String {
        self.key_extract(VL_CIK_HIER).to_string()
    }

    fn key_extract(&self, short_key: &str) -> &str {
        let short_len = short_key.len();
        let namestr = &self.name_str;

        let mut i = 0;
        while i < namestr.len() {
            // Look for the start marker '\x01'
            if &namestr[i..i + 1] == "\x01" {
                // Check if the following part matches the short_key and ends with '\x02'
                if i + 1 + short_len + 1 < namestr.len()
                    && &namestr[i + 1..i + 1 + short_len] == short_key
                    && &namestr[i + 1 + short_len..i + 2 + short_len] == "\x02"
                {
                    // Skip the start marker, short_key, and end marker
                    i += 1 + short_len + 1; // Skip '\x01' + short_key + '\x02'

                    let mut ep = i;
                    while ep < namestr.len() && &namestr[ep..ep + 1] != "\x01" {
                        ep += 1;
                    }
                    return &namestr[i..ep];
                }
            }
            i += 1;
        }
        ""
    }
}

impl Display for VlcPoint {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "VlcPoint {{\n")?;
        write!(f, "  name_str: {}\n", self.name_str)?;
        write!(f, "  filename: {}\n", self.filename())?;
        write!(f, "  hierarchy: {}\n", self.hierarchy())?;
        write!(f, "  comment: {}\n", self.comment())?;
        write!(f, "  type: {}\n", self.r#type())?;
        write!(f, "  thresh: {}\n", self.thresh())?;
        write!(f, "  linescov: {}\n", self.linescov())?;
        write!(f, "  lineno: {}\n", self.lineno())?;
        write!(f, "  column: {}\n", self.column())?;
        write!(f, "}}")
    }
}

impl Debug for VlcPoint {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct VlcReport {
    pub header: String,
    pub points_coverage: Vec<(VlcPoint, usize)>,
}

/// Note that you must use verilator >= v5.028 to calculate coverage information.
impl VlcReport {
    pub fn new() -> Self {
        Self {
            header: "SystemC::Coverage-3".to_string(),
            points_coverage: Vec::new(),
        }
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        Self::from_reader(reader)
    }

    pub fn from_reader<R: BufRead>(reader: R) -> io::Result<Self> {
        let lines = reader.lines();
        Self::parse_lines(lines)
    }

    fn parse_lines<I>(lines: I) -> io::Result<Self>
    where
        I: Iterator<Item = Result<String, io::Error>>,
    {
        let mut report = Self::new();

        for line_result in lines {
            let line = line_result?;

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if line.starts_with("#") {
                report.header = line[1..].trim().to_string();
            } else if line.starts_with("C") {
                let items = line.split(" ").collect::<Vec<&str>>();
                let name_str = items
                    .iter()
                    .find(|s| s.starts_with("'") && s.ends_with("'"))
                    // remove ''
                    .map(|s| &s[1..s.len() - 1]);
                let count = items
                    .iter()
                    .filter_map(|s| s.parse::<usize>().ok())
                    .collect::<Vec<usize>>();
                match (name_str, count.get(0)) {
                    (Some(name), Some(count)) => {
                        report.points_coverage.push((VlcPoint::new(name), *count));
                    }
                    _ => {}
                }
            }
        }

        Ok(report)
    }
}

impl VlcReport {
    pub fn get_scopes(&self) -> Vec<String> {
        self.points_coverage
            .iter()
            .map(|(report, _)| report.hierarchy().clone())
            .collect::<HashSet<String>>()
            .into_iter()
            .collect::<Vec<String>>()
    }

    fn parse_numbers(input: &str) -> Vec<i32> {
        let mut result = Vec::new();
        if input.is_empty() {
            return result;
        }

        // Split the input string by commas
        let parts = input.split(',');

        for part in parts {
            if let Some(range_pos) = part.find('-') {
                // If there's a range (e.g., 16-19)
                let start: i32 = part[0..range_pos].parse().unwrap();
                let end: i32 = part[range_pos + 1..].parse().unwrap();
                result.extend(start..=end); // Add the range of numbers
            } else {
                // If it's just a single number (e.g., 14 or 22)
                result.push(part.parse().unwrap());
            }
        }

        result
    }

    pub fn get_scope_coverage(&self, scope_name: &str) -> Vec<LineCoverage> {
        self.points_coverage
            .iter()
            .filter(|(report, _)| report.hierarchy() == scope_name)
            .map(|(report, count)| {
                let linescov = report.linescov();
                let numbers = VlcReport::parse_numbers(&linescov);
                // let line_offset = self
                //     .file_line_offsets
                //     .get(&filename)
                //     .cloned()
                //     .unwrap_or(0 as LineOffset);
                // let line_offset = 0;
                numbers.into_iter().map(move |lineno| {
                    LineCoverage::new(
                        &report.filename(),
                        scope_name,
                        // time will be added
                        TimeAnnotation::default(),
                        lineno as u32,
                        lineno as u32,
                        *count,
                    )
                })
            })
            .flatten()
            .collect::<Vec<LineCoverage>>()
    }
}

pub struct VlcCoverageReport {
    time_coverage_mapping_comb: HashMap<TimeAnnotation, VlcReport>,
    time_coverage_mapping_seq: HashMap<TimeAnnotation, VlcReport>,
    lines_coverage_comb: HashMap<(String, TimeAnnotation), Vec<LineCoverage>>,
    lines_coverage_seq: HashMap<(String, TimeAnnotation), Vec<LineCoverage>>,
}

impl VlcCoverageReport {
    /// The input is a list of (t, path), where t is sample time, path is the Vlc coverage file.
    pub fn new<P: AsRef<Path>>(paths: &[(TimeAnnotation, P)]) -> Self {
        let comb_mappings = paths
            .into_iter()
            .filter(|(_, p)| {
                let path_str = p.as_ref().to_str().unwrap();
                let ret = path_str.ends_with("comb.dat");
                ret
            })
            .filter_map(|(t, p)| {
                if let Ok(report) = VlcReport::from_file(p) {
                    Some((*t, report))
                } else {
                    warn!("Error parsing LCOV coverage file for {:?}", p.as_ref());
                    None
                }
            })
            .collect::<HashMap<_, _>>();
        let seq_mappings = paths
            .into_iter()
            .filter(|(t, p)| {
                let path_str = p.as_ref().to_str().unwrap();
                let ret = path_str.ends_with("seq.dat");
                ret
            })
            .filter_map(|(t, p)| {
                if let Ok(report) = VlcReport::from_file(p) {
                    Some((*t, report))
                } else {
                    warn!("Error parsing LCOV coverage file for {:?}", p.as_ref());
                    None
                }
            })
            .collect::<HashMap<_, _>>();

        let lines_coverage_comb = comb_mappings
            .iter()
            .map(|(time, report)| {
                report
                    .get_scopes()
                    .par_iter()
                    .map(move |scope_name| {
                        let lines_coverage = report
                            .get_scope_coverage(&scope_name)
                            .into_par_iter()
                            .map(|mut line_coverage| {
                                let filename = line_coverage
                                    .file_name
                                    .split("/")
                                    .collect::<Vec<&str>>()
                                    .last()
                                    .unwrap()
                                    .to_string();
                                // update line offset
                                line_coverage.line;
                                // wrap time inside
                                line_coverage.time = *time;
                                line_coverage
                            })
                            .collect::<Vec<_>>();
                        ((scope_name.clone(), time.clone()), lines_coverage)
                    })
                    .collect::<Vec<_>>()
            })
            // .collect::<Vec<_>>()
            .flatten()
            .collect::<HashMap<_, _>>();

        let lines_coverage_seq = seq_mappings
            .iter()
            .map(|(time, report)| {
                report
                    .get_scopes()
                    .par_iter()
                    .map(move |scope_name| {
                        let lines_coverage = report
                            .get_scope_coverage(&scope_name)
                            .into_par_iter()
                            .map(|mut line_coverage| {
                                let filename = line_coverage
                                    .file_name
                                    .split("/")
                                    .collect::<Vec<&str>>()
                                    .last()
                                    .unwrap()
                                    .to_string();
                                line_coverage.line;
                                // wrap time inside
                                line_coverage.time = *time;
                                line_coverage
                            })
                            .collect::<Vec<_>>();
                        ((scope_name.clone(), time.clone()), lines_coverage)
                    })
                    .collect::<Vec<_>>()
            })
            .flatten()
            .collect::<HashMap<_, _>>();

        Self {
            time_coverage_mapping_comb: if comb_mappings.is_empty() {
                seq_mappings.clone()
            } else {
                comb_mappings
            },
            time_coverage_mapping_seq: seq_mappings,
            lines_coverage_comb: if lines_coverage_comb.is_empty() {
                lines_coverage_seq.clone()
            } else {
                lines_coverage_comb
            },
            lines_coverage_seq,
        }
    }

    fn get_always_lines_coverage(
        &self,
        ctype: CircuitType,
        scope_name: &str,
        time: TimeAnnotation,
    ) -> Vec<LineCoverage> {
        let lines = match ctype {
            CircuitType::COMB => &self.lines_coverage_comb,
            CircuitType::SEQ => &self.lines_coverage_seq,
        };
        let key = (scope_name.to_string(), time);
        lines.get(&key).unwrap_or(&vec![]).clone()
    }
}

impl CoverageTracker for VlcCoverageReport {
    fn check_line_covered(
        &self,
        btype: Option<BlockType>,
        scope_name: Option<&str>,
        _module_name: Option<&str>,
        time: Option<TimeAnnotation>,
        lineno: u32,
    ) -> Option<usize> {
        match btype {
            Some(BlockType::Always(ctype)) => {
                // Always Coverage should check ctype
                if scope_name.and(time).is_some() {
                    let ret = self
                        .get_always_lines_coverage(ctype, scope_name.unwrap(), time.unwrap())
                        .iter()
                        .find(|&line_coverage| line_coverage.line == lineno)
                        .map(|line_coverage| line_coverage.count);
                    ret
                } else {
                    None
                }
            }
            // Assign or ModulePort is always covered.
            Some(_) => Some(1),
            None => None,
        }
    }

    fn get_covered_module_files(&self) -> Vec<String> {
        let res = self
            .time_coverage_mapping_seq
            .iter()
            .chain(self.time_coverage_mapping_comb.iter())
            .take(1)
            .map(|(_, report)| {
                report.points_coverage.iter().map(|(point, _)| {
                    let filename = point.filename().split('/').last().unwrap().to_string();
                    filename
                })
            })
            .flatten()
            .collect::<HashSet<_>>();
        res.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_vlc_report() {
        let vlc_file = "tests/test_files/coverage_per_instance_25_comb.dat";
        let vlc_report = VlcReport::from_file(vlc_file).unwrap();
        // println!("{:#?}", vlc_report);

        let scope_name = "TOP.tb.dut_1";
        let same_line_cov_cnt = vlc_report
            .points_coverage
            .iter()
            .filter(|(report, _)| report.lineno() == 2 && report.hierarchy() == scope_name)
            .map(|(r, _)| r)
            .count();
        assert_eq!(same_line_cov_cnt, 1);
    }

    #[test]
    fn test_vlc_coverage_report() {
        let vlc_file = "tests/test_files/coverage_per_instance_25_comb.dat";
        let vlc_coverage = VlcCoverageReport::new(&vec![(1, vlc_file)]);
        let covered_mod_files = vlc_coverage.get_covered_module_files();
        assert_eq!(covered_mod_files.len(), 2);
        assert!(covered_mod_files.contains(&"adder1.v".to_string()));
        assert!(covered_mod_files.contains(&"tb.v".to_string()));
    }

    #[test]
    fn test_vlc_coverage_tracker() {
        let vlc_file = "/home/lzz/exp_wkdir/ibex_test/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/coverage_17.dat";
        let tracker = VlcCoverageReport::new(&[(17, vlc_file)]);
        let mut t1_cov = tracker
            .get_always_lines_coverage(
                CircuitType::COMB,
                "TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core.ex_block_i.alu_i",
                17,
            )
            .into_iter()
            .filter(|cov| cov.count > 0)
            .collect::<Vec<LineCoverage>>();
        t1_cov.sort_by_key(|cov| cov.line);
        println!("{:#?}", t1_cov);
    }

    #[test]
    fn test_parse_numbers() {
        let input = "14,16-19,22";
        let numbers = VlcReport::parse_numbers(input);
        assert_eq!(numbers, vec![14, 16, 17, 18, 19, 22]);

        let input = "16-19";
        let numbers = VlcReport::parse_numbers(input);
        assert_eq!(numbers, vec![16, 17, 18, 19]);

        let input = "14";
        let numbers = VlcReport::parse_numbers(input);
        assert_eq!(numbers, vec![14]);
    }
}
