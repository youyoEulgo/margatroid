use agent_plugin::{AgentCreateReply, AgentMcl, AgentTurnState, TokenUsageState};
use margatroid_types::{Block, BlockInner, BlockPath, MclDeleteSelection, RefBlock, TokenUsage};

fn message_path() -> BlockPath {
    BlockPath {
        block_id: "conversation".to_owned(),
        inner_id: "messages".to_owned(),
    }
}

#[test]
fn agent_mcl_owns_blocks_and_enforces_one_block_namespace() {
    let mut mcl = AgentMcl::default();
    let mut block = Block::default();
    block
        .inners
        .insert("messages".to_owned(), BlockInner::Message(Vec::new()));
    mcl.create_block("conversation".to_owned(), block).unwrap();

    assert!(mcl
        .create_ref_block("conversation".to_owned(), RefBlock::default())
        .is_err());
    assert_eq!(mcl.blocks().blocks.len(), 1);
    assert!(mcl.ref_blocks().blocks.is_empty());
}

#[test]
fn agent_mcl_rejects_wrong_types_without_mutating_the_field() {
    let mut mcl = AgentMcl::default();
    let mut block = Block::default();
    block
        .inners
        .insert("messages".to_owned(), BlockInner::Message(Vec::new()));
    mcl.create_block("conversation".to_owned(), block).unwrap();

    assert!(mcl
        .insert(&message_path(), BlockInner::ResourceId(Vec::new()))
        .is_err());
    assert_eq!(mcl.select(&message_path()).unwrap().len(), 0);
}

#[test]
fn agent_mcl_delete_is_atomic_when_an_index_is_invalid() {
    let mut mcl = AgentMcl::default();
    let mut block = Block::default();
    block.inners.insert(
        "tools".to_owned(),
        BlockInner::ResourceId(vec![
            "tool:local/one:latest".parse().unwrap(),
            "tool:local/two:latest".parse().unwrap(),
        ]),
    );
    mcl.create_block("conversation".to_owned(), block).unwrap();
    let path = BlockPath {
        block_id: "conversation".to_owned(),
        inner_id: "tools".to_owned(),
    };

    assert!(mcl
        .delete(&path, MclDeleteSelection::Indices(vec![0, 4]))
        .is_err());
    assert_eq!(mcl.select(&path).unwrap().len(), 2);
}

#[test]
fn turn_state_only_finishes_the_active_turn() {
    let mut turn = AgentTurnState::default();
    turn.begin("turn-1".to_owned()).unwrap();
    assert!(turn.begin("turn-2".to_owned()).is_err());
    assert!(turn.finish("turn-2").is_err());
    turn.finish("turn-1").unwrap();
    assert_eq!(turn.turn_id, None);
}

#[test]
fn token_usage_saturates_and_updates_the_cache_hit_rate() {
    let mut state = TokenUsageState::default();
    state.add(&TokenUsage {
        input_tokens: 100,
        output_tokens: 20,
        cache_hit_tokens: 25,
    });
    assert_eq!(state.total_input_tokens, 100);
    assert_eq!(state.last_input_tokens, 100);
    assert_eq!(state.cache_hit_rate, 0.25);
}

#[tokio::test]
async fn create_reply_closes_when_all_reply_handles_are_dropped() {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let reply = AgentCreateReply::new(sender);
    drop(reply);
    assert!(receiver.await.is_err());
}
