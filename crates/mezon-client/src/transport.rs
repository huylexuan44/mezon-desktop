/// Main Mezon transport client with WebSocket/TCP support and REST API methods.
///
/// Handles connection management, message routing, and provides typed API methods
/// for interacting with the Mezon backend.
pub use crate::transport_adapter::TransportAdapter;
use anyhow::{Context, Result};
use mezon_proto::{api, realtime};
use prost::Message;
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;
use tokio::sync::{RwLock, oneshot, watch};

const DEFAULT_SEND_TIMEOUT_MS: u64 = 10000;
const DEFAULT_CONNECT_GATE_MS: u64 = 5000;
const DEFAULT_PING_TIMEOUT_MS: u64 = 5000;

fn parse_id<T>(value: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse::<T>()
        .map_err(|e| anyhow::anyhow!("invalid id {value:?}: {e}"))
}


/// Promise executor for matching responses to requests.
struct PromiseExecutor {
    sender: oneshot::Sender<(u32, Vec<u8>)>,
}

/// Represents real-time events pushed from the server.
#[derive(Debug, Clone)]
pub enum RealtimeEvent {
    ChannelMessage(api::ChannelMessage),
    MessageTyping(realtime::MessageTypingEvent),
    ChannelPresence(realtime::ChannelPresenceEvent),
    StatusPresence(realtime::StatusPresenceEvent),
    CustomStatus(realtime::CustomStatusEvent),
    MessageReaction(api::MessageReaction),
    MarkAsRead(realtime::MarkAsRead),
    ChannelCreated(realtime::ChannelCreatedEvent),
    ChannelUpdated(realtime::ChannelUpdatedEvent),
    ChannelDeleted(realtime::ChannelDeletedEvent),
    VoiceStarted(realtime::VoiceStartedEvent),
    VoiceEnded(realtime::VoiceEndedEvent),
    VoiceJoined(realtime::VoiceJoinedEvent),
    VoiceLeaved(realtime::VoiceLeavedEvent),
    UserChannelAdded(realtime::UserChannelAdded),
    UserChannelRemoved(realtime::UserChannelRemoved),
    AddClanUser(realtime::AddClanUserEvent),
    UserClanRemoved(realtime::UserClanRemoved),
    ClanUpdated(realtime::ClanUpdatedEvent),
    ClanProfileUpdated(realtime::ClanProfileUpdatedEvent),
    ClanDeleted(realtime::ClanDeletedEvent),
    AddFriend(realtime::AddFriend),
    RemoveFriend(realtime::RemoveFriend),
    /// Server-pushed session refresh over the socket (`refresh_session_event`, field 96).
    /// The native equivalent of mezon-js `client.onrefreshsession`.
    SessionRefreshed(api::Session),
    Notifications(realtime::Notifications),
    Unhandled(realtime::envelope::Message),
}

impl TryFrom<realtime::envelope::Message> for RealtimeEvent {
    type Error = &'static str;

    fn try_from(msg: realtime::envelope::Message) -> Result<Self, Self::Error> {
        match msg {
            realtime::envelope::Message::ChannelMessage(m) => Ok(Self::ChannelMessage(m)),
            realtime::envelope::Message::MessageTypingEvent(m) => Ok(Self::MessageTyping(m)),
            realtime::envelope::Message::ChannelPresenceEvent(m) => Ok(Self::ChannelPresence(m)),
            realtime::envelope::Message::StatusPresenceEvent(m) => Ok(Self::StatusPresence(m)),
            realtime::envelope::Message::CustomStatusEvent(m) => Ok(Self::CustomStatus(m)),
            realtime::envelope::Message::MessageReactionEvent(m) => Ok(Self::MessageReaction(m)),
            realtime::envelope::Message::MarkAsRead(m) => Ok(Self::MarkAsRead(m)),
            realtime::envelope::Message::ChannelCreatedEvent(m) => Ok(Self::ChannelCreated(m)),
            realtime::envelope::Message::ChannelUpdatedEvent(m) => Ok(Self::ChannelUpdated(m)),
            realtime::envelope::Message::ChannelDeletedEvent(m) => Ok(Self::ChannelDeleted(m)),
            realtime::envelope::Message::VoiceStartedEvent(m) => Ok(Self::VoiceStarted(m)),
            realtime::envelope::Message::VoiceEndedEvent(m) => Ok(Self::VoiceEnded(m)),
            realtime::envelope::Message::VoiceJoinedEvent(m) => Ok(Self::VoiceJoined(m)),
            realtime::envelope::Message::VoiceLeavedEvent(m) => Ok(Self::VoiceLeaved(m)),
            realtime::envelope::Message::UserChannelAddedEvent(m) => Ok(Self::UserChannelAdded(m)),
            realtime::envelope::Message::UserChannelRemovedEvent(m) => {
                Ok(Self::UserChannelRemoved(m))
            }
            realtime::envelope::Message::AddClanUserEvent(m) => Ok(Self::AddClanUser(m)),
            realtime::envelope::Message::UserClanRemovedEvent(m) => Ok(Self::UserClanRemoved(m)),
            realtime::envelope::Message::ClanUpdatedEvent(m) => Ok(Self::ClanUpdated(m)),
            realtime::envelope::Message::ClanProfileUpdatedEvent(m) => {
                Ok(Self::ClanProfileUpdated(m))
            }
            realtime::envelope::Message::ClanDeletedEvent(m) => Ok(Self::ClanDeleted(m)),
            realtime::envelope::Message::AddFriend(m) => Ok(Self::AddFriend(m)),
            realtime::envelope::Message::RemoveFriend(m) => Ok(Self::RemoveFriend(m)),
            realtime::envelope::Message::RefreshSessionEvent(s) => Ok(Self::SessionRefreshed(s)),
            realtime::envelope::Message::Notifications(n) => Ok(Self::Notifications(n)),
            other => Ok(Self::Unhandled(other)),
        }
    }
}

fn dispatch_realtime_push(
    cid: u16,
    payload: &[u8],
    on_event: &(dyn Fn(RealtimeEvent) + Send + Sync),
) {
    match realtime::Envelope::decode(payload) {
        Ok(envelope) => match envelope.message {
            Some(msg) => {
                tracing::debug!("server push (cid={cid}) -> publishing realtime event");
                if let Ok(event) = RealtimeEvent::try_from(msg) {
                    on_event(event);
                }
            }
            None => tracing::warn!("server push (cid={cid}): envelope has no message"),
        },
        Err(e) => tracing::warn!(
            "server push (cid={cid}) decode failed (len={}): {e}",
            payload.len()
        ),
    }
}

/// Main transport client.
pub struct MezonTransport {
    adapter: Arc<dyn TransportAdapter>,
    cid_counter: Arc<AtomicU16>,
    pending_requests: Arc<RwLock<HashMap<u16, PromiseExecutor>>>,
    send_timeout_ms: Duration,
    connect_gate: Duration,
    connected_tx: watch::Sender<bool>,
    connected_rx: watch::Receiver<bool>,
    #[allow(dead_code)]
    base_path: String,
}

impl MezonTransport {
    /// Create a new transport with the given adapter.
    pub fn new(adapter: Box<dyn TransportAdapter>, base_path: String) -> Self {
        let (connected_tx, connected_rx) = watch::channel(false);
        Self {
            adapter: Arc::from(adapter),
            cid_counter: Arc::new(AtomicU16::new(1)),
            pending_requests: Arc::new(RwLock::new(HashMap::new())),
            send_timeout_ms: Duration::from_millis(DEFAULT_SEND_TIMEOUT_MS),
            connect_gate: Duration::from_millis(DEFAULT_CONNECT_GATE_MS),
            connected_tx,
            connected_rx,
            base_path,
        }
    }

    /// Set request timeout.
    pub fn set_timeout(&mut self, timeout_ms: u64) {
        self.send_timeout_ms = Duration::from_millis(timeout_ms);
    }

    /// Generate a unique correlation ID.
    fn generate_cid(&self) -> u16 {
        loop {
            let cid = self.cid_counter.fetch_add(1, Ordering::SeqCst);
            if cid != 0 {
                return cid;
            }
        }
    }

    async fn wait_connected(&self, deadline: Duration) -> Result<()> {
        let mut rx = self.connected_rx.clone();
        if *rx.borrow_and_update() {
            return Ok(());
        }
        match tokio::time::timeout(deadline, async move {
            while rx.changed().await.is_ok() {
                if *rx.borrow_and_update() {
                    return true;
                }
            }
            false
        })
        .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(anyhow::anyhow!("connection signal closed")),
            Err(_) => Err(anyhow::anyhow!("not connected (gate timed out)")),
        }
    }

    /// Connect to the Mezon backend.
    pub async fn connect(
        &self,
        host: &str,
        port: u16,
        token: &str,
        on_event: impl Fn(RealtimeEvent) + Send + Sync + 'static,
        on_disconnected: impl Fn(bool) + Send + Sync + 'static,
    ) -> Result<()> {
        tracing::debug!("MezonTransport::connect() starting");
        tracing::debug!("  Host: {}, Port: {}", host, port);

        // Set up message handler
        tracing::debug!("Setting up message handler...");
        let pending_requests = self.pending_requests.clone();
        let on_event: Arc<dyn Fn(RealtimeEvent) + Send + Sync> = Arc::new(on_event);
        self.adapter
            .set_on_message(Arc::new(move |cid, code, message| {
                tracing::trace!("on_message: cid={cid} code={code} len={}", message.len());

                if cid != 0 {
                    let pending = pending_requests.clone();
                    let on_event = on_event.clone();
                    tokio::spawn(async move {
                        let executor = pending.write().await.remove(&cid);
                        match executor {
                            Some(executor) => {
                                let _ = executor.sender.send((code, message));
                            }
                            None => dispatch_realtime_push(cid, &message, on_event.as_ref()),
                        }
                    });
                } else {
                    dispatch_realtime_push(cid, &message, on_event.as_ref());
                }
            }))
            .await;
        tracing::debug!("  Message handler set");

        let connected_for_open = self.connected_tx.clone();
        self.adapter
            .set_on_open(Arc::new(move || {
                let _ = connected_for_open.send(true);
            }))
            .await;

        // Set up close handler
        tracing::debug!("Setting up close handler...");
        let pending_for_close = self.pending_requests.clone();
        let connected_for_close = self.connected_tx.clone();
        self.adapter
            .set_on_close(Arc::new(move |was_clean| {
                let _ = connected_for_close.send(false);
                let pending = pending_for_close.clone();
                tokio::spawn(async move {
                    pending.write().await.clear();
                });
                on_disconnected(was_clean);
            }))
            .await;
        tracing::debug!("  Close handler set");

        self.adapter
            .set_on_error(Arc::new(|err| {
                tracing::warn!("realtime transport error: {err}");
            }))
            .await;

        // Connect
        tracing::debug!("Calling adapter.connect()...");
        self.adapter
            .connect(host, port, token)
            .await
            .with_context(|| format!("Failed to connect adapter to {host}:{port}"))?;

        tracing::debug!("MezonTransport::connect() completed successfully");
        Ok(())
    }

    /// Send a raw message and wait for response.
    pub async fn send(&self, cid: u16, message: Vec<u8>) -> Result<(u32, Vec<u8>)> {
        self.wait_connected(self.connect_gate).await?;
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.write().await;
            pending.insert(cid, PromiseExecutor { sender: tx });
        }
        if let Err(e) = self.adapter.send(message).await {
            self.pending_requests.write().await.remove(&cid);
            return Err(e);
        }
        let result = tokio::time::timeout(self.send_timeout_ms, rx)
            .await
            .map_err(|_| {
                let pending = self.pending_requests.clone();
                tokio::spawn(async move {
                    pending.write().await.remove(&cid);
                });
                anyhow::anyhow!("Request timed out")
            })?
            .map_err(|_| anyhow::anyhow!("Response channel closed"))?;
        Ok(result)
    }

    /// Check if the adapter is connected.
    pub async fn is_open(&self) -> bool {
        self.adapter.is_open()
    }

    /// Close the connection.
    pub async fn close(&self) -> Result<()> {
        let _ = self.connected_tx.send(false);
        self.pending_requests.write().await.clear();
        self.adapter.close().await
    }

    /// Send a ping.
    pub async fn ping(&self, cid: u16) -> Result<()> {
        self.adapter.send_ping(cid).await
    }

    /// Send a ping and wait for matching pong.
    pub async fn ping_roundtrip(&self) -> Result<()> {
        let cid = self.generate_cid();
        tracing::debug!("MezonTransport::ping_roundtrip() cid={}", cid);

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.write().await;
            pending.insert(cid, PromiseExecutor { sender: tx });
            tracing::debug!(
                "  Registered ping pending request. Total pending: {}",
                pending.len()
            );
        }

        tracing::debug!("Sending ping cid={}", cid);
        if let Err(e) = self.adapter.send_ping(cid).await {
            self.pending_requests.write().await.remove(&cid);
            return Err(e);
        }

        tokio::time::timeout(Duration::from_millis(DEFAULT_PING_TIMEOUT_MS), rx)
            .await
            .map_err(|_| {
                tracing::error!("Ping timed out after {} ms", DEFAULT_PING_TIMEOUT_MS);
                let pending = self.pending_requests.clone();
                tokio::spawn(async move {
                    pending.write().await.remove(&cid);
                });
                anyhow::anyhow!("Ping timed out")
            })?
            .map_err(|_| anyhow::anyhow!("Ping response channel closed"))?;

        tracing::debug!("Pong received for cid={}", cid);
        Ok(())
    }
}

