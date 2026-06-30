use std::collections::HashMap;

use gpui::{Entity, WeakEntity};
use mezon_store::{MessageId, ProfileContext, Settings};

use super::channel_messages::ChannelMessages;
use super::gif_video::GifVideoView;
use super::video_player::VideoPlayerView;
use crate::image_cache::LruImageCache;
use crate::theme::Theme;

pub struct RowCtx<'a> {
    pub theme: &'a Theme,
    pub locale: &'a str,
    pub current_user_id: &'a str,
    pub suppress_hover: bool,
    pub avatar_cache: Entity<LruImageCache>,
    pub unread_boundary_id: Option<MessageId>,
    pub highlight_id: Option<MessageId>,
    pub profile_context: Option<ProfileContext>,
    pub settings: Entity<Settings>,
    pub active_videos: &'a HashMap<(MessageId, usize), Entity<VideoPlayerView>>,
    pub gif_videos: &'a HashMap<(MessageId, usize), Entity<GifVideoView>>,
    pub video_host: WeakEntity<ChannelMessages>,
}

pub const DEFAULT_DISPLAY_NAME_COLOR: u32 = 0x17_ac_86;
pub const REPLY_USERNAME_COLOR: u32 = 0x84_ad_ff;

pub const CONTENT_INSET: f32 = 72.0;
pub const AVATAR_SIZE: f32 = 40.0;
pub const AVATAR_LEFT: f32 = 16.0;
pub const CONTENT_RIGHT_PAD: f32 = 48.0;
pub const REPLY_INSET: f32 = 36.0;
