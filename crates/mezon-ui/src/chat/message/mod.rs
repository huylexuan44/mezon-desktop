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
mod gif_video;
// image-viewer: disabled, reimplement later
// mod image_viewer;
mod parts;
mod skeleton;
mod system_row;
mod time;
mod user_row;
mod video_player;

pub use channel_messages::ChannelMessages;
pub use context::DEFAULT_DISPLAY_NAME_COLOR;
