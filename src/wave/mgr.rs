use crate::{TimeAnnotation, WaveInspector, WaveformTable};
use serde_json::json;
use std::collections::HashMap;

#[derive(Clone)]
pub struct WaveformManager {
    pub waveform_path: String,
}

impl WaveformManager {
    pub fn new(waveform_path: &str) -> Self {
        Self {
            waveform_path: waveform_path.to_string(),
        }
    }

    fn create_wave_inspector(&self) -> anyhow::Result<WaveInspector> {
        std::panic::catch_unwind(|| WaveInspector::new(&self.waveform_path))
            .map_err(|e| anyhow::anyhow!("Waveform loading panicked: {:?}", e))?
            .map_err(Into::into)
    }

    fn extract_bit_width_and_value(value: &crate::SignalValueInterpretation) -> (String, String) {
        let (bit_width, value_str) = value.get_width_value();
        let bit_width_str = bit_width
            .map(|width| width.to_string())
            .unwrap_or("Unknown bit-width".to_string());
        (bit_width_str, value_str)
    }

    fn create_signal_json(
        var_name: &str,
        time: usize,
        bit_width: &str,
        value: &str,
    ) -> serde_json::Value {
        json!({
            "signal_name": var_name,
            "time": time,
            "bit-width": bit_width,
            "value": value
        })
    }

    pub fn get_signal_value_at_time<T: AsRef<str>>(
        &mut self,
        scope: &[T],
        vars: &[T],
        time: TimeAnnotation,
    ) -> anyhow::Result<HashMap<String, (String, String, TimeAnnotation)>> {
        scope
            .iter()
            .for_each(|s| assert!(!s.as_ref().contains(".")));
        let mut wave_inspector = self.create_wave_inspector()?;
        let wave_values = wave_inspector.get_signal_values_at_time(scope, vars, time as usize)?;
        let mut res = HashMap::new();
        wave_values.iter().for_each(|(var_name, value)| {
            let (bit_width, value) = value.get_width_value();

            let bit_width = bit_width
                .map(|width| width.to_string())
                .unwrap_or("Unknown bit-width".to_string());
            res.insert(var_name.to_string(), (value.to_string(), bit_width, time));
        });
        Ok(res)
    }

    pub fn display_signal_values_at_time<T: AsRef<str>>(
        &mut self,
        scope: &[T],
        vars: &[T],
        time: TimeAnnotation,
    ) -> anyhow::Result<String> {
        scope
            .iter()
            .for_each(|s| assert!(!s.as_ref().contains(".")));
        let mut wave_inspector = self.create_wave_inspector()?;
        let wave_values = wave_inspector.get_signal_values_at_time(scope, vars, time as usize)?;
        let mut table = WaveformTable::new(false);
        table.add_row(time as usize, wave_values);
        Ok(table.to_string())
    }

    pub fn display_signal_values_at_time_json<T: AsRef<str>>(
        &mut self,
        scope: &[T],
        vars: &[T],
        time: TimeAnnotation,
    ) -> anyhow::Result<String> {
        let signal_values = self.get_signal_value_at_time(scope, vars, time)?;
        let json_values: Vec<_> = signal_values
            .into_iter()
            .map(|(var_name, (value, bit_width, time))| {
                Self::create_signal_json(&var_name, time as usize, &bit_width, &value)
            })
            .collect();

        if json_values.is_empty() {
            Err(anyhow::anyhow!("No signal values found"))
        } else {
            Ok(serde_json::to_string_pretty(&json_values)?)
        }
    }

    pub fn display_signal_values_with_batch<T: AsRef<str>>(
        &mut self,
        scope: &[T],
        vars: &[(T, &TimeAnnotation)],
        ignore_time: bool,
    ) -> anyhow::Result<String> {
        let mut wave_inspector = self.create_wave_inspector()?;
        let vars_dup = vars
            .iter()
            .map(|(v, t)| (v, (**t) as usize))
            .collect::<Vec<_>>();
        let wave_values =
            wave_inspector.get_signal_values_with_batch(scope, vars_dup.as_slice())?;
        let mut table = WaveformTable::new(ignore_time);
        wave_values.iter().for_each(|((time, var_name), value)| {
            let signal_values = HashMap::from([(var_name.clone(), value.clone())]);
            table.add_row(*time, signal_values);
        });

        Ok(table.to_string())
    }

    pub fn display_signal_values_with_batch_json<T: AsRef<str>>(
        &mut self,
        scope: &[T],
        vars: &[(T, &TimeAnnotation)],
        _ignore_time: bool,
    ) -> anyhow::Result<String> {
        let mut wave_inspector = self.create_wave_inspector()?;
        let vars_dup = vars
            .iter()
            .map(|(v, t)| (v, (**t) as usize))
            .collect::<Vec<_>>();
        let wave_values =
            wave_inspector.get_signal_values_with_batch(scope, vars_dup.as_slice())?;

        let json_values: Vec<_> = wave_values
            .into_iter()
            .map(|((time, var_name), value)| {
                let (bit_width, value_str) = Self::extract_bit_width_and_value(&value);
                Self::create_signal_json(&var_name, time, &bit_width, &value_str)
            })
            .collect();

        let ret = serde_json::to_string_pretty(&json_values)?;
        Ok(ret)
    }
}
