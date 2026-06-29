//! Message list rendering, ported 1:1 from the Mezon React web app.
//!
//! Layering mirrors React: [`timeline`] owns scroll/viewport/pagination
//! (`ChannelMessages`), [`dispatch`] routes each row by type
//! (`ChannelMessage`), and the row/part modules render the visuals
//! (`MessageWithUser`, `MessageWithSystem`, and their children).

mod channel_messages;
mod content;
mod context;
mod dispatch;
mod parts;
mod skeleton;
mod system_row;
mod user_row;

pub use channel_messages::ChannelMessages;
