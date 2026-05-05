use crate::{unwrap_serde_value, BlockManager, BlockParser};
use async_trait::async_trait;
use log::{error, info};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

#[derive(Deserialize, Serialize, Debug)]
pub struct SignalValuesCommandWrapper {
    bid: u64,
    time: u64,
}

pub struct GetSignalValuesTool<'a, Parser: BlockParser> {
    block_manager: Arc<BlockManager<'a, Parser>>,
}

impl<'a, Parser: BlockParser> GetSignalValuesTool<'a, Parser> {
    pub fn new(block_manager: Arc<BlockManager<'a, Parser>>) -> Self {
        Self { block_manager }
    }
}

#[async_trait]
impl<'a, Parser> Tool for GetSignalValuesTool<'a, Parser>
where
    Parser: BlockParser,
    <Parser as BlockParser>::Block<'a>: Sync + Send,
{
    fn name(&self) -> String {
        String::from("get_signal_values_at_time")
    }

    fn description(&self) -> String {
        "This function retrieves the values of signals within the block labeled with the specified bid at a specific time t during the simulation.".to_string()
    }

    fn parameters(&self) -> Value {
        let prompt = self.description();
        json!({
          "description": prompt,
          "type": "object",
          "properties": {
            "bid": {
              "type": "integer",
              "description": "ID of the block from which the signal values are retrieved."
            },
            "time": {
              "type": "integer",
              "description": "The time at which the signal values are retrieved in the waveform."
            }
          },
          "required": ["bid", "time"]
        })
    }

    async fn run(&self, input: Value) -> Result<String, Box<dyn Error>> {
        let wrapper: SignalValuesCommandWrapper = serde_json::from_value(input)?;
        info!("requesting code snippet for {:?}", wrapper);
        Ok(self
            .block_manager
            .get_signal_values(wrapper.bid, wrapper.time))
    }

    async fn parse_input(&self, input: &str) -> Value {
        info!("Parsing input: {}", input);
        let wrapper_result = serde_json::from_str::<SignalValuesCommandWrapper>(input);
        unwrap_serde_value!(wrapper_result)
    }
}
