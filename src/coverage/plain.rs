use crate::{BlockType, CoverageTracker, TimeAnnotation};
use std::path::{Path, PathBuf};

pub struct PlainCoverageReport {
    module_files: Vec<PathBuf>,
}

impl PlainCoverageReport {
    pub fn new<P: AsRef<Path>>(module_files: &[P]) -> Self {
        let module_files = module_files
            .into_iter()
            .map(|p| p.as_ref().to_path_buf())
            .collect();
        Self { module_files }
    }
}

impl CoverageTracker for PlainCoverageReport {
    fn check_line_covered(
        &self,
        _btype: Option<BlockType>,
        _scope_name: Option<&str>,
        _module_name: Option<&str>,
        _time: Option<TimeAnnotation>,
        _lineno: u32,
    ) -> Option<usize> {
        Some(1)
    }

    fn get_covered_module_files(&self) -> Vec<String> {
        self.module_files
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap().to_string())
            .collect()
    }
}
