use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::hash::Hash;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Iterates through the files in the given `project_path` to find all files
/// that have the extensions `.sv` or `.v`, and returns a list of their paths.
/// If `project_path` is a file, it will be included if it has the appropriate extension.
/// If `project_path` is a directory, all `.sv` and `.v` files in that directory will be included.
pub fn get_module_files<P: AsRef<Path>>(project_path: P) -> Vec<PathBuf> {
    let path = project_path.as_ref();
    let mut files = Vec::new();

    // Check if the path is a file or directory
    if path.is_file() {
        // If it's a file, check if it has the appropriate extension
        if let Some(extension) = path.extension() {
            if extension == "sv" || extension == "v" {
                files.push(path.to_path_buf());
            }
        }
    } else if path.is_dir() {
        // If it's a directory, iterate through the entries
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.filter_map(Result::ok) {
                let entry_path = entry.path();
                // Only process files (skip subdirectories)
                if entry_path.is_file() {
                    // Check if the file has the appropriate extension
                    if let Some(extension) = entry_path.extension() {
                        if extension == "sv" || extension == "v" {
                            files.push(entry_path);
                        }
                    }
                }
            }
        }
    }

    files
}

/// it looks like that we need to define a `Scope` type.
pub fn get_last_scope(scope_name: &str) -> Option<&str> {
    if let Some(pos) = scope_name.rfind('.') {
        Some(&scope_name[..pos])
    } else {
        None
    }
}

pub fn get_next_scope(scope_name: &str, dut_name: &str) -> Option<String> {
    Some(format!("{}.{}", scope_name, dut_name))
}

/// Extract the meaningful suffix from a Chisel-generated signal name
/// for cross-module boundary matching.
///
/// Chisel renames signals at module boundaries. For example:
/// - WBU input:  `io_in_bits_decode_cf_redirect_target`
/// - EXU output: `_exu_io_out_bits_decode_cf_redirect_target`
/// - ALU output: `io_redirect_target`
///
/// All share the suffix `redirect_target`. This function extracts it
/// by splitting on `_` and taking the last two segments.
///
/// Returns at least the last segment. If there's only one segment,
/// returns the full signal name.
pub fn extract_signal_suffix(signal_name: &str) -> &str {
    let parts: Vec<&str> = signal_name.split('_').collect();
    if parts.len() >= 2 {
        // Take last 2 segments: e.g., "redirect_target", "out_bits", "rf_data"
        let start = parts.len() - 2;
        // But skip generic Chisel prefixes like "bits", "data" if they are the only suffix
        let suffix = &signal_name[parts[..start].iter().map(|s| s.len() + 1).sum::<usize>()..];
        suffix
    } else {
        signal_name
    }
}

#[cfg(test)]
mod suffix_tests {
    use super::*;

    #[test]
    fn test_extract_suffix() {
        assert_eq!(
            extract_signal_suffix("io_in_bits_decode_cf_redirect_target"),
            "redirect_target"
        );
        assert_eq!(
            extract_signal_suffix("_exu_io_out_bits_decode_cf_redirect_target"),
            "redirect_target"
        );
        assert_eq!(extract_signal_suffix("io_redirect_target"), "redirect_target");
        assert_eq!(extract_signal_suffix("io_out_bits"), "out_bits");
        assert_eq!(extract_signal_suffix("io_in_bits_func"), "bits_func");
    }
}

/// Returns the top-k most frequent items from a collection.
///
/// # Arguments
///
/// * `items` - A vector containing all items to be counted.
///
/// * `k` - The number of top items to return. If k is greater than the number of
///   unique items, all items will be returned in order of frequency.
///
/// # Returns
///
/// A vector of tuples (item, count) sorted by count in descending order,
/// then by item in ascending order when counts are equal, limited to k items.
///
/// # Example
///
/// ```
/// use sv_analysis::top_k_items;
/// let items = vec!["apple", "banana", "cherry", "banana", "cherry", "apple", "cherry", "durian"];
///
/// let top_2 = top_k_items(items, 2);
/// assert_eq!(top_2, vec![("cherry", 3), ("apple", 2)]);
/// ```
pub fn top_k_items<T>(items: Vec<T>, k: usize) -> Vec<(T, usize)>
where
    T: Eq + Hash + Clone + Ord,
{
    let mut item_counts: HashMap<T, usize> = HashMap::new();

    for item in items {
        *item_counts.entry(item).or_insert(0) += 1;
    }

    let mut count_pairs: Vec<(T, usize)> = item_counts.into_iter().collect();

    count_pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    count_pairs.truncate(k);

    count_pairs
}

/// Saves any serializable data to a JSON file
pub fn save_data_to_json<T: Serialize, P: AsRef<Path>>(data: &T, path: P) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    let mut file = File::create(path)?;
    file.write_all(json.as_bytes())?;

    Ok(())
}

pub fn load_json_to_data<T: DeserializeOwned, P: AsRef<Path>>(path: P) -> anyhow::Result<T> {
    let file = File::open(path)?;
    let data = serde_json::from_reader(file)?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NodeID, TimeAnnotation};

    #[test]
    fn test_top_k_items() {
        let items = vec![
            "apple", "banana", "cherry", "banana", "cherry", "apple", "cherry", "durian",
        ];

        let top_3 = top_k_items(items, 3);
        assert_eq!(top_3, vec![("cherry", 3), ("apple", 2), ("banana", 2)]);
    }

    #[test]
    fn test_empty_list() {
        let items: Vec<String> = vec![];
        let top = top_k_items(items, 5);
        assert!(top.is_empty());
    }

    #[test]
    fn test_k_larger_than_unique_items() {
        let items = vec![1, 2, 2, 3, 3, 3];
        let top = top_k_items(items, 10);
        assert_eq!(top, vec![(3, 3), (2, 2), (1, 1)]);
    }

    #[test]
    fn test_get_module_files() {
        let project_path = "/home/lzz/exp_wkdir/ibex_test/ibex/rtl";
        let module_files = get_module_files(project_path);
        println!("{:#?}", module_files);
    }

    #[test]
    fn test_get_last_scope() {
        let name = "TOP.ex.alu";
        assert_eq!(Some("TOP.ex"), get_last_scope(name));

        let name = "TOP";
        assert_eq!(None, get_last_scope(name));
    }

    #[test]
    fn test_load_json_to_data() {
        let path = "suspicious_modules.json";
        let data: Vec<((NodeID, Option<TimeAnnotation>), String)> =
            load_json_to_data(path).unwrap();

        let mut mod_times = HashMap::new();
        for ((node, time), name) in data {
            mod_times
                .entry(time)
                .or_insert(HashMap::new())
                .entry(name)
                .or_insert(vec![])
                .push(node.get_text().to_string());
        }

        println!("{:#?}", mod_times);
    }
}
