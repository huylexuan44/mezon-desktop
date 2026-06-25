mod panel;

pub use panel::{InboxPopoverPanel, clan_has_inbox_badge};

use mezon_store::InboxCategory;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InboxTab {
    ForYou,
    Messages,
    #[default]
    Mentions,
    Topics,
}

impl InboxTab {
    pub fn all() -> [Self; 4] {
        [Self::ForYou, Self::Messages, Self::Mentions, Self::Topics]
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::ForYou => "notifications.tabs.forYou",
            Self::Messages => "notifications.tabs.messages",
            Self::Mentions => "notifications.tabs.mentions",
            Self::Topics => "notifications.tabs.topics",
        }
    }

    pub fn category(self) -> Option<InboxCategory> {
        match self {
            Self::ForYou => Some(InboxCategory::ForYou),
            Self::Messages => Some(InboxCategory::Messages),
            Self::Mentions => Some(InboxCategory::Mentions),
            Self::Topics => None,
        }
    }
}
