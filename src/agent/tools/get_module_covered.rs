use crate::{BlockManager, BlockParser};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

#[derive(Deserialize, Serialize, Debug)]
pub struct ModuleCoveredCommandWrapper {
    module_name: String,
    signals: Vec<String>,
}

pub struct GetModuleCoveredTool<'a, Parser: BlockParser> {
    block_manager: Arc<BlockManager<'a, Parser>>,
}

impl<'a, Parser: BlockParser> GetModuleCoveredTool<'a, Parser> {
    pub fn new(block_manager: Arc<BlockManager<'a, Parser>>) -> Self {
        Self { block_manager }
    }
}

#[async_trait]
impl<'a, Parser> Tool for GetModuleCoveredTool<'a, Parser>
where
    Parser: BlockParser,
    <Parser as BlockParser>::Block<'a>: Sync + Send,
{
    fn name(&self) -> String {
        String::from("get_module_covered")
    }

    fn description(&self) -> String {
        "This function retrieves a set of module names covered by failing tests.".to_string()
    }

    fn parameters(&self) -> Value {
        let prompt = self.description();
        json!({
            "description": prompt,
            "type": "object",
            "properties": {},
            "required": [],
        })
    }

    async fn run(&self, _input: Value) -> Result<String, Box<dyn Error>> {
        Ok(self.block_manager.get_modules_covered())
    }
}
