use crate::{Block, TimeAnnotation};
use serde_json::{json, Value};

pub fn save_trace_to_json<'a, B>(blocks: &[(B, Option<TimeAnnotation>)]) -> Value
where
    B: Block<'a>,
{
    let res = blocks
        .iter()
        .map(|(block, time)| {
            let typ = block.get_block_type().to_string();
            let suspicious_trace = block.get_suspicious_trace();
            let suspicious_outputs =
                suspicious_trace.map_or(vec![], |trace| vec![(&trace.0).clone()]);
            let suspicious_input_vars = suspicious_trace.map_or(vec![], |trace| (&trace.1).clone());
            json!({
                "bid": block.get_bid(),
                "scope": block.get_scope(),
                "type": typ,
                "time": time,
                "suspicious_outputs": suspicious_outputs,
                "suspicious_input_vars_with_time": suspicious_input_vars,
            })
        })
        .collect::<Vec<Value>>();

    serde_json::to_value(res).unwrap()
}
