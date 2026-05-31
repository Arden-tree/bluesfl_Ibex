pub mod mgr;
pub mod repr;

use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::path::Path;
use wellen::simple::Waveform;
use wellen::{self, simple::read, Hierarchy, SignalRef, SignalValue, TimeTableIdx, WellenError};

pub struct WaveInspector {
    waveform: Waveform,
    /// Maps sv-parser scope paths to FST scope paths.
    /// Key: reversed FST scope path (e.g., "alu.exu.backend.nutcore...")
    /// Value: FST scope path as Vec<String>
    scope_index: Vec<(Vec<String>, Vec<String>)>,
}

/// Builds a mapping from sv-parser scope paths to FST scope paths.
/// Collects all FST scopes indexed by their reversed component sequence,
/// enabling longest-suffix matching against sv-parser scope paths.
fn build_scope_index(hierarchy: &Hierarchy) -> Vec<(Vec<String>, Vec<String>)> {
    let mut index = Vec::new();
    for scope_ref in hierarchy.scopes() {
        collect_scopes_recursive(hierarchy, scope_ref, &mut vec![], &mut index);
    }
    index
}

fn collect_scopes_recursive(
    hierarchy: &Hierarchy,
    scope_ref: wellen::ScopeRef,
    path: &mut Vec<String>,
    index: &mut Vec<(Vec<String>, Vec<String>)>,
) {
    let scope = &hierarchy[scope_ref];
    let name = scope.name(hierarchy).to_string();
    path.push(name);

    // Store (reversed_path, forward_path)
    let reversed: Vec<String> = path.iter().rev().cloned().collect();
    let forward: Vec<String> = path.clone();
    index.push((reversed, forward));

    for sub_ref in scope.scopes(hierarchy) {
        collect_scopes_recursive(hierarchy, sub_ref, path, index);
    }
    path.pop();
}

/// Given an sv-parser scope (as Vec<&str>), find all matching FST scope paths
/// sorted by longest-suffix match length (descending).
/// Returns candidates for caller to verify signal existence.
fn map_scope_candidates(
    sv_scope: &[&str],
    scope_index: &[(Vec<String>, Vec<String>)],
) -> Vec<(usize, Vec<String>)> {
    let sv_rev: Vec<&str> = sv_scope.iter().rev().cloned().collect();

    let mut candidates: Vec<(usize, Vec<String>)> = Vec::new();

    for (fst_rev, fst_fwd) in scope_index {
        let common_len = sv_rev
            .iter()
            .zip(fst_rev.iter())
            .take_while(|(a, b)| a == b)
            .count();

        if common_len > 0 {
            candidates.push((common_len, fst_fwd.clone()));
        }
    }

    // Sort by match length descending (best first)
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates
}

/// Represents a signal value that may be in raw form or translated through an enum mapping
#[derive(Debug, Clone)]
pub enum SignalValueInterpretation<'a> {
    /// The original raw signal value
    Raw(SignalValue<'a>),
    /// The human-readable enum-mapped representation of the signal value
    EnumMapped(String),
    /// Unknown Value, this signal is not included in the waveform file
    Unknown,
}

impl<'a> SignalValueInterpretation<'a> {
    pub fn get_width_value(&self) -> (Option<u32>, String) {
        match self {
            SignalValueInterpretation::Raw(signal_value) => match signal_value {
                SignalValue::Binary(data, bit_width)
                | SignalValue::FourValue(data, bit_width)
                | SignalValue::NineValue(data, bit_width) => {
                    let hex_str = data
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>();

                    let value = format!("0x{}", hex_str);
                    let width = Some(*bit_width);
                    (width, value)
                }
                SignalValue::String(s) => (None, s.to_string()),
                SignalValue::Real(r) => (None, r.to_string()),
            },
            SignalValueInterpretation::EnumMapped(x) => (None, x.to_string()),
            SignalValueInterpretation::Unknown => (None, "Unknown".to_string()),
        }
    }
}

