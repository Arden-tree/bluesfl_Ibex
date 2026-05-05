use std::collections::HashMap;

pub mod block;
pub mod macros;
mod utils;
pub type PromptArgs = HashMap<String, String>;
pub use utils::token_price;
