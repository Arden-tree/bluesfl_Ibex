use crate::block::{Block, CircuitType};
use crate::dataflow::NodeID;
use crate::tracer::TimeAnnotation;
use crate::{BlockManager, BlockParser, BlockType, Tracer};
use anyhow::anyhow;
use async_trait::async_trait;

pub struct StaticSlicing<'a, 'b, Parser>
where
    Parser: BlockParser,
{
    block_manager: &'b BlockManager<'a, Parser>,
    time_step: Option<TimeAnnotation>,
}

impl<'a, 'b, Parser> StaticSlicing<'a, 'b, Parser>
where
    Parser: BlockParser,
{
    pub fn new(
        block_manager: &'b BlockManager<'a, Parser>,
        time_step: Option<TimeAnnotation>,
    ) -> StaticSlicing<'a, 'b, Parser> {
        StaticSlicing {
            block_manager,
            time_step,
        }
    }
}

#[async_trait(?Send)]
impl<'a, 'b, T, Parser> Tracer<'a, T> for StaticSlicing<'a, 'b, Parser>
where
    T: Block<'a> + Sync + Send,
    Parser: BlockParser<Block<'a> = T>,
{
    fn get_scope_blocks(&self, scope_name: &str) -> anyhow::Result<&[T]> {
        let res = self
            .block_manager
            .get_scope_blocks(scope_name)
            .ok_or_else(|| anyhow!("Scope {} not found", scope_name))?;
        Ok((*res).0.as_slice())
    }

    fn get_scopes(&self) -> Vec<&str> {
        self.block_manager.get_scopes()
    }
    fn get_block_covered_ast_lines(&self, _block: &T, _time: TimeAnnotation) -> Vec<(u32, usize)> {
        todo!("Not implemented")
    }
    fn get_driven_signals_in_block(
        &mut self,
        block: &T,
        btype: &BlockType,
        sig: NodeID,
        time: Option<TimeAnnotation>,
    ) -> (Option<TimeAnnotation>, Option<Vec<NodeID>>) {
        let next_time = if let Some(time) = self.time_step.and(time) {
            if matches!(btype, BlockType::Always(CircuitType::SEQ)) {
                Some(time - self.time_step.unwrap())
            } else {
                Some(time)
            }
        } else {
            None
        };

        let local_sigs = block
            .get_output_nodes()
            .into_iter()
            .filter(|&node| node.get_text() == sig.get_text())
            .collect::<Vec<_>>();

        let vars = local_sigs
            .into_iter()
            .map(|node| {
                block
                    .get_node_dataflow(node.clone())
                    .into_iter()
                    .filter(|node| matches!(node, NodeID::Identifier(_, _)))
                    .collect::<Vec<_>>()
            })
            .flatten()
            .collect::<Vec<_>>();

        (next_time, Some(vars))
    }
}
