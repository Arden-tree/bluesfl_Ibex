use crate::SignalValueInterpretation;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use tabled::builder::Builder;
use tabled::settings::Style;

type WaveformData<'a> = HashMap<usize, HashMap<String, SignalValueInterpretation<'a>>>;

pub struct WaveformTable<'a> {
    rows: WaveformData<'a>,
    ignore_time: bool,
}

impl<'a> WaveformTable<'a> {
    pub fn new(ignore_time: bool) -> Self {
        Self {
            rows: HashMap::new(),
            ignore_time,
        }
    }

    pub fn add_row(&mut self, time: usize, values: HashMap<String, SignalValueInterpretation<'a>>) {
        // assert_eq!(values.len(), self.signal_names.len());
        // WARN: some signals may cannot be got from the waveform. So when displaying siganl values,
        // we will set `Unknown` to empty signals.

        self.rows.entry(time).or_default().extend(values);
    }

    pub fn make_table_string_at_time(&self, time: usize) -> String {
        let mut records = Vec::new();

        let mut header = Vec::new();
        header.push("signals".to_string());
        header.push("values".to_string());
        records.push(header);

        if let Some(data) = self.rows.get(&time) {
            if !self.ignore_time {
                let mut time_record = Vec::new();
                time_record.push("time".to_string());
                time_record.push(format!("{:.2}", time));
                records.push(time_record);
            }

            for (sig_name, sig_val) in data.iter() {
                let mut record = Vec::new();
                record.push(sig_name.clone());
                record.push(format!("{}", sig_val.to_string()));

                records.push(record);
            }
        }
        let mut builder = Builder::default();
        for record in records {
            builder.push_record(record);
        }

        let mut table = builder.build();
        table.with(Style::markdown());
        let ret = table.to_string();
        ret
    }
}

impl Display for WaveformTable<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut result = vec![];

        for time in self.rows.keys() {
            let table_string = self.make_table_string_at_time(*time);
            result.push(table_string);
        }

        let result = result.join("\n\n");
        write!(f, "{}", result)
    }
}
