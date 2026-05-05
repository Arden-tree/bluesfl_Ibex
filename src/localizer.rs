use crate::{Block, NodeID, TimeAnnotation};
use derive_builder::Builder;
use rig::completion::Usage;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

pub type BugIDType = String;

pub trait Localizer<'a, T>
where
    T: Block<'a> + Sync + Send + Debug + 'static,
{
    fn get_bug_id(&self) -> BugIDType;
    fn get_localized_modules(&self) -> Vec<(String, Option<(NodeID, Option<TimeAnnotation>)>)>;

    fn get_localized_blocks(&self) -> Vec<(T, Option<(NodeID, Option<TimeAnnotation>)>)>;

    fn get_localization_results(&mut self) -> LocalizationResult;
}

#[derive(Debug, Serialize, Deserialize, Builder, Clone)]
#[builder(setter(into))]
pub struct LocalizationChoice {
    #[builder(default)]
    pub module_name: Option<String>,
    #[builder(default)]
    pub line_number: Option<usize>,
    #[builder(default)]
    pub block_id: Option<u64>,
    #[builder(default)]
    pub score: Option<f64>,
}

// TODO: refactor LocalizationResult to contain metadata and elements
#[derive(Debug, Serialize, Deserialize, Builder)]
#[builder(setter(into))]
pub struct LocalizationResult {
    pub bug_id: BugIDType,
    #[builder(default)]
    pub token_usage: Option<Usage>,
    #[builder(default)]
    pub token_price: Option<f64>,
    pub choices: Vec<LocalizationChoice>,
}
