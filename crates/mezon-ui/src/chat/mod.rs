pub mod area;
pub mod channel_header;
pub mod create_thread_panel;
pub mod grouping;
pub mod input_bar;
pub mod layout;
pub mod message_list;
pub mod message_row;
pub mod threads_popover;

pub use area::ChatArea;
pub use channel_header::ChannelHeader;
pub use grouping::is_combined;
pub use input_bar::InputBar;
pub use layout::ChatLayout;
pub use message_list::MessageTimeline;
pub use message_row::MessageRow;
pub use mezon_store::COMBINE_TIME_WINDOW;

#[derive(Debug, Clone)]
pub struct ReplyTarget {
    pub sender_name: String,
    pub content_preview: String,
}