// ============================================================================
// API Methods - Hot Path (frequently called)
// ============================================================================

/// API response types (simplified - expand as needed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiAccount {
    pub user_id: i64,
    pub username: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub about_me: Option<String>,
    pub phone_number: Option<String>,
    pub password_setted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiSession {
    pub token: String,
    pub refresh_token: String,
    pub user_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiChannelDesc {
    pub channel_id: i64,
    pub channel_label: String,
    pub channel_type: u32,
    pub clan_id: i64,
    pub category_name: String,
    pub category_id: i64,
    pub channel_private: i32,
    pub count_mess_unread: i32,
    pub member_count: i32,
    pub parent_id: i64,
    pub is_mute: bool,
    pub last_seen_message_id: i64,
    pub last_seen_timestamp: i64,
    pub last_sent_message_id: i64,
    pub last_sent_timestamp: i64,
    pub badge_count: i32,
}

/// A direct-message / group conversation descriptor (clan_id = 0 namespace). Unlike
/// [`ApiChannelDesc`] this carries the DM participant arrays the UI needs to render rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDirectChannel {
    pub channel_id: i64,
    pub channel_label: String,
    /// Raw channel type: 2 = group, 3 = 1-1 DM.
    pub channel_type: u32,
    pub channel_avatar: String,
    pub avatars: Vec<String>,
    pub usernames: Vec<String>,
    pub display_names: Vec<String>,
    pub user_ids: Vec<i64>,
    pub onlines: Vec<bool>,
    pub member_count: i32,
    pub count_mess_unread: i32,
    pub last_sent_timestamp: i64,
    pub last_seen_timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiCategoryDesc {
    pub category_id: i64,
    pub category_name: String,
    pub clan_id: i64,
    pub category_order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiVoiceChannelUser {
    pub channel_id: i64,
    pub user_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiChannelApp {
    pub app_id: String,
    pub app_name: String,
    pub app_logo: Option<String>,
    pub app_url: String,
    pub channel_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiClanDesc {
    pub clan_id: i64,
    pub clan_name: String,
    pub creator_id: i64,
    pub logo: String,
    pub banner: String,
    pub welcome_channel_id: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiAttachment {
    pub url: String,
    pub filename: String,
    pub filetype: String,
    pub width: i32,
    pub height: i32,
}

fn parse_message_attachments(bytes: &[u8]) -> Vec<ApiAttachment> {
    if bytes.is_empty() {
        return Vec::new();
    }
    match api::MessageAttachmentList::decode(bytes) {
        Ok(list) => list
            .attachments
            .into_iter()
            .filter(|a| !a.url.is_empty())
            .map(|a| ApiAttachment {
                url: a.url,
                filename: a.filename,
                filetype: a.filetype,
                width: a.width,
                height: a.height,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(
                "failed to decode message attachments ({} bytes): {e}",
                bytes.len()
            );
            Vec::new()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiMessage {
    pub message_id: i64,
    pub content: String,
    pub sender_id: i64,
    pub sender_name: String,
    pub avatar: String,
    pub create_time: i64,
    pub attachments: Vec<ApiAttachment>,
}

impl MezonTransport {
    /// Build a protobuf-encoded API request envelope.
    ///
    /// Wire format: Envelope { cid: uint32, api_request_event: ApiRequestEvent }
    fn build_api_request(&self, cid: u16, api_name: &str, body: Vec<u8>) -> Result<Vec<u8>> {
        let api_index = self
            .get_api_index(api_name)
            .ok_or_else(|| anyhow::anyhow!("unknown API name: {api_name}"))?;
        let envelope = realtime::Envelope {
            cid: i32::from(cid),
            message: Some(realtime::envelope::Message::ApiRequestEvent(
                realtime::ApiRequestEvent {
                    api_index: api_index as i32,
                    api_name: api_name.to_string(),
                    body,
                },
            )),
        };
        Ok(envelope.encode_to_vec())
    }

    async fn send_api_request(
        &self,
        cid: u16,
        api_name: &str,
        body: Vec<u8>,
    ) -> Result<(u32, Vec<u8>)> {
        self.send(cid, self.build_api_request(cid, api_name, body)?)
            .await
    }

    fn account_from_user(
        user: api::User,
        email: Option<String>,
        password_setted: bool,
    ) -> ApiAccount {
        ApiAccount {
            user_id: user.id,
            username: user.username,
            email,
            display_name: (!user.display_name.is_empty()).then_some(user.display_name),
            avatar_url: (!user.avatar_url.is_empty()).then_some(user.avatar_url),
            about_me: (!user.about_me.is_empty()).then_some(user.about_me),
            phone_number: (!user.phone_number.is_empty()).then_some(user.phone_number),
            password_setted,
        }
    }

    fn channel_desc_from_proto(channel: api::ChannelDescription) -> ApiChannelDesc {
        let last_seen_message_id = channel
            .last_seen_message
            .as_ref()
            .map(|m| m.id)
            .unwrap_or_default();
        let last_seen_timestamp = channel
            .last_seen_message
            .as_ref()
            .map(|m| i64::from(m.timestamp_seconds))
            .unwrap_or(0);
        let last_sent_message_id = channel
            .last_sent_message
            .as_ref()
            .map(|m| m.id)
            .unwrap_or_default();
        let last_sent_timestamp = channel
            .last_sent_message
            .as_ref()
            .map(|m| i64::from(m.timestamp_seconds))
            .unwrap_or(0);
        ApiChannelDesc {
            channel_id: channel.channel_id,
            channel_label: channel.channel_label,
            channel_type: channel.r#type as u32,
            clan_id: channel.clan_id,
            category_name: channel.category_name,
            category_id: channel.category_id,
            channel_private: channel.channel_private,
            count_mess_unread: channel.count_mess_unread,
            member_count: channel.member_count,
            parent_id: channel.parent_id,
            is_mute: channel.is_mute,
            last_seen_message_id,
            last_seen_timestamp,
            last_sent_message_id,
            last_sent_timestamp,
            badge_count: channel.count_mess_unread,
        }
    }

    fn direct_channel_from_proto(channel: api::ChannelDescription) -> ApiDirectChannel {
        let last_seen_timestamp = channel
            .last_seen_message
            .as_ref()
            .map(|m| i64::from(m.timestamp_seconds))
            .unwrap_or(0);
        let last_sent_timestamp = channel
            .last_sent_message
            .as_ref()
            .map(|m| i64::from(m.timestamp_seconds))
            .unwrap_or(0);
        ApiDirectChannel {
            channel_id: channel.channel_id,
            channel_label: channel.channel_label,
            channel_type: channel.r#type as u32,
            channel_avatar: channel.channel_avatar,
            avatars: channel.avatars,
            usernames: channel.usernames,
            display_names: channel.display_names,
            user_ids: channel.user_ids,
            onlines: channel.onlines,
            member_count: channel.member_count,
            count_mess_unread: channel.count_mess_unread,
            last_sent_timestamp,
            last_seen_timestamp,
        }
    }

    fn category_desc_from_proto(cat: api::CategoryDesc) -> ApiCategoryDesc {
        ApiCategoryDesc {
            category_id: cat.category_id,
            category_name: cat.category_name,
            clan_id: cat.clan_id,
            category_order: cat.category_order,
        }
    }

    fn clan_desc_from_proto(clan: api::ClanDesc) -> ApiClanDesc {
        ApiClanDesc {
            clan_id: clan.clan_id,
            clan_name: clan.clan_name,
            creator_id: clan.creator_id,
            logo: clan.logo,
            banner: clan.banner,
            welcome_channel_id: clan.welcome_channel_id,
        }
    }

    pub fn message_from_proto(message: api::ChannelMessage) -> ApiMessage {
        let content = serde_json::from_str::<serde_json::Value>(&message.content)
            .ok()
            .and_then(|v| v.get("t").and_then(|t| t.as_str().map(|s| s.to_string())))
            .unwrap_or_else(|| message.content.clone());

        let sender_name = if !message.clan_nick.is_empty() {
            message.clan_nick.clone()
        } else if !message.display_name.is_empty() {
            message.display_name.clone()
        } else {
            message.username.clone()
        };

        let attachments = parse_message_attachments(&message.attachments);

        ApiMessage {
            message_id: message.message_id,
            content,
            sender_id: message.sender_id,
            sender_name,
            avatar: message.avatar,
            create_time: i64::from(message.create_time_seconds),
            attachments,
        }
    }

    /// Get API index from API name (matches TypeScript ApiNameEnum order)
    fn get_api_index(&self, api_name: &str) -> Option<u32> {
        let index = match api_name {
            // HOT PATH
            "ListChannelDescs" => 0,
            "GetAccount" => 1,
            "ListClanDescs" => 2,
            "ListClanUsers" => 3,
            "ListRoles" => 4,
            "ListEvents" => 5,
            "GetRoleOfUserInTheClan" => 6,
            "GetListPermission" => 7,
            "ListUserPermissionInChannel" => 8,
            "GetNotificationClan" => 9,
            "ListMutedChannel" => 10,
            "ListStreamingChannelUsers" => 11,
            "ListQuickMenuAccess" => 12,
            "GetNotificationChannel" => 13,
            "ListFriends" => 14,
            "EmojiRecentList" => 15,
            "GetListEmojisByUserId" => 16,
            "ListClanBadgeCount" => 17,
            "ListChannelBadgeCount" => 18,
            "ListLogedDevice" => 19,
            "ListClanUsersStatus" => 20,
            "ListChannelApps" => 21,
            "GetListFavoriteChannel" => 22,
            "ListCategoryDescs" => 23,
            "ListOnboarding" => 24,
            "GetListStickersByUserId" => 25,
            "GetSystemMessageByClanId" => 26,
            "GetPinMessagesList" => 27,
            "GetChannelCanvasList" => 28,
            "ListChannelTimeline" => 29,
            "ListChannelMessages" => 30,
            "ListActivity" => 31,
            "ListChannelByUserId" => 32,
            "ListUserClansByUserId" => 33,
            "GetUserProfileOnClan" => 34,
            "RegistFCMDeviceToken" => 35,
            "IsBanned" => 36,
            "ListThreadDescs" => 37,
            "ListArchivedChannelDescs" => 38,
            "ListChannelDetail" => 39,
            "GetChannelCategoryNotiSettingsList" => 40,
            "ListRoleUsers" => 41,
            "ListChannelUsers" => 42,
            "ListChannelAttachment" => 43,
            "ListChannelVoiceUsers" => 44,
            "ListUserOnline" => 45,
            "ListNotifications" => 46,
            "ListChannelUsersUC" => 47,
            "ListWebhookByChannelId" => 48,
            "GetPermissionByRoleIdChannelId" => 49,
            "ListChannelSetting" => 50,
            "ListApps" => 51,
            "GetApp" => 52,
            "ListForSaleItems" => 53,
            "ListClanWebhook" => 54,
            "GetUserStatus" => 55,
            "ListSdTopic" => 56,
            // COLD PATH
            "AddFriends" => 57,
            "AddChannelUsers" => 58,
            "RegistrationEmail" => 59,
            "BlockFriends" => 60,
            "UnblockFriends" => 61,
            "UploadAttachmentFile" => 62,
            "UploadOauthFile" => 63,
            "AddRolesChannelDesc" => 64,
            "CreateCategoryDesc" => 65,
            "CreateChannelDesc" => 66,
            "CreateRole" => 67,
            "CreateEvent" => 68,
            "DeleteRole" => 69,
            "DeleteEvent" => 70,
            "DeleteRoleChannelDesc" => 71,
            "DeleteChannelDesc" => 72,
            "CloseDMByChannelId" => 73,
            "OpenDMByChannelId" => 74,
            "DeleteAccount" => 75,
            "DeleteFriends" => 76,
            "DeleteCategoryDesc" => 77,
            "DeleteNotifications" => 78,
            "DeleteClanDesc" => 79,
            "UpdateUser" => 80,
            "UpdateUserProfileByClan" => 81,
            "UpdateClanOrder" => 82,
            "RemoveChannelUsers" => 83,
            "LeaveThread" => 84,
            "ArchiveChannel" => 85,
            "LinkSMS" => 86,
            "ConfirmLinkMezonOTP" => 87,
            "LinkEmail" => 88,
            "CreateClanDesc" => 89,
            "RemoveClanUsers" => 90,
            "BanClanUsers" => 91,
            "CreateLinkInviteUser" => 92,
            "InviteUser" => 93,
            "SetRoleChannelPermission" => 94,
            "SetNotificationChannelSetting" => 95,
            "SetMuteChannel" => 96,
            "SetMuteCategory" => 97,
            "SetNotificationClanSetting" => 98,
            "SetNotificationCategorySetting" => 99,
            "DeleteNotificationCategorySetting" => 100,
            "DeleteNotificationChannel" => 101,
            "CreatePinMessage" => 102,
            "CreateMessage2Inbox" => 103,
            "UnlinkMezon" => 104,
            "UnlinkEmail" => 105,
            "UpdateAccount" => 106,
            "UpdateUsername" => 107,
            "UpdateCategory" => 108,
            "UpdateCategoryOrder" => 109,
            "UpdateRoleOrder" => 110,
            "UpdateClanDesc" => 111,
            "UpdateChannelDesc" => 112,
            "UpdateChannelPrivate" => 113,
            "UpdateRole" => 114,
            "UpdateEvent" => 115,
            "SearchMessage" => 116,
            "CreateClanEmoji" => 117,
            "DeleteByIdClanEmoji" => 118,
            "UpdateClanEmojiById" => 119,
            "GenerateWebhook" => 120,
            "HandleWebhook" => 121,
            "UpdateWebhookById" => 122,
            "DeleteWebhookById" => 123,
            "AddClanSticker" => 124,
            "UpdateClanStickerById" => 125,
            "DeleteClanStickerById" => 126,
            "ChangeChannelCategory" => 127,
            "CheckDuplicateName" => 128,
            "AddApp" => 129,
            "DeleteApp" => 130,
            "UpdateApp" => 131,
            "AddAppToClan" => 132,
            "CreateSystemMessage" => 133,
            "UpdateSystemMessage" => 134,
            "DeleteSystemMessage" => 135,
            "StreamingServerCallback" => 136,
            "EditChannelCanvases" => 137,
            "GetChannelCanvasDetail" => 138,
            "DeleteChannelCanvas" => 139,
            "AddChannelFavorite" => 140,
            "RemoveChannelFavorite" => 141,
            "CreateActiviy" => 142,
            "GetPubKeys" => 143,
            "PushPubKey" => 144,
            "GetChanEncryptionMethod" => 145,
            "SetChanEncryptionMethod" => 146,
            "GetKeyServer" => 147,
            "ListAuditLog" => 148,
            "GetOnboardingDetail" => 149,
            "CreateOnboarding" => 150,
            "UpdateOnboarding" => 151,
            "DeleteOnboarding" => 152,
            "ListOnboardingStep" => 153,
            "UpdateOnboardingStep" => 154,
            "GenerateClanWebhook" => 155,
            "UpdateClanWebhookById" => 156,
            "DeleteClanWebhookById" => 157,
            "HandleClanWebhook" => 158,
            "UpdateUserStatus" => 159,
            "UpdateUserCustomStatus" => 160,
            "GetTopicDetail" => 161,
            "CreateSdTopic" => 162,
            "DeleteSdTopic" => 163,
            "CreateExternalMezonMeet" => 164,
            "GenerateMeetToken" => 165,
            "RemoveParticipantMezonMeet" => 166,
            "MuteParticipantMezonMeet" => 167,
            "CreateRoomChannelApps" => 168,
            "GetMezonOauthClient" => 169,
            "DeleteMezonOauthClient" => 170,
            "UpdateMezonOauthClient" => 171,
            "SearchThread" => 172,
            "GenerateHashChannelApps" => 173,
            "DeleteUserEvent" => 174,
            "AddUserEvent" => 175,
            "DeleteQuickMenuAccess" => 176,
            "AddQuickMenuAccess" => 177,
            "UpdateQuickMenuAccess" => 178,
            "TransferOwnership" => 179,
            "SendChannelMessage" => 180,
            "UpdateChannelMessage" => 181,
            "DeleteChannelMessage" => 182,
            "ReportMessageAbuse" => 183,
            "MessageButtonClick" => 184,
            "DropdownBoxSelected" => 185,
            "ActiveArchivedThread" => 186,
            "UpdateChannelTimeline" => 187,
            "AddAgentToChannel" => 188,
            "DisconnectAgent" => 189,
            "CreateChannelTimeline" => 190,
            "DetailChannelTimeline" => 191,
            "CreatePoll" => 192,
            "VotePoll" => 193,
            "ClosePoll" => 194,
            "GetPoll" => 195,
            "ReactChannelMessage" => 196,
            "MultipartUploadAttachmentFileStart" => 197,
            "MultipartUploadAttachmentFileFinish" => 198,
            "SessionRefresh" => 199,
            "SessionLogout" => 200,
            "Healthcheck" => 201,
            "UnbanClanUsers" => 202,
            "ListBannedUsers" => 203,
            "GetNotificationCategory" => 204,
            "ListRolePermissions" => 205,
            "IsFollower" => 206,
            "DeletePinMessage" => 207,
            "MarkAsRead" => 208,
            "UploadBatchAttachmentFile" => 209,
            _ => {
                tracing::warn!("unknown API name: {api_name}");
                return None;
            }
        };
        Some(index)
    }

    /// Get the current user's account.
    pub async fn get_account(&self) -> Result<ApiAccount> {
        tracing::debug!("MezonTransport::get_account() called");

        let cid = self.generate_cid();
        tracing::debug!("  Generated CID: {}", cid);

        // Build API request envelope
        let api_name = "GetAccount";
        let body = Vec::new();

        tracing::debug!("  Building API request envelope...");
        tracing::debug!("    API name: {}", api_name);
        tracing::debug!("    API index: {:?}", self.get_api_index(api_name));
        tracing::debug!("    Body len: {}", body.len());

        let request_bytes = self.build_api_request(cid, api_name, body)?;
        tracing::debug!("  Request envelope size: {} bytes", request_bytes.len());

        tracing::debug!("  Calling self.send() with cid={}...", cid);
        let send_result = self.send(cid, request_bytes).await;

        match send_result {
            Ok((code, response)) => {
                tracing::debug!(
                    "Received response: code={}, len={} bytes",
                    code,
                    response.len()
                );

                if code != 0 {
                    tracing::error!("API error: code={}", code);
                    return Err(anyhow::anyhow!("API error: code={}", code));
                }

                if let Ok(envelope) = realtime::Envelope::decode(response.as_slice())
                    && let Some(realtime::envelope::Message::Error(error)) = envelope.message
                {
                    return Err(anyhow::anyhow!(
                        "GetAccount API error: code={} error={}",
                        error.code,
                        error.message
                    ));
                }

                let account = api::Account::decode(response.as_slice())?;
                let user = account
                    .user
                    .ok_or_else(|| anyhow::anyhow!("GetAccount response missing user"))?;
                let account = Self::account_from_user(
                    user,
                    (!account.email.is_empty()).then_some(account.email),
                    account.password_setted,
                );
                tracing::debug!("Decoded account response: {} bytes", response.len());
                Ok(account)
            }
            Err(e) => {
                tracing::error!("self.send() failed: {}", e);
                Err(e)
            }
        }
    }

    /// List users in a clan.
    pub async fn list_clan_users(&self, clan_id: i64) -> Result<Vec<api::ClanUserList>> {
        let cid = self.generate_cid();
        let body = api::ListClanUsersRequest { clan_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListClanUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let list = api::ClanUserList::decode(response.as_slice())?;
        Ok(vec![list])
    }

    /// List channels in a clan.
    pub async fn list_channel_descs(&self, clan_id: i64) -> Result<Vec<ApiChannelDesc>> {
        let cid = self.generate_cid();

        let api_name = "ListChannelDescs";
        let body = api::ListChannelDescsRequest {
            clan_id,
            limit: 500,
            state: 1,
            channel_type: 1,
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self.send_api_request(cid, api_name, body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!(
                "API error: code={} {}",
                code,
                String::from_utf8_lossy(&response).trim()
            ));
        }

        let channels = api::ChannelDescList::decode(response.as_slice())?;
        Ok(channels
            .channeldesc
            .into_iter()
            .map(Self::channel_desc_from_proto)
            .collect())
    }

    /// List the user's direct-message / group conversations (clan_id = 0). Mirrors mezon-react's
    /// `fetchDirectMessage` → `ListChannelDescs(clan_id="0", channel_type=GROUP)`, which returns
    /// both 1-1 DMs (type 3) and groups (type 2).
    pub async fn list_dm_channel_descs(&self) -> Result<Vec<ApiDirectChannel>> {
        let cid = self.generate_cid();

        let api_name = "ListChannelDescs";
        let body = api::ListChannelDescsRequest {
            clan_id: 0,
            limit: 500,
            state: 1,
            channel_type: 2,
            page: 1,
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self.send_api_request(cid, api_name, body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!(
                "API error: code={} {}",
                code,
                String::from_utf8_lossy(&response).trim()
            ));
        }

        let channels = api::ChannelDescList::decode(response.as_slice())?;
        Ok(channels
            .channeldesc
            .into_iter()
            .map(Self::direct_channel_from_proto)
            .collect())
    }

    /// List roles in a clan.
    pub async fn list_roles(
        &self,
        clan_id: i64,
        limit: i32,
        cursor: &str,
    ) -> Result<api::RoleListEventResponse> {
        let cid = self.generate_cid();
        let body = api::RoleListEventRequest {
            clan_id,
            limit,
            state: 0,
            cursor: cursor.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListRoles", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::RoleListEventResponse::decode(response.as_slice())?)
    }

    /// List user's clans.
    pub async fn list_clan_descs(&self) -> Result<Vec<ApiClanDesc>> {
        let cid = self.generate_cid();

        let body = api::ListClanDescRequest {
            limit: 50,
            state: 1,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListClanDescs", body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let clans = api::ClanDescList::decode(response.as_slice())?;
        Ok(clans
            .clandesc
            .into_iter()
            .map(Self::clan_desc_from_proto)
            .collect())
    }

    /// List users in a channel.
    pub async fn list_channel_users(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_type: i32,
    ) -> Result<api::ChannelUserList> {
        let cid = self.generate_cid();
        let body = api::ListChannelUsersRequest {
            clan_id,
            channel_id,
            channel_type,
            limit: 2000,
            state: 1,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListChannelUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelUserList::decode(response.as_slice())?)
    }

    /// List messages in a channel.
    pub async fn list_channel_messages(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        direction: i32,
        limit: u32,
    ) -> Result<Vec<ApiMessage>> {
        let cid = self.generate_cid();

        let api_name = "ListChannelMessages";
        let body = api::ListChannelMessagesRequest {
            clan_id,
            channel_id,
            message_id,
            direction,
            limit: limit as i32,
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self.send_api_request(cid, api_name, body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let messages = api::ChannelMessageList::decode(response.as_slice())?;
        let parsed: Vec<ApiMessage> = messages
            .messages
            .into_iter()
            .filter(|m| {
                if m.code != 0 {
                    tracing::warn!("Skipping message with code={}", m.code);
                    false
                } else {
                    true
                }
            })
            .map(Self::message_from_proto)
            .collect();
        tracing::debug!(
            "list_channel_messages: channel_id={channel_id} count={} response_bytes={}",
            parsed.len(),
            response.len(),
        );
        Ok(parsed)
    }

    /// Send a message to a channel.
    pub async fn join_chat(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_type: i32,
        is_public: bool,
    ) -> Result<()> {
        let cid = self.generate_cid();
        tracing::debug!(
            "join_chat: clan_id={clan_id} channel_id={channel_id} type={channel_type} is_public={is_public}"
        );
        let envelope = realtime::Envelope {
            cid: i32::from(cid),
            message: Some(realtime::envelope::Message::ChannelJoin(
                realtime::ChannelJoin {
                    clan_id,
                    channel_id,
                    channel_type,
                    is_public,
                },
            )),
        };
        let (code, _response) = self.send(cid, envelope.encode_to_vec()).await?;
        if code != 0 {
            anyhow::bail!("join_chat error: code={code}");
        }
        Ok(())
    }

    pub async fn send_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
    ) -> Result<ApiMessage> {
        self.send_channel_message_with_attachments(
            clan_id,
            channel_id,
            content,
            is_public,
            mode,
            vec![],
        )
        .await
    }

    pub async fn send_channel_message_with_attachments(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        attachments: Vec<api::MessageAttachment>,
    ) -> Result<ApiMessage> {
        let cid = self.generate_cid();

        let api_name = "SendChannelMessage";
        let parsed_clan_id: i64 = clan_id;
        let parsed_channel_id: i64 = channel_id;
        tracing::debug!(
            "send_channel_message: clan_id={} channel_id={} is_public={} content_len={} attachments={}",
            parsed_clan_id,
            parsed_channel_id,
            is_public,
            content.len(),
            attachments.len()
        );
        // mezon stores message content as JSON `{ "t": <text> }` (matches mezon-js), not raw text.
        let content_json = serde_json::json!({ "t": content }).to_string();
        // No client `id`: the server generates the message Snowflake (mezon-js omits it).
        // Sending a client-side id made the server reject with code 13 (INTERNAL).
        let body = realtime::ChannelMessageSend {
            clan_id: parsed_clan_id,
            channel_id: parsed_channel_id,
            content: content_json,
            attachments,
            mode,
            is_public,
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self.send_api_request(cid, api_name, body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let ack = realtime::ChannelMessageAck::decode(response.as_slice())?;
        tracing::debug!(
            "send_channel_message ack: message_id={} channel_id={} code={}",
            ack.message_id,
            ack.channel_id,
            ack.code
        );
        Ok(ApiMessage {
            message_id: ack.message_id,
            content: content.to_string(),
            sender_id: 0,
            sender_name: ack.username,
            avatar: String::new(),
            create_time: i64::from(ack.create_time_seconds),
            attachments: Vec::new(),
        })
    }

    /// List user's friends.
    pub async fn list_friends(&self) -> Result<Vec<ApiAccount>> {
        let cid = self.generate_cid();

        let api_name = "ListFriends";
        let body = api::ListFriendsRequest::default().encode_to_vec();

        let (code, response) = self.send_api_request(cid, api_name, body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let friends = api::FriendList::decode(response.as_slice())?;
        Ok(friends
            .friends
            .into_iter()
            .filter_map(|friend| {
                friend
                    .user
                    .map(|user| Self::account_from_user(user, None, false))
            })
            .collect())
    }

    /// List clan badge counts.
    pub async fn list_clan_badge_count(&self) -> Result<api::ListClanBadgeCountResponse> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListClanBadgeCount", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListClanBadgeCountResponse::decode(
            response.as_slice(),
        )?)
    }

    pub async fn list_clan_badge_count_typed(&self) -> Result<Vec<(String, i32, bool)>> {
        let raw = self.list_clan_badge_count().await?;
        Ok(raw
            .list_badge
            .into_iter()
            .map(|b| (b.clan_id.to_string(), b.badge, b.has_unread))
            .collect())
    }

    /// List channel badge counts.
    pub async fn list_channel_badge_count(
        &self,
        clan_id: i64,
    ) -> Result<api::ListChannelBadgeCountResponse> {
        let cid = self.generate_cid();
        let body = api::ListChannelBadgeCountRequest {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelBadgeCount", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListChannelBadgeCountResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List notifications.
    pub async fn list_notifications(
        &self,
        clan_id: i64,
        limit: i32,
        notification_id: i64,
        category: i32,
        direction: i32,
    ) -> Result<api::NotificationList> {
        let cid = self.generate_cid();
        let body = api::ListNotificationsRequest {
            clan_id,
            limit,
            notification_id,
            category,
            direction,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListNotifications", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::NotificationList::decode(response.as_slice())?)
    }

    /// Get user profile on a clan.
    pub async fn get_user_profile_on_clan(&self, clan_id: i64) -> Result<api::ClanProfile> {
        let cid = self.generate_cid();
        let body = api::ClanProfileRequest { clan_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetUserProfileOnClan", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ClanProfile::decode(response.as_slice())?)
    }

    /// List category descriptions in a clan.
    pub async fn list_category_descs(&self, clan_id: i64) -> Result<api::CategoryDescList> {
        let cid = self.generate_cid();
        let body = api::CategoryDesc {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListCategoryDescs", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CategoryDescList::decode(response.as_slice())?)
    }

    pub async fn list_categories_typed(&self, clan_id: i64) -> Result<Vec<ApiCategoryDesc>> {
        let raw = self.list_category_descs(clan_id).await?;
        Ok(raw
            .categorydesc
            .into_iter()
            .map(Self::category_desc_from_proto)
            .collect())
    }

    pub async fn list_channel_badge_counts(&self, clan_id: i64) -> Result<Vec<ApiChannelDesc>> {
        let raw = self.list_channel_badge_count(clan_id).await?;
        Ok(raw
            .channeldesc
            .into_iter()
            .map(Self::channel_desc_from_proto)
            .collect())
    }

    pub async fn list_voice_channel_users(&self, clan_id: i64) -> Result<Vec<ApiVoiceChannelUser>> {
        let raw = self.list_channel_voice_users(clan_id).await?;
        Ok(raw
            .voice_channel_users
            .into_iter()
            .map(|u| ApiVoiceChannelUser {
                channel_id: u.channel_id,
                user_ids: u
                    .user_ids
                    .iter()
                    .filter_map(|s| s.parse::<i64>().ok())
                    .collect(),
            })
            .collect())
    }

    /// List channel description detail.
    pub async fn list_channel_detail(&self, channel_id: i64) -> Result<api::ChannelDescription> {
        let cid = self.generate_cid();
        let body = api::ListChannelDetailRequest { channel_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelDetail", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelDescription::decode(response.as_slice())?)
    }

    /// List thread descriptions.
    pub async fn list_thread_descs(
        &self,
        channel_id: i64,
        clan_id: i64,
        limit: i32,
        page: i32,
    ) -> Result<api::ChannelDescList> {
        let cid = self.generate_cid();
        let body = api::ListThreadRequest {
            channel_id,
            clan_id,
            limit,
            page,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListThreadDescs", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelDescList::decode(response.as_slice())?)
    }

    /// List channels by user ID.
    pub async fn list_channel_by_user_id(&self) -> Result<Vec<ApiChannelDesc>> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListChannelByUserId", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let channel_list = api::ChannelDescList::decode(response.as_slice())?;
        Ok(channel_list
            .channeldesc
            .into_iter()
            .map(Self::channel_desc_from_proto)
            .collect())
    }

    /// Get notification settings for a clan.
    pub async fn get_notification_clan(&self, clan_id: i64) -> Result<i32> {
        let cid = self.generate_cid();

        let body = api::NotificationClan { clan_id }.encode_to_vec();

        let (code, response) = self
            .send_api_request(cid, "GetNotificationClan", body)
            .await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let setting = api::NotificationSetting::decode(response.as_slice())?;
        Ok(setting.notification_setting_type)
    }

    /// List events in a clan.
    pub async fn list_events(&self, clan_id: i64) -> Result<api::EventList> {
        let cid = self.generate_cid();
        let body = api::ListEventsRequest { clan_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListEvents", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EventList::decode(response.as_slice())?)
    }

    /// List activity.
    pub async fn list_activity(&self) -> Result<api::ListUserActivity> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListActivity", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListUserActivity::decode(response.as_slice())?)
    }

    pub async fn list_channel_apps(&self, clan_id: i64) -> Result<Vec<ApiChannelApp>> {
        let cid = self.generate_cid();
        let body = api::ListChannelAppsRequest { clan_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListChannelApps", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("ListChannelApps code={}", code));
        }
        let resp = api::ListChannelAppsResponse::decode(response.as_slice())?;
        Ok(resp
            .channel_apps
            .into_iter()
            .map(|a| ApiChannelApp {
                app_id: a.app_id.to_string(),
                app_name: a.app_name,
                app_logo: (!a.app_logo.is_empty()).then_some(a.app_logo),
                app_url: a.app_url,
                channel_id: a.channel_id,
            })
            .collect())
    }

    /// List emoji recent by user ID.
    pub async fn emoji_recent_list(&self) -> Result<api::EmojiRecentList> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "EmojiRecentList", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EmojiRecentList::decode(response.as_slice())?)
    }

    /// List emojis by user ID.
    pub async fn list_emojis_by_user_id(&self) -> Result<api::EmojiListedResponse> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetListEmojisByUserId", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EmojiListedResponse::decode(response.as_slice())?)
    }

    /// List stickers by user ID.
    pub async fn list_stickers_by_user_id(&self) -> Result<api::StickerListedResponse> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetListStickersByUserId", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::StickerListedResponse::decode(response.as_slice())?)
    }

    /// Get system message by clan ID.
    pub async fn get_system_message_by_clan_id(&self, clan_id: i64) -> Result<api::SystemMessage> {
        let cid = self.generate_cid();
        let body = api::GetSystemMessage { clan_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetSystemMessageByClanId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SystemMessage::decode(response.as_slice())?)
    }

    /// Get pin messages list.
    pub async fn get_pin_messages_list(
        &self,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<api::PinMessagesList> {
        let cid = self.generate_cid();
        let body = api::PinMessageRequest {
            channel_id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetPinMessagesList", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::PinMessagesList::decode(response.as_slice())?)
    }

    /// List channel timeline.
    pub async fn list_channel_timeline(
        &self,
        clan_id: i64,
        channel_id: i64,
        year: i32,
        limit: i32,
    ) -> Result<api::ListChannelTimelineResponse> {
        let cid = self.generate_cid();
        let body = api::ListChannelTimelineRequest {
            clan_id,
            channel_id,
            year,
            limit,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelTimeline", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListChannelTimelineResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Get role of user in clan.
    pub async fn get_role_of_user_in_clan(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<api::RoleList> {
        let cid = self.generate_cid();
        let body = api::ListPermissionOfUsersRequest {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetRoleOfUserInTheClan", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::RoleList::decode(response.as_slice())?)
    }

    /// Get list permission.
    pub async fn get_list_permission(&self) -> Result<api::PermissionList> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetListPermission", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::PermissionList::decode(response.as_slice())?)
    }

    /// List user permission in channel.
    pub async fn list_user_permission_in_channel(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<api::UserPermissionInChannelListResponse> {
        let cid = self.generate_cid();
        let body = api::UserPermissionInChannelListRequest {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListUserPermissionInChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UserPermissionInChannelListResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Get user status.
    pub async fn get_user_status(&self) -> Result<api::UserStatus> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetUserStatus", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UserStatus::decode(response.as_slice())?)
    }

    /// List online users.
    pub async fn list_user_online(
        &self,
        clan_id: i64,
        limit: i32,
        page: i32,
    ) -> Result<api::ListUserOnlineResponse> {
        let cid = self.generate_cid();
        let body = api::ListUserOnlineRequest {
            clan_id,
            limit,
            page,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListUserOnline", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListUserOnlineResponse::decode(response.as_slice())?)
    }

    /// List streaming channel users.
    pub async fn list_streaming_channel_users(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<api::StreamingChannelUserList> {
        let cid = self.generate_cid();
        let body = api::ListChannelUsersRequest {
            clan_id,
            channel_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListStreamingChannelUsers", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::StreamingChannelUserList::decode(response.as_slice())?)
    }

    /// List quick menu access.
    pub async fn list_quick_menu_access(
        &self,
        bot_id: i64,
        channel_id: i64,
        menu_type: i32,
    ) -> Result<api::QuickMenuAccessList> {
        let cid = self.generate_cid();
        let body = api::ListQuickMenuAccessRequest {
            bot_id,
            channel_id,
            menu_type,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListQuickMenuAccess", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::QuickMenuAccessList::decode(response.as_slice())?)
    }

    /// Get notification channel.
    pub async fn get_notification_channel(
        &self,
        channel_id: i64,
    ) -> Result<api::NotificationUserChannel> {
        let cid = self.generate_cid();
        let body = api::NotificationChannel { channel_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetNotificationChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::NotificationUserChannel::decode(response.as_slice())?)
    }

    /// Get notification category.
    pub async fn get_notification_category(
        &self,
        category_id: i64,
    ) -> Result<api::NotificationUserChannel> {
        let cid = self.generate_cid();
        let body = api::DefaultNotificationCategory { category_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetNotificationCategory", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::NotificationUserChannel::decode(response.as_slice())?)
    }

    /// List clan users status.
    pub async fn list_clan_users_status(&self, clan_id: i64) -> Result<api::ClanUserStatusList> {
        let cid = self.generate_cid();
        let body = api::ListClanUsersStatusRequest { clan_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListClanUsersStatus", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ClanUserStatusList::decode(response.as_slice())?)
    }

    /// Get list favorite channels.
    pub async fn get_list_favorite_channel(
        &self,
        clan_id: i64,
    ) -> Result<api::ListFavoriteChannelResponse> {
        let cid = self.generate_cid();
        let body = api::ListFavoriteChannelRequest { clan_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetListFavoriteChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListFavoriteChannelResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List logged devices.
    pub async fn list_loged_device(&self) -> Result<api::LogedDeviceList> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListLogedDevice", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let devices = api::LogedDeviceList::decode(response.as_slice())?;
        Ok(devices)
    }

    /// List channel users (UC variant).
    pub async fn list_channel_users_uc(
        &self,
        channel_id: i64,
        limit: i32,
    ) -> Result<api::AllUsersAddChannelResponse> {
        let cid = self.generate_cid();
        let body = api::AllUsersAddChannelRequest { channel_id, limit }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelUsersUC", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::AllUsersAddChannelResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List webhook by channel ID.
    pub async fn list_webhook_by_channel_id(
        &self,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<api::WebhookListResponse> {
        let cid = self.generate_cid();
        let body = api::WebhookListRequest {
            channel_id,
            clan_id,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListWebhookByChannelId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::WebhookListResponse::decode(response.as_slice())?)
    }

    /// Get permission by role ID and channel ID.
    pub async fn get_permission_by_role_id_channel_id(
        &self,
        role_id: i64,
        channel_id: i64,
    ) -> Result<api::PermissionRoleChannelListEventResponse> {
        let cid = self.generate_cid();
        let body = api::PermissionRoleChannelListEventRequest {
            role_id,
            channel_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetPermissionByRoleIdChannelId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::PermissionRoleChannelListEventResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List channel setting.
    pub async fn list_channel_setting(
        &self,
        clan_id: i64,
    ) -> Result<api::ChannelSettingListResponse> {
        let cid = self.generate_cid();
        let body = api::ChannelSettingListRequest {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelSetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelSettingListResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List apps.
    pub async fn list_apps(&self, filter: &str) -> Result<api::AppList> {
        let cid = self.generate_cid();
        let body = api::ListAppsRequest {
            filter: filter.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListApps", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::AppList::decode(response.as_slice())?)
    }

    /// Get app by ID.
    pub async fn get_app(&self, id: i64) -> Result<api::App> {
        let cid = self.generate_cid();
        let body = api::App {
            id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "GetApp", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::App::decode(response.as_slice())?)
    }

    /// List for sale items.
    pub async fn list_for_sale_items(&self, page: i32) -> Result<api::ForSaleItemList> {
        let cid = self.generate_cid();
        let body = api::ListForSaleItemsRequest { page }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListForSaleItems", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ForSaleItemList::decode(response.as_slice())?)
    }

    /// List clan webhook.
    pub async fn list_clan_webhook(&self, clan_id: i64) -> Result<api::ListClanWebhookResponse> {
        let cid = self.generate_cid();
        let body = api::ListClanWebhookRequest { clan_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListClanWebhook", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListClanWebhookResponse::decode(response.as_slice())?)
    }

    /// List Sd Topics.
    pub async fn list_sd_topic(&self, clan_id: i64, limit: i32) -> Result<api::SdTopicList> {
        let cid = self.generate_cid();
        let body = api::ListSdTopicRequest { clan_id, limit }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListSdTopic", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SdTopicList::decode(response.as_slice())?)
    }

    /// Get topic detail.
    pub async fn get_topic_detail(&self, topic_id: i64) -> Result<api::SdTopic> {
        let cid = self.generate_cid();
        let body = api::SdTopicDetailRequest { topic_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "GetTopicDetail", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SdTopic::decode(response.as_slice())?)
    }

    /// List channel attachment.
    pub async fn list_channel_attachment(
        &self,
        channel_id: i64,
        clan_id: i64,
        limit: i32,
    ) -> Result<api::ChannelAttachmentList> {
        let cid = self.generate_cid();
        let body = api::ListChannelAttachmentRequest {
            channel_id,
            clan_id,
            limit,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelAttachment", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelAttachmentList::decode(response.as_slice())?)
    }

    /// List voice channel users.
    pub async fn list_channel_voice_users(
        &self,
        clan_id: i64,
    ) -> Result<api::VoiceChannelUserList> {
        let cid = self.generate_cid();
        let body = api::ListChannelUsersRequest {
            clan_id,
            limit: 100,
            state: 1,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelVoiceUsers", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::VoiceChannelUserList::decode(response.as_slice())?)
    }

    /// List archived channel descriptions.
    pub async fn list_archived_channel_descs(
        &self,
        clan_id: i64,
    ) -> Result<api::ListArchivedChannelDescsResponse> {
        let cid = self.generate_cid();
        let body = api::ListArchivedChannelDescsRequest { clan_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListArchivedChannelDescs", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListArchivedChannelDescsResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List user clans by user ID.
    pub async fn list_user_clans_by_user_id(&self) -> Result<api::AllUserClans> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListUserClansByUserId", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::AllUserClans::decode(response.as_slice())?)
    }

    /// Check if user is banned.
    pub async fn is_banned(&self, channel_id: i64) -> Result<api::IsBannedResponse> {
        let cid = self.generate_cid();
        let body = api::IsBannedRequest { channel_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "IsBanned", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::IsBannedResponse::decode(response.as_slice())?)
    }

    /// List banned users.
    pub async fn list_banned_users(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<api::BannedUserList> {
        let cid = self.generate_cid();
        let body = api::BannedUserListRequest {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListBannedUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::BannedUserList::decode(response.as_slice())?)
    }

    /// Get channel canvas list.
    pub async fn get_channel_canvas_list(
        &self,
        channel_id: i64,
        clan_id: i64,
        limit: i32,
        page: i32,
    ) -> Result<api::ChannelCanvasListResponse> {
        let cid = self.generate_cid();
        let body = api::ChannelCanvasListRequest {
            channel_id,
            clan_id,
            limit,
            page,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetChannelCanvasList", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelCanvasListResponse::decode(response.as_slice())?)
    }

    /// Get channel canvas detail.
    pub async fn get_channel_canvas_detail(
        &self,
        id: i64,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<api::ChannelCanvasDetailResponse> {
        let cid = self.generate_cid();
        let body = api::ChannelCanvasDetailRequest {
            id,
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetChannelCanvasDetail", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelCanvasDetailResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List onboarding.
    pub async fn list_onboarding(
        &self,
        clan_id: i64,
        limit: i32,
        page: i32,
    ) -> Result<api::ListOnboardingResponse> {
        let cid = self.generate_cid();
        let body = api::ListOnboardingRequest {
            clan_id,
            limit,
            page,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListOnboarding", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListOnboardingResponse::decode(response.as_slice())?)
    }

    /// Get onboarding detail.
    pub async fn get_onboarding_detail(
        &self,
        id: i64,
        clan_id: i64,
    ) -> Result<api::OnboardingItem> {
        let cid = self.generate_cid();
        let body = api::OnboardingRequest { id, clan_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetOnboardingDetail", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::OnboardingItem::decode(response.as_slice())?)
    }

    /// List onboarding steps.
    pub async fn list_onboarding_step(
        &self,
        clan_id: i64,
    ) -> Result<api::ListOnboardingStepResponse> {
        let cid = self.generate_cid();
        let body = api::ListOnboardingStepRequest {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListOnboardingStep", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListOnboardingStepResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List role users.
    pub async fn list_role_users(&self, role_id: i64) -> Result<api::RoleUserList> {
        let cid = self.generate_cid();
        let body = api::ListRoleUsersRequest {
            role_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListRoleUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::RoleUserList::decode(response.as_slice())?)
    }

    /// List role permissions.
    pub async fn list_role_permissions(&self, role_id: i64) -> Result<api::PermissionList> {
        let cid = self.generate_cid();
        let body = api::ListPermissionsRequest { role_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListRolePermissions", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::PermissionList::decode(response.as_slice())?)
    }

    /// Check if user is a follower.
    pub async fn is_follower(&self, follow_id: i64) -> Result<api::IsFollowerResponse> {
        let cid = self.generate_cid();
        let body = api::IsFollowerRequest { follow_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "IsFollower", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::IsFollowerResponse::decode(response.as_slice())?)
    }

    /// Get channel encryption method.
    pub async fn get_chan_encryption_method(
        &self,
        channel_id: i64,
    ) -> Result<api::ChanEncryptionMethod> {
        let cid = self.generate_cid();
        let body = api::ChanEncryptionMethod {
            channel_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetChanEncryptionMethod", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChanEncryptionMethod::decode(response.as_slice())?)
    }

    /// Get key server.
    pub async fn get_key_server(&self) -> Result<api::GetKeyServerResp> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetKeyServer", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GetKeyServerResp::decode(response.as_slice())?)
    }

    /// Get pub keys.
    pub async fn get_pub_keys(&self, user_ids: &[&str]) -> Result<api::GetPubKeysResponse> {
        let cid = self.generate_cid();
        let body = api::GetPubKeysRequest {
            user_ids: user_ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "GetPubKeys", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GetPubKeysResponse::decode(response.as_slice())?)
    }

    /// List audit log.
    pub async fn list_audit_log(&self, clan_id: i64) -> Result<api::ListAuditLog> {
        let cid = self.generate_cid();
        let body = api::ListAuditLogRequest {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListAuditLog", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListAuditLog::decode(response.as_slice())?)
    }

    /// Search message.
    pub async fn search_message(
        &self,
        _query: &str,
        from: i32,
        size: i32,
    ) -> Result<api::SearchMessageResponse> {
        let cid = self.generate_cid();
        let body = api::SearchMessageRequest {
            from,
            size,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "SearchMessage", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SearchMessageResponse::decode(response.as_slice())?)
    }

    /// Search thread.
    pub async fn search_thread(&self, clan_id: i64, label: &str) -> Result<api::ChannelDescList> {
        let cid = self.generate_cid();
        let body = api::SearchThreadRequest {
            clan_id,
            label: label.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "SearchThread", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelDescList::decode(response.as_slice())?)
    }

    /// List Mezon OAuth client.
    pub async fn list_mezon_oauth_client(&self) -> Result<api::MezonOauthClientList> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListMezonOauthClient", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::MezonOauthClientList::decode(response.as_slice())?)
    }

    /// Get Mezon OAuth client.
    pub async fn get_mezon_oauth_client(&self, client_id: &str) -> Result<api::MezonOauthClient> {
        let cid = self.generate_cid();
        let body = api::GetMezonOauthClientRequest {
            client_id: client_id.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetMezonOauthClient", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::MezonOauthClient::decode(response.as_slice())?)
    }

    /// Generate hash channel apps.
    pub async fn generate_hash_channel_apps(
        &self,
        app_id: i64,
    ) -> Result<api::GenerateHashChannelAppsResponse> {
        let cid = self.generate_cid();
        let body = api::GenerateHashChannelAppsRequest { app_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GenerateHashChannelApps", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GenerateHashChannelAppsResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Get notification category settings list.
    pub async fn get_channel_category_noti_settings_list(
        &self,
        clan_id: i64,
    ) -> Result<api::NotificationChannelCategorySettingList> {
        let cid = self.generate_cid();
        let body = api::NotificationClan { clan_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetChannelCategoryNotiSettingsList", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::NotificationChannelCategorySettingList::decode(
            response.as_slice(),
        )?)
    }

    /// List muted channels.
    pub async fn list_muted_channels(&self, clan_id: i64) -> Result<Vec<String>> {
        let cid = self.generate_cid();

        let body = api::ListMutedChannelRequest { clan_id }.encode_to_vec();

        let (code, response) = self.send_api_request(cid, "ListMutedChannel", body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let muted = api::MutedChannelList::decode(response.as_slice())?;
        Ok(muted
            .muted_list
            .into_iter()
            .map(|channel_id| channel_id.to_string())
            .collect())
    }
}

// ============================================================================
// API Methods - Cold Path (infrequent operations)
// ============================================================================

impl MezonTransport {
    /// Create a new channel.
    pub async fn create_channel(
        &self,
        _clan_id: i64,
        channel_label: &str,
        channel_type: u32,
        category_id: Option<i64>,
        parent_id: Option<i64>,
    ) -> Result<ApiChannelDesc> {
        let cid = self.generate_cid();

        let category_id = category_id.unwrap_or(0);
        let parent_id = parent_id.unwrap_or(0);
        let body = api::CreateChannelDescRequest {
            clan_id: _clan_id,
            channel_label: channel_label.to_string(),
            r#type: channel_type as i32,
            category_id,
            parent_id,
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self
            .send_api_request(cid, "CreateChannelDesc", body)
            .await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let channel = api::ChannelDescription::decode(response.as_slice())?;
        Ok(Self::channel_desc_from_proto(channel))
    }

    /// Delete a channel.
    pub async fn delete_channel(&self, _channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();

        let body = api::DeleteChannelDescRequest {
            channel_id: _channel_id,
            ..Default::default()
        }
        .encode_to_vec();

        let (code, _response) = self
            .send_api_request(cid, "DeleteChannelDesc", body)
            .await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        Ok(())
    }

    /// Add a friend.
    pub async fn add_friend(&self, _user_id: i64) -> Result<()> {
        let cid = self.generate_cid();

        let body = api::AddFriendsRequest {
            ids: vec![_user_id],
            ..Default::default()
        }
        .encode_to_vec();

        let (code, _response) = self.send_api_request(cid, "AddFriends", body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        Ok(())
    }

    /// Delete a friend.
    pub async fn delete_friend(&self, _user_id: i64) -> Result<()> {
        let cid = self.generate_cid();

        let body = api::DeleteFriendsRequest {
            ids: vec![_user_id],
            ..Default::default()
        }
        .encode_to_vec();

        let (code, _response) = self.send_api_request(cid, "DeleteFriends", body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        Ok(())
    }

    /// Update channel description.
    pub async fn update_channel_desc(
        &self,
        clan_id: i64,
        channel_id: i64,
        label: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateChannelDescRequest {
            clan_id,
            channel_id,
            channel_label: Some(label.to_string()),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateChannelDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update channel private.
    pub async fn update_channel_private(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_private: i32,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ChangeChannelPrivateRequest {
            clan_id,
            channel_id,
            channel_private,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateChannelPrivate", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Change channel category.
    pub async fn change_channel_category(
        &self,
        clan_id: i64,
        channel_id: i64,
        new_category_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ChangeChannelCategoryRequest {
            clan_id,
            channel_id,
            new_category_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "ChangeChannelCategory", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add channel users.
    pub async fn add_channel_users(&self, channel_id: i64, user_ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::AddChannelUsersRequest {
            channel_id,
            user_ids: user_ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "AddChannelUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Remove channel users.
    pub async fn remove_channel_users(&self, channel_id: i64, user_ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::RemoveChannelUsersRequest {
            channel_id,
            user_ids: user_ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "RemoveChannelUsers", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Leave thread.
    pub async fn leave_thread(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::LeaveThreadRequest {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "LeaveThread", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Archive channel.
    pub async fn archive_channel(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ArchiveChannelRequest {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "ArchiveChannel", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Reactivate archived thread.
    pub async fn active_archived_thread(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = realtime::ActiveArchivedThread {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "ActiveArchivedThread", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Close DM.
    pub async fn close_dm_by_channel_id(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteChannelDescRequest {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "CloseDMByChannelId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Open DM.
    pub async fn open_dm_by_channel_id(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteChannelDescRequest {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "OpenDMByChannelId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create clan.
    pub async fn create_clan_desc(
        &self,
        clan_name: &str,
        logo: &str,
        banner: &str,
    ) -> Result<ApiClanDesc> {
        let cid = self.generate_cid();
        let body = api::CreateClanDescRequest {
            clan_name: clan_name.to_string(),
            logo: logo.to_string(),
            banner: banner.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateClanDesc", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let clan = api::ClanDesc::decode(response.as_slice())?;
        Ok(Self::clan_desc_from_proto(clan))
    }

    /// Update clan.
    pub async fn update_clan_desc(&self, clan_id: i64, clan_name: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateClanDescRequest {
            clan_id,
            clan_name: clan_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateClanDesc", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete clan.
    pub async fn delete_clan_desc(&self, clan_desc_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteClanDescRequest { clan_desc_id }.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteClanDesc", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Remove clan users.
    pub async fn remove_clan_users(&self, clan_id: i64, user_ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::RemoveClanUsersRequest {
            clan_id,
            user_ids: user_ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "RemoveClanUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Ban clan users.
    pub async fn ban_clan_users(
        &self,
        clan_id: i64,
        channel_id: i64,
        user_ids: &[&str],
        ban_time: i32,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::BanClanUsersRequest {
            clan_id,
            channel_id,
            user_ids: user_ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
            ban_time,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "BanClanUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Unban clan users.
    pub async fn unban_clan_users(
        &self,
        clan_id: i64,
        channel_id: i64,
        user_ids: &[&str],
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::BanClanUsersRequest {
            clan_id,
            channel_id,
            user_ids: user_ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UnbanClanUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create category.
    pub async fn create_category_desc(
        &self,
        category_name: &str,
        clan_id: i64,
    ) -> Result<api::CategoryDesc> {
        let cid = self.generate_cid();
        let body = api::CreateCategoryDescRequest {
            category_name: category_name.to_string(),
            clan_id,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateCategoryDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CategoryDesc::decode(response.as_slice())?)
    }

    /// Delete category.
    pub async fn delete_category_desc(&self, category_id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteCategoryDescRequest {
            category_id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteCategoryDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update category.
    pub async fn update_category(
        &self,
        category_id: i64,
        category_name: &str,
        clan_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateCategoryDescRequest {
            category_id,
            category_name: category_name.to_string(),
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateCategory", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Block friends.
    pub async fn block_friends(&self, ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::BlockFriendsRequest {
            ids: ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "BlockFriends", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Unblock friends.
    pub async fn unblock_friends(&self, ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::BlockFriendsRequest {
            ids: ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UnblockFriends", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update username.
    pub async fn update_username(&self, username: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateUsernameRequest {
            username: username.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateUsername", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update user profile.
    pub async fn update_user(&self, display_name: &str, avatar_url: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateUsersRequest {
            display_name: display_name.to_string(),
            avatar_url: avatar_url.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "UpdateUser", body).await?;
        if code != 0 {
            let msg = String::from_utf8_lossy(&response);
            tracing::error!("UpdateUser error: code={}, response={}", code, msg);
            return Err(anyhow::anyhow!(
                "API error: code={}, response={}",
                code,
                msg
            ));
        }
        Ok(())
    }

    /// Update user profile by clan.
    pub async fn update_user_profile_by_clan(
        &self,
        clan_id: i64,
        nick_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateClanProfileRequest {
            clan_id,
            nick_name: Some(nick_name.to_string()),
            avatar: avatar_url.map(|s| s.to_string()),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "UpdateUserProfileByClan", body)
            .await?;
        if code != 0 {
            let msg = String::from_utf8_lossy(&response);
            tracing::error!(
                "UpdateUserProfileByClan error: code={}, response={}",
                code,
                msg
            );
            return Err(anyhow::anyhow!(
                "API error: code={}, response={}",
                code,
                msg
            ));
        }
        Ok(())
    }

    /// Mark as read.
    pub async fn mark_as_read(
        &self,
        channel_id: i64,
        category_id: i64,
        clan_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::MarkAsReadRequest {
            channel_id,
            category_id,
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "MarkAsRead", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update user status.
    pub async fn update_user_status(&self, status: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UserStatusUpdate {
            status: status.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateUserStatus", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update user custom status.
    pub async fn update_user_custom_status(&self, status: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UserStatusUpdate {
            status: status.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateUserCustomStatus", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Session logout.
    pub async fn session_logout(&self, token: &str, refresh_token: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SessionLogoutRequest {
            token: token.to_string(),
            refresh_token: refresh_token.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "SessionLogout", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Log out / remove a specific device by device_id.
    /// Uses the current session credentials + target device_id.
    pub async fn logout_device(
        &self,
        token: &str,
        refresh_token: &str,
        device_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SessionLogoutRequest {
            token: token.to_string(),
            refresh_token: refresh_token.to_string(),
            device_id: device_id.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "SessionLogout", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set notification channel setting.
    pub async fn set_notification_channel_setting(
        &self,
        channel_category_id: i64,
        notification_type: i32,
        clan_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetNotificationRequest {
            channel_category_id,
            notification_type,
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetNotificationChannelSetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set notification clan setting.
    pub async fn set_notification_clan_setting(
        &self,
        clan_id: i64,
        notification_type: i32,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetDefaultNotificationRequest {
            clan_id,
            notification_type,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetNotificationClanSetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set notification category setting.
    pub async fn set_notification_category_setting(
        &self,
        channel_category_id: i64,
        notification_type: i32,
        clan_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetNotificationRequest {
            channel_category_id,
            notification_type,
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetNotificationCategorySetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set mute channel.
    pub async fn set_mute_channel(&self, id: i64, mute_time: i32, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetMuteRequest {
            id,
            mute_time,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "SetMuteChannel", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set mute category.
    pub async fn set_mute_category(&self, id: i64, mute_time: i32, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetMuteRequest {
            id,
            mute_time,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "SetMuteCategory", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete notifications.
    pub async fn delete_notifications(&self, ids: &[&str], category: i32) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteNotificationsRequest {
            ids: ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
            category,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteNotifications", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete notification category setting.
    pub async fn delete_notification_category_setting(&self, category_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DefaultNotificationCategory { category_id }.encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteNotificationCategorySetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete notification channel.
    pub async fn delete_notification_channel(&self, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::NotificationChannel { channel_id }.encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteNotificationChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set role channel permission.
    pub async fn set_role_channel_permission(&self, role_id: i64, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateRoleChannelRequest {
            role_id,
            channel_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetRoleChannelPermission", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Check duplicate name.
    pub async fn check_duplicate_name(
        &self,
        name: &str,
        r#type: i32,
        condition_id: i64,
    ) -> Result<api::CheckDuplicateNameResponse> {
        let cid = self.generate_cid();
        let body = api::CheckDuplicateNameRequest {
            name: name.to_string(),
            r#type,
            condition_id,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CheckDuplicateName", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CheckDuplicateNameResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Upload attachment file.
    pub async fn upload_attachment_file(
        &self,
        filename: &str,
        filetype: &str,
        size: i32,
        width: i32,
        height: i32,
    ) -> Result<api::UploadAttachment> {
        let cid = self.generate_cid();
        let body = api::UploadAttachmentRequest {
            filename: filename.to_string(),
            filetype: filetype.to_string(),
            size,
            width,
            height,
            part_count: 0,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "UploadAttachmentFile", body)
            .await?;
        if code != 0 {
            let msg = String::from_utf8_lossy(&response);
            tracing::error!(
                "UploadAttachmentFile error: code={}, response={}",
                code,
                msg
            );

            if let Ok(envelope) = realtime::Envelope::decode(response.as_slice())
                && let Some(realtime::envelope::Message::Error(error)) = envelope.message
            {
                return Err(anyhow::anyhow!(
                    "UploadAttachmentFile API error: code={} error={}",
                    error.code,
                    error.message
                ));
            }

            return Err(anyhow::anyhow!(
                "API error: code={}, response={}",
                code,
                msg
            ));
        }
        Ok(api::UploadAttachment::decode(response.as_slice())?)
    }

    /// Upload OAuth file.
    pub async fn upload_oauth_file(
        &self,
        filename: &str,
        filetype: &str,
        size: i32,
    ) -> Result<api::UploadAttachment> {
        let cid = self.generate_cid();
        let body = api::UploadAttachmentRequest {
            filename: filename.to_string(),
            filetype: filetype.to_string(),
            size,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "UploadOauthFile", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UploadAttachment::decode(response.as_slice())?)
    }

    /// Push pub key.
    pub async fn push_pub_key(&self, encr: &[u8], sign: &[u8]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::PushPubKeyRequest {
            pk: Some(api::PubKey {
                encr: encr.to_vec(),
                sign: sign.to_vec(),
            }),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "PushPubKey", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set channel encryption method.
    pub async fn set_chan_encryption_method(&self, channel_id: i64, method: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ChanEncryptionMethod {
            channel_id,
            method: method.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetChanEncryptionMethod", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Transfer ownership.
    pub async fn transfer_ownership(&self, clan_id: i64, new_owner_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::TransferOwnershipRequest {
            clan_id,
            new_owner_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "TransferOwnership", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Report message abuse.
    pub async fn report_message_abuse(&self, message_id: i64, abuse_type: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ReportMessageAbuseReqest {
            message_id,
            abuse_type: abuse_type.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "ReportMessageAbuse", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Message button click.
    pub async fn message_button_click(
        &self,
        message_id: i64,
        channel_id: i64,
        button_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = realtime::MessageButtonClicked {
            message_id,
            channel_id,
            button_id: button_id.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "MessageButtonClick", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Dropdown box selected.
    pub async fn dropdown_box_selected(
        &self,
        message_id: i64,
        channel_id: i64,
        selectbox_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = realtime::DropdownBoxSelected {
            message_id,
            channel_id,
            selectbox_id: selectbox_id.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DropdownBoxSelected", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add agent to channel.
    pub async fn add_agent_to_channel(&self, channel_id: i64, room_name: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateAiAgentRequest {
            channel_id,
            room_name: room_name.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "AddAgentToChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Disconnect agent.
    pub async fn disconnect_agent(&self, channel_id: i64, room_name: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateAiAgentRequest {
            channel_id,
            room_name: room_name.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DisconnectAgent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Regist FCM device token.
    pub async fn regist_fcm_device_token(
        &self,
        token: &str,
        device_id: &str,
        platform: &str,
    ) -> Result<api::RegistFcmDeviceTokenResponse> {
        let cid = self.generate_cid();
        let body = api::RegistFcmDeviceTokenRequest {
            token: token.to_string(),
            device_id: device_id.to_string(),
            platform: platform.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "RegistFCMDeviceToken", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::RegistFcmDeviceTokenResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Create link invite user.
    pub async fn create_link_invite_user(
        &self,
        clan_id: i64,
        channel_id: i64,
        expiry_time: i32,
    ) -> Result<api::LinkInviteUser> {
        let cid = self.generate_cid();
        let body = api::LinkInviteUserRequest {
            clan_id,
            channel_id,
            expiry_time,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateLinkInviteUser", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::LinkInviteUser::decode(response.as_slice())?)
    }

    /// Invite user.
    pub async fn invite_user(&self, invite_id: i64) -> Result<api::InviteUserRes> {
        let cid = self.generate_cid();
        let body = api::InviteUserRequest { invite_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "InviteUser", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::InviteUserRes::decode(response.as_slice())?)
    }

    /// Create activity.
    pub async fn create_activiy(
        &self,
        activity_name: &str,
        activity_type: i32,
    ) -> Result<api::UserActivity> {
        let cid = self.generate_cid();
        let body = api::CreateActivityRequest {
            activity_name: activity_name.to_string(),
            activity_type,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateActiviy", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UserActivity::decode(response.as_slice())?)
    }

    /// Create message to inbox.
    pub async fn create_message_2_inbox(
        &self,
        message_id: i64,
        channel_id: i64,
        clan_id: i64,
        content: &str,
    ) -> Result<api::ChannelMessageHeader> {
        let cid = self.generate_cid();
        let body = api::Message2InboxRequest {
            message_id,
            channel_id,
            clan_id,
            content: content.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateMessage2Inbox", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelMessageHeader::decode(response.as_slice())?)
    }

    /// Create pin message.
    pub async fn create_pin_message(
        &self,
        message_id: i64,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<api::ChannelMessageHeader> {
        let cid = self.generate_cid();
        let body = api::PinMessageRequest {
            message_id,
            channel_id,
            clan_id,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreatePinMessage", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelMessageHeader::decode(response.as_slice())?)
    }

    /// Delete pin message.
    pub async fn delete_pin_message(
        &self,
        id: i64,
        message_id: i64,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeletePinMessage {
            id,
            message_id,
            channel_id,
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeletePinMessage", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update clan order.
    pub async fn update_clan_order(&self, clans_order: &[(i32, i64)]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateClanOrderRequest {
            clans_order: clans_order
                .iter()
                .map(|(order, clan_id)| {
                    Ok(api::update_clan_order_request::ClanOrder {
                        order: *order,
                        clan_id: *clan_id,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateClanOrder", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update category order.
    pub async fn update_category_order(
        &self,
        clan_id: i64,
        categories: &[(i32, i64)],
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateCategoryOrderRequest {
            clan_id,
            categories: categories
                .iter()
                .map(|(order, category_id)| {
                    Ok(api::CategoryOrderUpdate {
                        order: *order,
                        category_id: *category_id,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateCategoryOrder", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update role order.
    pub async fn update_role_order(&self, clan_id: i64, roles: &[(i32, i64)]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateRoleOrderRequest {
            clan_id,
            roles: roles
                .iter()
                .map(|(order, role_id)| {
                    Ok(api::RoleOrderUpdate {
                        order: *order,
                        role_id: *role_id,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateRoleOrder", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create event.
    pub async fn create_event(
        &self,
        title: &str,
        clan_id: i64,
        channel_id: i64,
        start_time: u32,
        end_time: u32,
    ) -> Result<api::EventManagement> {
        let cid = self.generate_cid();
        let body = api::CreateEventRequest {
            title: title.to_string(),
            clan_id,
            channel_id,
            start_time_seconds: start_time,
            end_time_seconds: end_time,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EventManagement::decode(response.as_slice())?)
    }

    /// Delete event.
    pub async fn delete_event(&self, event_id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteEventRequest {
            event_id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update event.
    pub async fn update_event(&self, event_id: i64, clan_id: i64, title: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateEventRequest {
            event_id,
            clan_id,
            title: title.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add user event.
    pub async fn add_user_event(&self, clan_id: i64, event_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UserEventRequest { clan_id, event_id }.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "AddUserEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete user event.
    pub async fn delete_user_event(&self, clan_id: i64, event_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UserEventRequest { clan_id, event_id }.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteUserEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create role.
    pub async fn create_role(&self, title: &str, clan_id: i64) -> Result<api::Role> {
        let cid = self.generate_cid();
        let body = api::CreateRoleRequest {
            title: title.to_string(),
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateRole", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::Role::decode(response.as_slice())?)
    }

    /// Delete role.
    pub async fn delete_role(&self, role_id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteRoleRequest {
            role_id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteRole", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update role.
    pub async fn update_role(&self, role_id: i64, title: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateRoleRequest {
            role_id,
            title: Some(title.to_string()),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateRole", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete role channel desc.
    pub async fn delete_role_channel_desc(&self, role_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteRoleRequest {
            role_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteRoleChannelDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add roles channel desc.
    pub async fn add_roles_channel_desc(&self, role_ids: &[&str], channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::AddRoleChannelDescRequest {
            role_ids: role_ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "AddRolesChannelDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create clan emoji.
    pub async fn create_clan_emoji(
        &self,
        clan_id: i64,
        source: &str,
        shortname: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanEmojiCreateRequest {
            clan_id,
            source: source.to_string(),
            shortname: shortname.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "CreateClanEmoji", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete clan emoji by ID.
    pub async fn delete_clan_emoji_by_id(&self, id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanEmojiDeleteRequest {
            id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteByIdClanEmoji", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update clan emoji by ID.
    pub async fn update_clan_emoji_by_id(
        &self,
        id: i64,
        shortname: &str,
        clan_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanEmojiUpdateRequest {
            id,
            shortname: shortname.to_string(),
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateClanEmojiById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add clan sticker.
    pub async fn add_clan_sticker(
        &self,
        clan_id: i64,
        source: &str,
        shortname: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanStickerAddRequest {
            clan_id,
            source: source.to_string(),
            shortname: shortname.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "AddClanSticker", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update clan sticker by ID.
    pub async fn update_clan_sticker_by_id(&self, id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanStickerUpdateByIdRequest {
            id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateClanStickerById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete clan sticker by ID.
    pub async fn delete_clan_sticker_by_id(&self, id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanStickerDeleteRequest {
            id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteClanStickerById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Generate webhook.
    pub async fn generate_webhook(
        &self,
        webhook_name: &str,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::WebhookCreateRequest {
            webhook_name: webhook_name.to_string(),
            channel_id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "GenerateWebhook", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update webhook by ID.
    pub async fn update_webhook_by_id(&self, id: i64, webhook_name: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::WebhookUpdateRequestById {
            id,
            webhook_name: webhook_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateWebhookById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete webhook by ID.
    pub async fn delete_webhook_by_id(&self, id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::WebhookDeleteRequestById {
            id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteWebhookById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Generate clan webhook.
    pub async fn generate_clan_webhook(
        &self,
        clan_id: i64,
        webhook_name: &str,
    ) -> Result<api::GenerateClanWebhookResponse> {
        let cid = self.generate_cid();
        let body = api::GenerateClanWebhookRequest {
            clan_id,
            webhook_name: webhook_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GenerateClanWebhook", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GenerateClanWebhookResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Update clan webhook by ID.
    pub async fn update_clan_webhook_by_id(
        &self,
        id: i64,
        clan_id: i64,
        webhook_name: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateClanWebhookRequest {
            id,
            clan_id,
            webhook_name: webhook_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateClanWebhookById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete clan webhook by ID.
    pub async fn delete_clan_webhook_by_id(&self, id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanWebhookRequest { id, clan_id }.encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteClanWebhookById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add app.
    pub async fn add_app(&self, appname: &str) -> Result<api::App> {
        let cid = self.generate_cid();
        let body = api::AddAppRequest {
            appname: appname.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "AddApp", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::App::decode(response.as_slice())?)
    }

    /// Delete app.
    pub async fn delete_app(&self, id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::App {
            id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteApp", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update app.
    pub async fn update_app(&self, id: i64, appname: &str) -> Result<api::App> {
        let cid = self.generate_cid();
        let body = api::UpdateAppRequest {
            id,
            appname: Some(appname.to_string()),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "UpdateApp", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::App::decode(response.as_slice())?)
    }

    /// Add app to clan.
    pub async fn add_app_to_clan(&self, app_id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::AppClan { app_id, clan_id }.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "AddAppToClan", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create system message.
    pub async fn create_system_message(&self, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SystemMessageRequest {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "CreateSystemMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update system message.
    pub async fn update_system_message(&self, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SystemMessageRequest {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateSystemMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete system message.
    pub async fn delete_system_message(&self, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteSystemMessage { clan_id }.encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteSystemMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Edit channel canvases.
    pub async fn edit_channel_canvases(
        &self,
        channel_id: i64,
        clan_id: i64,
        title: &str,
        content: &str,
    ) -> Result<api::EditChannelCanvasResponse> {
        let cid = self.generate_cid();
        let body = api::EditChannelCanvasRequest {
            channel_id,
            clan_id,
            title: title.to_string(),
            content: content.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "EditChannelCanvases", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EditChannelCanvasResponse::decode(response.as_slice())?)
    }

    /// Delete channel canvas.
    pub async fn delete_channel_canvas(
        &self,
        canvas_id: i64,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteChannelCanvasRequest {
            canvas_id,
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteChannelCanvas", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add channel favorite.
    pub async fn add_channel_favorite(&self, channel_id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::AddFavoriteChannelRequest {
            channel_id,
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "AddChannelFavorite", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Remove channel favorite.
    pub async fn remove_channel_favorite(&self, channel_id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::RemoveFavoriteChannelRequest {
            channel_id,
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "RemoveChannelFavorite", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create onboarding.
    pub async fn create_onboarding(&self, clan_id: i64) -> Result<api::ListOnboardingResponse> {
        let cid = self.generate_cid();
        let body = api::CreateOnboardingRequest {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateOnboarding", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListOnboardingResponse::decode(response.as_slice())?)
    }

    /// Update onboarding.
    pub async fn update_onboarding(&self, id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateOnboardingRequest {
            id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateOnboarding", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete onboarding.
    pub async fn delete_onboarding(&self, id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::OnboardingRequest { id, clan_id }.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteOnboarding", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update onboarding step.
    pub async fn update_onboarding_step(&self, clan_id: i64, onboarding_step: i32) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateOnboardingStepRequest {
            clan_id,
            onboarding_step,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateOnboardingStep", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create Sd topic.
    pub async fn create_sd_topic(
        &self,
        message_id: i64,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<api::SdTopic> {
        let cid = self.generate_cid();
        let body = api::SdTopicRequest {
            message_id,
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateSdTopic", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SdTopic::decode(response.as_slice())?)
    }

    /// Create external Mezon meet.
    pub async fn create_external_mezon_meet(&self) -> Result<api::GenerateMezonMeetResponse> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "CreateExternalMezonMeet", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GenerateMezonMeetResponse::decode(response.as_slice())?)
    }

    /// Generate meet token.
    pub async fn generate_meet_token(
        &self,
        channel_id: i64,
        room_name: &str,
    ) -> Result<api::GenerateMeetTokenResponse> {
        let cid = self.generate_cid();
        let body = api::GenerateMeetTokenRequest {
            channel_id,
            room_name: room_name.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GenerateMeetToken", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GenerateMeetTokenResponse::decode(response.as_slice())?)
    }

    /// Remove participant Mezon meet.
    pub async fn remove_participant_mezon_meet(
        &self,
        channel_id: i64,
        room_name: &str,
        username: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::MeetParticipantRequest {
            channel_id,
            room_name: room_name.to_string(),
            username: username.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "RemoveParticipantMezonMeet", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Mute participant Mezon meet.
    pub async fn mute_participant_mezon_meet(
        &self,
        channel_id: i64,
        room_name: &str,
        username: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::MeetParticipantRequest {
            channel_id,
            room_name: room_name.to_string(),
            username: username.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "MuteParticipantMezonMeet", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create room channel apps.
    pub async fn create_room_channel_apps(
        &self,
        channel_id: i64,
        room_name: &str,
    ) -> Result<api::CreateRoomChannelApps> {
        let cid = self.generate_cid();
        let body = api::CreateRoomChannelApps {
            channel_id,
            room_name: room_name.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateRoomChannelApps", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CreateRoomChannelApps::decode(response.as_slice())?)
    }

    /// Update Mezon OAuth client.
    pub async fn update_mezon_oauth_client(
        &self,
        client_id: &str,
        client_name: &str,
    ) -> Result<api::MezonOauthClient> {
        let cid = self.generate_cid();
        let body = api::MezonOauthClient {
            client_id: client_id.to_string(),
            client_name: client_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "UpdateMezonOauthClient", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::MezonOauthClient::decode(response.as_slice())?)
    }

    /// Add quick menu access.
    pub async fn add_quick_menu_access(
        &self,
        bot_id: i64,
        clan_id: i64,
        menu_name: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::QuickMenuAccess {
            bot_id,
            clan_id,
            menu_name: menu_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "AddQuickMenuAccess", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update quick menu access.
    pub async fn update_quick_menu_access(
        &self,
        bot_id: i64,
        clan_id: i64,
        menu_name: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::QuickMenuAccess {
            bot_id,
            clan_id,
            menu_name: menu_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateQuickMenuAccess", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete quick menu access.
    pub async fn delete_quick_menu_access(&self, id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::QuickMenuAccess {
            id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteQuickMenuAccess", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update channel message.
    pub async fn update_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        content: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = realtime::ChannelMessageUpdate {
            clan_id,
            channel_id,
            message_id,
            content: content.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateChannelMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete channel message.
    pub async fn delete_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = realtime::ChannelMessageRemove {
            clan_id,
            channel_id,
            message_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteChannelMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// React channel message.
    pub async fn react_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        emoji_id: i64,
        emoji: &str,
        count: i32,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::MessageReaction {
            clan_id,
            channel_id,
            message_id,
            emoji_id,
            emoji: emoji.to_string(),
            count,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "ReactChannelMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create poll.
    pub async fn create_poll(
        &self,
        channel_id: i64,
        clan_id: i64,
        question: &str,
    ) -> Result<api::CreatePollResponse> {
        let cid = self.generate_cid();
        let body = api::CreatePollRequest {
            channel_id,
            clan_id,
            question: question.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreatePoll", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CreatePollResponse::decode(response.as_slice())?)
    }

    /// Vote poll.
    pub async fn vote_poll(
        &self,
        poll_id: i64,
        message_id: i64,
        channel_id: i64,
    ) -> Result<api::VotePollResponse> {
        let cid = self.generate_cid();
        let body = api::VotePollRequest {
            poll_id,
            message_id,
            channel_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "VotePoll", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::VotePollResponse::decode(response.as_slice())?)
    }

    /// Close poll.
    pub async fn close_poll(&self, poll_id: i64, message_id: i64, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClosePollRequest {
            poll_id,
            message_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "ClosePoll", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Get poll.
    pub async fn get_poll(
        &self,
        poll_id: i64,
        message_id: i64,
        channel_id: i64,
    ) -> Result<api::GetPollResponse> {
        let cid = self.generate_cid();
        let body = api::GetPollRequest {
            poll_id,
            message_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "GetPoll", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GetPollResponse::decode(response.as_slice())?)
    }

    /// Create channel timeline.
    pub async fn create_channel_timeline(
        &self,
        clan_id: i64,
        channel_id: i64,
        title: &str,
    ) -> Result<api::CreateChannelTimelineResponse> {
        let cid = self.generate_cid();
        let body = api::CreateChannelTimelineRequest {
            clan_id,
            channel_id,
            title: title.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateChannelTimeline", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CreateChannelTimelineResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Update channel timeline.
    pub async fn update_channel_timeline(
        &self,
        clan_id: i64,
        channel_id: i64,
        id: i64,
        title: &str,
    ) -> Result<api::UpdateChannelTimelineResponse> {
        let cid = self.generate_cid();
        let body = api::UpdateChannelTimelineRequest {
            clan_id,
            channel_id,
            id,
            title: title.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "UpdateChannelTimeline", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UpdateChannelTimelineResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Detail channel timeline.
    pub async fn detail_channel_timeline(
        &self,
        clan_id: i64,
        channel_id: i64,
        id: i64,
    ) -> Result<api::ChannelTimelineDetailResponse> {
        let cid = self.generate_cid();
        let body = api::ChannelTimelineDetailRequest {
            clan_id,
            channel_id,
            id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "DetailChannelTimeline", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelTimelineDetailResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Update user account.
    pub async fn update_account(
        &self,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
        about_me: Option<&str>,
    ) -> Result<()> {
        let cid = self.generate_cid();

        let body = api::UpdateAccountRequest {
            display_name: display_name
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            avatar_url: avatar_url.filter(|s| !s.is_empty()).map(|s| s.to_string()),
            about_me: about_me.filter(|s| !s.is_empty()).map(|s| s.to_string()),
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self.send_api_request(cid, "UpdateAccount", body).await?;

        tracing::debug!(
            "UpdateAccount response: code={}, response={:?}",
            code,
            response
        );

        if code == 0 {
            Ok(())
        } else {
            let msg = String::from_utf8_lossy(&response);
            tracing::error!("UpdateAccount error: code={}, response={}", code, msg);
            Err(anyhow::anyhow!(
                "API error: code={}, response={}",
                code,
                msg
            ))
        }
    }

    /// Delete user account.
    pub async fn delete_account(&self) -> Result<()> {
        let cid = self.generate_cid();

        let (code, _response) = self
            .send_api_request(cid, "DeleteAccount", Vec::new())
            .await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        Ok(())
    }

    /// Authenticate against the server with a refresh token.
    pub async fn session_refresh(&self, token: &str, is_remember: bool) -> Result<ApiSession> {
        let cid = self.generate_cid();
        let body = api::SessionRefreshRequest {
            token: token.to_string(),
            is_remember,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "SessionRefresh", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let session = api::Session::decode(response.as_slice())?;
        Ok(ApiSession {
            token: session.token,
            refresh_token: session.refresh_token,
            user_id: session.user_id,
        })
    }

    /// Register email.
    pub async fn registration_email(
        &self,
        req: api::RegistrationEmailRequest,
    ) -> Result<ApiSession> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "RegistrationEmail", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let session = api::Session::decode(response.as_slice())?;
        Ok(ApiSession {
            token: session.token,
            refresh_token: session.refresh_token,
            user_id: session.user_id,
        })
    }

    /// Link email.
    pub async fn link_email(&self, req: api::AccountEmail) -> Result<ApiSession> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "LinkEmail", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let session = api::Session::decode(response.as_slice())?;
        Ok(ApiSession {
            token: session.token,
            refresh_token: session.refresh_token,
            user_id: session.user_id,
        })
    }

    /// Unlink email.
    pub async fn unlink_email(&self, req: api::AccountEmail) -> Result<()> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UnlinkEmail", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Link SMS.
    pub async fn link_sms(&self, req: api::AccountMezon) -> Result<ApiSession> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "LinkSMS", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let session = api::Session::decode(response.as_slice())?;
        Ok(ApiSession {
            token: session.token,
            refresh_token: session.refresh_token,
            user_id: session.user_id,
        })
    }

    /// Unlink Mezon (SMS).
    pub async fn unlink_mezon(&self, req: api::AccountMezon) -> Result<()> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UnlinkMezon", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Confirm link Mezon OTP.
    pub async fn confirm_link_mezon_otp(
        &self,
        req: api::LinkAccountConfirmRequest,
    ) -> Result<ApiSession> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ConfirmLinkMezonOTP", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let session = api::Session::decode(response.as_slice())?;
        Ok(ApiSession {
            token: session.token,
            refresh_token: session.refresh_token,
            user_id: session.user_id,
        })
    }

    /// Multipart upload attachment file start.
    pub async fn multipart_upload_attachment_file_start(
        &self,
        req: api::UploadAttachmentRequest,
    ) -> Result<api::MultipartUploadAttachment> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "MultipartUploadAttachmentFileStart", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::MultipartUploadAttachment::decode(response.as_slice())?)
    }

    /// Multipart upload attachment file finish.
    pub async fn multipart_upload_attachment_file_finish(
        &self,
        req: api::MultipartUploadAttachmentFinishRequest,
    ) -> Result<api::UploadAttachment> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "MultipartUploadAttachmentFileFinish", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UploadAttachment::decode(response.as_slice())?)
    }

    /// Upload batch attachment file.
    pub async fn upload_batch_attachment_file(
        &self,
        req: api::UploadBatchAttachmentRequest,
    ) -> Result<api::UploadAttachmentBatch> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "UploadBatchAttachmentFile", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UploadAttachmentBatch::decode(response.as_slice())?)
    }

    /// Delete SD topic.
    pub async fn delete_sd_topic(&self, req: api::DeleteSdTopicRequest) -> Result<()> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteSdTopic", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockAdapter {
        send_ok: bool,
    }

    #[async_trait::async_trait]
    impl TransportAdapter for MockAdapter {
        async fn connect(&self, _host: &str, _port: u16, _token: &str) -> Result<()> {
            Ok(())
        }
        async fn send(&self, _message: Vec<u8>) -> Result<()> {
            if self.send_ok {
                Ok(())
            } else {
                Err(anyhow::anyhow!("mock send failed"))
            }
        }
        async fn send_ping(&self, _cid: u16) -> Result<()> {
            Ok(())
        }
        fn is_open(&self) -> bool {
            true
        }
        async fn close(&self) -> Result<()> {
            Ok(())
        }
        async fn set_on_message(&self, _handler: crate::transport_adapter::MessageHandler) {}
        async fn set_on_open(&self, _handler: crate::transport_adapter::OpenHandler) {}
        async fn set_on_close(&self, _handler: crate::transport_adapter::CloseHandler) {}
        async fn set_on_error(&self, _handler: crate::transport_adapter::ErrorHandler) {}
    }

    fn transport(send_ok: bool) -> MezonTransport {
        MezonTransport::new(Box::new(MockAdapter { send_ok }), String::new())
    }

    #[tokio::test]
    async fn gate_passes_immediately_when_connected() {
        let t = transport(true);
        t.connected_tx.send(true).unwrap();
        t.wait_connected(Duration::from_secs(5)).await.unwrap();
    }

    #[tokio::test]
    async fn gate_times_out_when_never_connected() {
        let t = transport(true);
        let err = t
            .wait_connected(Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not connected"));
    }

    #[tokio::test]
    async fn gate_resolves_when_connection_arrives_mid_wait() {
        let t = transport(true);
        let tx = t.connected_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let _ = tx.send(true);
        });
        t.wait_connected(Duration::from_secs(5)).await.unwrap();
    }

    #[tokio::test]
    async fn send_gate_times_out_and_inserts_no_pending_when_disconnected() {
        let mut t = transport(true);
        t.connect_gate = Duration::from_millis(50);
        let cid = t.generate_cid();
        let err = t.send(cid, vec![1, 2, 3, 4]).await.unwrap_err();
        assert!(err.to_string().contains("not connected"));
        assert!(t.pending_requests.read().await.is_empty());
    }

    #[tokio::test]
    async fn send_removes_pending_when_adapter_send_fails() {
        let mut t = transport(false);
        t.connect_gate = Duration::from_millis(50);
        t.connected_tx.send(true).unwrap();
        let cid = t.generate_cid();
        let err = t.send(cid, vec![1, 2, 3, 4]).await.unwrap_err();
        assert!(err.to_string().contains("mock send failed"));
        assert!(t.pending_requests.read().await.is_empty());
    }

    #[test]
    fn api_index_pins_known_names_and_rejects_unknown() {
        let t = transport(true);
        assert_eq!(t.get_api_index("ListChannelDescs"), Some(0));
        assert_eq!(t.get_api_index("GetAccount"), Some(1));
        assert_eq!(t.get_api_index("ListClanDescs"), Some(2));
        assert_eq!(t.get_api_index("ListChannelMessages"), Some(30));
        assert_eq!(t.get_api_index("UploadBatchAttachmentFile"), Some(209));
        assert_eq!(t.get_api_index("DefinitelyNotAnApi"), None);
    }
}
