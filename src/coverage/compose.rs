use crate::coverage::param::ParameterCoverageReport;
use crate::{BlockType, CoverageTracker, TimeAnnotation, VlcCoverageReport};
use std::collections::HashSet;
use std::path::Path;

pub struct CompositeCoverageTracker {
    vlc: VlcCoverageReport,
    param: ParameterCoverageReport,
}

impl CompositeCoverageTracker {
    pub fn new<P: AsRef<Path>>(paths: &[(TimeAnnotation, P)], json_path: P) -> Self {
        let vlc = VlcCoverageReport::new(paths);
        let param = ParameterCoverageReport::new(json_path);

        CompositeCoverageTracker { vlc, param }
    }
}

impl CoverageTracker for CompositeCoverageTracker {
    fn check_line_covered(
        &self,
        btype: Option<BlockType>,
        scope_name: Option<&str>,
        module_name: Option<&str>,
        time: Option<TimeAnnotation>,
        lineno: u32,
    ) -> Option<usize> {
        match btype.clone() {
            Some(BlockType::Always(_)) => {
                self.vlc
                    .check_line_covered(btype, scope_name, module_name, time, lineno)
            }
            Some(BlockType::Assign) => {
                self.param
                    .check_line_covered(btype, scope_name, module_name, time, lineno)
            }
            Some(_) => Some(1),
            None => None,
        }
    }

    fn get_covered_module_files(&self) -> Vec<String> {
        self.vlc
            .get_covered_module_files()
            .into_iter()
            .chain(
                self.param
                    .get_covered_module_files()
                    .into_iter()
                    .map(|name| {
                        if name.find(".").is_none() {
                            // FIXME: actually, the module file may be not suffixed with "sv".
                            format!("{}.sv", name)
                        } else {
                            name
                        }
                    }),
            )
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<String>>()
    }
}
