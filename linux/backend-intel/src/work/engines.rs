//! Which of the card's engines a guest's engine node runs on.

use crate::xe::abi::EngineInstance;
use crate::xe::plan;
use crate::xe_query::Engine;

/// The xe engine class and instance behind each node the guest's drivers use, by the node's number
/// in Intel's own numbering. The compute engine has two numbers.
const NODES: [(u32, u16, u16); 7] = [
    (0, Engine::RENDER, 0),
    (1, Engine::VIDEO_DECODE, 0),
    (2, Engine::COPY, 0),
    (3, Engine::VIDEO_ENHANCE, 0),
    (4, Engine::VIDEO_DECODE, 1),
    (5, Engine::COMPUTE, 0),
    (13, Engine::COMPUTE, 0),
];

/// The engine a node's work runs on, if the node is one that runs and the card has the engine.
pub fn for_node(engines: &[Engine], node: u32) -> Option<EngineInstance> {
    let &(_, class, instance) = NODES.iter().find(|(number, ..)| *number == node)?;
    plan::engine(engines, class, instance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card;

    #[test]
    fn each_node_runs_on_its_engine_class() {
        let engines = card::tests::battlemage().engines;
        let class = |node| for_node(&engines, node).map(|engine| engine.class);
        assert_eq!(class(0), Some(Engine::RENDER));
        assert_eq!(class(1), Some(Engine::VIDEO_DECODE));
        assert_eq!(class(2), Some(Engine::COPY));
        assert_eq!((class(5), class(13)), (Some(Engine::COMPUTE), Some(Engine::COMPUTE)));
    }

    #[test]
    fn a_node_with_no_engine_behind_it_runs_nowhere() {
        let engines = card::tests::battlemage().engines;
        assert_eq!(for_node(&engines, 3), None, "this card lists no video enhance engine");
        assert_eq!(for_node(&engines, 6), None, "not a node that runs");
        assert_eq!(for_node(&[], 0), None);
    }
}
