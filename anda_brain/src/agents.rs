mod formation;
mod maintenance;
mod recall;

use anda_core::{Document, Principal};
use anda_db::schema::DocumentId;
use anda_engine::memory::{Conversation, ConversationStatus};
use parking_lot::RwLock;
use std::collections::VecDeque;

pub use formation::*;
pub use maintenance::*;
pub use recall::*;

#[async_trait::async_trait]
pub trait BrainHook: Send + Sync {
    fn is_maintenance_processing(&self) -> bool;
    async fn on_conversation_end(&self, agent_name: &str, conversation: &Conversation);
    async fn try_start_formation(&self);
    async fn try_start_maintenance(&self, formation_id: DocumentId) -> Option<DocumentId>;
}

/// Principal ID: uuc56-gyb
pub static SELF_USER_ID: Principal = Principal::from_slice(&[1]);
pub static SYSTEM_PROMPT_DYNAMIC_BOUNDARY: &str = "__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__";

pub(super) fn push_completed_history(
    history: &RwLock<VecDeque<Document>>,
    conversation: &Conversation,
    max_len: usize,
) {
    if conversation.status != ConversationStatus::Completed || max_len == 0 {
        return;
    }

    let doc: Document = conversation.clone().into();
    let mut history = history.write();
    history.push_back(doc);
    let len = history.len();
    if len > max_len {
        history.drain(0..(len - max_len));
    }
}

#[cfg(test)]
mod tests {
    use super::push_completed_history;
    use anda_engine::memory::{Conversation, ConversationStatus};
    use parking_lot::RwLock;
    use std::collections::VecDeque;

    #[test]
    fn push_completed_history_ignores_working_conversations_and_caps_length() {
        let history = RwLock::new(VecDeque::new());
        let mut conversation = Conversation {
            _id: 1,
            status: ConversationStatus::Working,
            ..Default::default()
        };

        push_completed_history(&history, &conversation, 2);
        assert!(history.read().is_empty());

        conversation.status = ConversationStatus::Completed;
        push_completed_history(&history, &conversation, 2);

        conversation._id = 2;
        push_completed_history(&history, &conversation, 2);

        conversation._id = 3;
        push_completed_history(&history, &conversation, 2);

        assert_eq!(history.read().len(), 2);
    }
}