impl<'a> Display for SignalValueInterpretation<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SignalValueInterpretation::Raw(signal_value) => match signal_value {
                SignalValue::Binary(data, bit_width)
                | SignalValue::FourValue(data, bit_width)
                | SignalValue::NineValue(data, bit_width) => {
                    write!(f, "{}-bits: 0x", bit_width)?;
                    for byte in *data {
                        write!(f, "{:02x}", byte)?;
                    }
                    Ok(())
                }
                SignalValue::String(s) => write!(f, "String: {}", s),
                SignalValue::Real(r) => write!(f, "Real: {}", r),
            },
            SignalValueInterpretation::EnumMapped(x) => write!(f, "{}", x),
            SignalValueInterpretation::Unknown => write!(f, "Unknown"),
        }
    }
}

impl WaveInspector {
    pub fn new<T: AsRef<Path>>(path: T) -> Result<Self, WellenError> {
        let waveform = read(path)?;
        let hierarchy = waveform.hierarchy();
        let scope_index = build_scope_index(hierarchy);
        Ok(Self { waveform, scope_index })
    }

    fn get_signal_item<S: AsRef<str>, T: AsRef<str>>(
        hierarchy: &Hierarchy,
        scope: &[S],
        var: T,
        scope_index: &[(Vec<String>, Vec<String>)],
        time: usize,
    ) -> Option<Vec<(usize, String, SignalRef, Option<HashMap<String, String>>)>> {
        let var_str = var.as_ref();
        let scope_vec: Vec<&str> = scope.iter().map(|s| s.as_ref()).collect();

        // 1. Try exact scope path
        if let Some(var_id) = hierarchy.lookup_var(&scope_vec, &var_str) {
            return Some(Self::make_var_info(hierarchy, var_id, time, var_str));
        }

        // 2. Map scope via suffix matching, verify signal existence for each candidate
        let candidates = map_scope_candidates(&scope_vec, scope_index);
        for (match_len, mapped_scope) in &candidates {
            if let Some(var_id) = hierarchy.lookup_var(&mapped_scope[..], &var_str.to_string()) {
                log::info!(
                    "Scope mapped (match={}/{}): {:?} -> {:?}, signal '{}' found",
                    match_len, scope_vec.len(), scope_vec, mapped_scope, var_str
                );
                return Some(Self::make_var_info(hierarchy, var_id, time, var_str));
            }
        }

        // 3. Try array scope (exact + best mapped candidate)
        let mut scope_candidates: Vec<Vec<String>> = vec![
            scope_vec.iter().map(|s| s.to_string()).collect()
        ];
        if let Some((_, best)) = candidates.first() {
            scope_candidates.push(best.clone());
        }
        for scope_path in scope_candidates {
            let mut array_scope: Vec<String> = scope_path.clone();
            array_scope.push(var_str.to_string());
            if let Some(scope_ref) = hierarchy.lookup_scope(&array_scope) {
                let vars: Vec<_> = hierarchy[scope_ref]
                    .vars(hierarchy)
                    .map(|var_id| {
                        let array_index_name = hierarchy[var_id].name(hierarchy);
                        let var_ref = &hierarchy[var_id];
                        let signal_ref = var_ref.signal_ref();
                        let var_name = var_str.to_string() + array_index_name;
                        (time, var_name, signal_ref, None)
                    })
                    .collect();
                if !vars.is_empty() {
                    return Some(vars);
                }
            }
        }

        None
    }

    fn make_var_info(
        hierarchy: &Hierarchy,
        var_id: wellen::VarRef,
        time: usize,
        var_str: &str,
    ) -> Vec<(usize, String, SignalRef, Option<HashMap<String, String>>)> {
        let var_ref = &hierarchy[var_id];
        let signal_ref = var_ref.signal_ref();
        let enum_mapping = var_ref.enum_type(hierarchy).map(|(_, enum_pairs)| {
            enum_pairs
                .iter()
                .map(|(x, y)| (x.to_string(), y.to_string()))
                .collect::<HashMap<_, _>>()
        });
        vec![(time, var_str.to_string(), signal_ref, enum_mapping)]
    }

