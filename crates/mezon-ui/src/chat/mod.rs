pub mod area;
pub mod channel_header;
pub mod grouping;
pub mod inbox;
pub mod input_bar;
pub mod layout;
pub mod member_list;
pub mod message_list;
pub mod message_row;
pub mod screen_share_modal;
pub mod voice;

pub use area::ChatArea;
pub use channel_header::ChannelHeader;
pub use grouping::is_combined;
pub use input_bar::InputBar;
pub use layout::ChatLayout;
pub use member_list::MemberListPanel;
pub use message_list::MessageTimeline;
pub use message_row::MessageRow;
pub use mezon_store::COMBINE_TIME_WINDOW;

#[derive(Debug, Clone)]
pub struct ReplyTarget {
    pub sender_name: String,
    pub content_preview: String,
}
