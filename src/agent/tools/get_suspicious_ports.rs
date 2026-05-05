use crate::block::mgr::BlockManager;
use crate::{unwrap_serde_value, BlockParser};
use async_trait::async_trait;
use log::{error, info};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

#[derive(Deserialize, Serialize, Debug)]
pub struct SuspiciousPortsCommandWrapper {
    module_name: String,
}

pub struct GetSuspiciousPortsTool<'a, Parser: BlockParser> {
    block_manager: Arc<BlockManager<'a, Parser>>,
}

impl<'a, Parser: BlockParser> GetSuspiciousPortsTool<'a, Parser> {
    pub fn new(block_manager: Arc<BlockManager<'a, Parser>>) -> Self {
        Self { block_manager }
    }
}

#[async_trait]
impl<'a, Parser> Tool for GetSuspiciousPortsTool<'a, Parser>
where
    Parser: BlockParser,
    <Parser as BlockParser>::Block<'a>: Sync + Send,
{
    fn name(&self) -> String {
        String::from("get_suspicious_ports")
    }

    fn description(&self) -> String {
        r#"This function takes a module name as input and returns an array of suspicious output ports in the module header."
            "#.to_string()
    }

    fn parameters(&self) -> Value {
        let prompt = self.description();
        json!({
            "description": prompt,
            "type": "object",
            "properties": {
                "module_name": {
                    "type": "string",
                    "description": "The name of the module to retrieve the code snippet for. e.g. \"alu\""
                },
            },
            "required": ["module_name"],
        })
    }

    async fn run(&self, input: Value) -> Result<String, Box<dyn Error>> {
        let wrapper: SuspiciousPortsCommandWrapper = serde_json::from_value(input)?;
        info!("requesting suspicious ports for module {:?}", wrapper);
        Ok(self
            .block_manager
            .get_suspicious_ports(&wrapper.module_name))
    }

    async fn parse_input(&self, input: &str) -> Value {
        info!("Parsing input: {}", input);
        let wrapper_result = serde_json::from_str::<SuspiciousPortsCommandWrapper>(input);
        unwrap_serde_value!(wrapper_result)
    }
}
