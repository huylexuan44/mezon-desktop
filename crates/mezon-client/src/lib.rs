// mezon-client: Rust equivalent of mezon-js
// Handles REST API calls and WebSocket connection to Mezon backend.

pub mod abridged_tcp_adapter;
pub mod app_api;
pub mod auth;
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
pub use network_monitor::NetworkMonitor;
pub use session::Session;
pub use transport::MezonTransport;
pub use transport::RealtimeEvent;
pub use transport::{
    ApiCategoryDesc, ApiChannelApp, ApiChannelDesc, ApiThreadDesc,
    ApiVoiceChannelUser,
};
pub use transport_adapter::TransportAdapter;
pub use transport_runtime::TransportClient;

/// Default WebSocket host (used for Stage 2+ WebSocket connection).
pub const DEFAULT_WS_HOST: &str = "sock.mezon.ai";
pub const DEFAULT_WS_PORT: u16 = 443;
pub const DEFAULT_WS_SECURE: bool = true;
