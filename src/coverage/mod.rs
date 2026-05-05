pub mod compose;
pub mod param;
pub mod plain;
pub mod vlc;

use crate::{BlockType, TimeAnnotation};

pub type LineOffset = u32;

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct LineCoverage {
    pub file_name: String,
    pub scope_name: String,
    pub time: TimeAnnotation,
    // original lineno before parse | verilator AST report lineno
    pub line: u32,
    #[deprecated]
    pub origin_line: u32,
    pub count: usize,
}

impl LineCoverage {
    pub fn new(
        file_name: &str,
        scope_name: &str,
        time: TimeAnnotation,
        line: u32,
        origin_line: u32,
        count: usize,
    ) -> Self {
        Self {
            file_name: file_name.to_string(),
            scope_name: scope_name.to_string(),
            time,
            line,
            #[allow(deprecated)]
            origin_line,
            count,
        }
    }
}

/// You need to prepare two kinds of coverage report:
/// For each timestamp t, you need to cal
/// 1. seq coverage: zero before posedge clk, save after posedge
/// 2. comb coverage: zero before negedge clk, save after negedge
/// comb coverage will be used to cal coverage for comb circuit. vs.
/// Before query whether `lineno` is covered, you need to know which type is it;
/// 1. None: this block not instantiated in the final design
/// 2. Some(0): this line is not covered
/// 3. Some(>0): covered.
pub trait CoverageTracker {
    fn check_line_covered(
        &self,
        btype: Option<BlockType>,
        scope_name: Option<&str>,
        module_name: Option<&str>,
        time: Option<TimeAnnotation>,
        lineno: u32,
    ) -> Option<usize>;
    fn get_covered_module_files(&self) -> Vec<String>;
}
