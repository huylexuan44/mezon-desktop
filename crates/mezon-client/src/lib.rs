// mezon-client: Rust equivalent of mezon-js
// Handles REST API calls and WebSocket connection to Mezon backend.

pub mod abridged_tcp_adapter;
pub mod app_api;
pub mod auth;
pub mod image_disk_cache;
pub mod inbox;
pub mod keychain;
pub mod network_monitor;
pub mod session;
pub mod tls_crypto;
pub mod transport;
pub mod transport_adapter;
pub mod transport_runtime;

pub use abridged_tcp_adapter::AbridgedTcpAdapter;
pub use app_api::{AppApi, ConnectionStatus};
pub use auth::MezonClient;
pub use auth::QrLoginId;
pub use auth::{DEFAULT_API_HOST, DEFAULT_API_PORT, DEFAULT_API_SECURE, DEFAULT_SERVER_KEY};
pub use inbox::{
    DIRECTION_AROUND_TIMESTAMP, DIRECTION_BEFORE_TIMESTAMP, INBOX_PAGE_LIMIT, InboxCategory,
    InboxMentionSpan, InboxMessagePreview, InboxNotification, TopicDiscussion,
    attachment_link_is_image, display_text_from_message_content, inbox_notification_from_api,
    inbox_notifications_from_list, message_content_is_attachment, topic_discussion_from_api,
    topics_from_list,
};
pub use network_monitor::NetworkMonitor;
pub use session::Session;
pub use transport::MezonTransport;
pub use transport::RealtimeEvent;
pub use transport::{ApiCategoryDesc, ApiChannelApp, ApiChannelDesc, ApiVoiceChannelUser};
pub use transport_adapter::TransportAdapter;
pub use transport_runtime::TransportClient;

/// Default WebSocket host (used for Stage 2+ WebSocket connection).
pub const DEFAULT_WS_HOST: &str = "sock.mezon.ai";
pub const DEFAULT_WS_PORT: u16 = 443;
pub const DEFAULT_WS_SECURE: bool = true;
