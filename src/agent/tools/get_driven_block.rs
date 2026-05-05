use crate::block::mgr::BlockManager;
use crate::{unwrap_serde_value, BlockParser};
use async_trait::async_trait;
use log::{error, info};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

#[derive(Deserialize, Serialize, Debug)]
pub struct DrivenBlockCommandWrapper {
    module_name: String,
    signals: Vec<String>,
}

pub struct GetDrivenBlockTool<'a, Parser: BlockParser> {
    language: String,
    block_manager: Arc<BlockManager<'a, Parser>>,
}

impl<'a, Parser: BlockParser> GetDrivenBlockTool<'a, Parser> {
    pub fn new(language: &str, block_manager: Arc<BlockManager<'a, Parser>>) -> Self {
        Self {
            language: language.into(),
            block_manager,
        }
    }
}

#[async_trait]
impl<'a, Parser> Tool for GetDrivenBlockTool<'a, Parser>
where
    Parser: BlockParser,
    <Parser as BlockParser>::Block<'a>: Sync + Send,
{
    fn name(&self) -> String {
        String::from("get_driven_block")
    }

    fn description(&self) -> String {
        format!(
            r#"This function takes a module name and an array of signal names within that module as input, "
            "and returns the corresponding {} code snippet that drives these signals. "
            "Note that the snippet focuses solely on the behavioral logic, and syntax errors are to be disregarded for the purpose of this snippet.
            "#,
            self.language
        )
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
                "signals": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "description": "Signal name."
                    },
                    "description": "Signal names to retrieve the code snippet that drives these signals."
                }
            },
            "required": ["module_name", "signals"],
        })
    }

    async fn run(&self, input: Value) -> Result<String, Box<dyn Error>> {
        let wrapper: DrivenBlockCommandWrapper = serde_json::from_value(input)?;
        info!("requesting driven blocks for {:?}", wrapper);
        let ctx = self
            .block_manager
            .get_block_snippet(&wrapper.module_name, &wrapper.signals)
            .iter()
            .map(|(signal, bid, ctx)| {
                json!({
                    "signal": signal,
                    "bid": bid,
                    "code context": ctx
                })
                .to_string()
            })
            .collect::<Vec<_>>();
        Ok(ctx.join("\n"))
    }

    async fn parse_input(&self, input: &str) -> Value {
        info!("Parsing input: {}", input);
        let wrapper_result = serde_json::from_str::<DrivenBlockCommandWrapper>(input);
        unwrap_serde_value!(wrapper_result)
    }
}