    fn get_signal_value(
        waveform: &Waveform,
        signal_ref: SignalRef,
        time: usize,
        mapping: Option<HashMap<String, String>>,
    ) -> SignalValueInterpretation {
        let value = waveform
            .get_signal(signal_ref)
            .and_then(|signal| {
                signal.get_offset(time as TimeTableIdx).map(|offset| {
                    let value = signal.get_value_at(&offset, 0);

                    mapping
                        .as_ref()
                        .and_then(|map| map.get(&value.to_string()).cloned())
                        .map(SignalValueInterpretation::EnumMapped)
                        .unwrap_or_else(|| SignalValueInterpretation::Raw(value))
                })
            })
            .unwrap_or(SignalValueInterpretation::Unknown);
        value
    }

    pub fn get_signal_values_at_time<S: AsRef<str>, T: AsRef<str>>(
        &mut self,
        scope: &[S],
        vars: &[T],
        time: usize,
    ) -> Result<HashMap<String, SignalValueInterpretation>, WellenError> {
        let hierarchy = self.waveform.hierarchy();

        // Create variable data with signal references and enum mappings in one functional chain
        let var_data: Vec<_> = vars
            .iter()
            .filter_map(|var| {
                Self::get_signal_item(hierarchy, scope, var, &self.scope_index, time)
            })
            .flatten()
            .collect();

        let signal_refs: Vec<_> = var_data
            .iter()
            .map(|(_, _, signal_ref, _)| *signal_ref)
            .collect();

        self.waveform.load_signals(&signal_refs);

        // Transform the variable data into the final result
        let results = var_data
            .into_iter()
            .map(|(time, var_name, signal_ref, mapping)| {
                let value = Self::get_signal_value(&self.waveform, signal_ref, time, mapping);
                (var_name, value)
            })
            .collect();

        Ok(results)
    }

    pub fn get_signal_values_with_batch<S: AsRef<str>, T: AsRef<str>>(
        &mut self,
        scope: &[S],
        vars: &[(T, usize)],
    ) -> Result<HashMap<(usize, String), SignalValueInterpretation>, WellenError> {
        let hierarchy = self.waveform.hierarchy();

        // Create variable data with signal references and enum mappings in one functional chain
        let var_data: Vec<_> = vars
            .iter()
            .filter_map(|(var, time)| {
                let var_info = Self::get_signal_item(hierarchy, scope, var, &self.scope_index, *time);
                // var_info may cannot be fetched, so return `None` directly
                var_info
            })
            .flatten()
            .collect();

        let signal_refs: Vec<_> = var_data
            .iter()
            .map(|(_, _, signal_ref, _)| *signal_ref)
            .collect();

        self.waveform.load_signals(&signal_refs);

        // Transform the variable data into the final result
        let results = var_data
            .into_iter()
            .map(|(time, var_name, signal_ref, mapping)| {
                let value = Self::get_signal_value(&self.waveform, signal_ref, time, mapping);
                ((time, var_name), value)
            })
            .collect();

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WaveformTable;
    #[test]
    fn test_get_signal_values_at_time() {
        // bug1: cannot get value of imd_val_q_i
        // bug2: no align

        let wave_path = "/home/lzz/exp_wkdir/ibex_test/ibex/sim.fst";
        let start_scope = vec![
            "TOP",
            "ibex_simple_system",
            "u_top",
            "u_ibex_top",
            "u_ibex_core",
            "ex_block_i",
            "alu_i",
        ];
        let signals = vec![
            "instr_first_cycle_i",
            "multdiv_sel_i",
            "operand_b_i",
            "operand_a_i",
            "multdiv_operand_a_i",
            "imd_val_q_i",
            "multdiv_operand_b_i",
            "operator_i",
        ];
        let start_time = 15;
        let mut inspector = WaveInspector::new(wave_path).unwrap();
        let wave_values = inspector
            .get_signal_values_at_time(&start_scope, &signals, start_time)
            .unwrap();
        println!("{:?}", wave_values.keys().collect::<Vec<_>>());

        let mut table = WaveformTable::new(false);
        table.add_row(start_time, wave_values);
        println!("{}", table.to_string());
    }
}
