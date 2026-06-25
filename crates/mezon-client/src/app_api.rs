use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::{
    TransportClient,
    transport::{
        ApiAccount, ApiCategoryDesc, ApiChannelApp, ApiChannelDesc, ApiClanDesc, ApiDirectChannel,
        ApiMessage, ApiVoiceChannelUser, RealtimeEvent,
    },
};

const CHECK_NAME_TYPE_NICKNAME: i32 = 4;

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn clamp_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn image_dimensions(data: &[u8]) -> (i32, i32) {
    image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok())
        .map(|(w, h)| {
            (
                i32::try_from(w).unwrap_or(i32::MAX),
                i32::try_from(h).unwrap_or(i32::MAX),
            )
        })
        .unwrap_or((0, 0))
}

/// Connection lifecycle of the realtime transport — the analog of Zed's `client::Status`.
/// Exposed as a `watch` stream via [`AppApi::status`] so stores/UI react instead of polling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Clone)]
pub struct AppApi {
    transport: Arc<TransportClient>,
    realtime_tx: Arc<tokio::sync::broadcast::Sender<RealtimeEvent>>,
    status_tx: Arc<tokio::sync::watch::Sender<ConnectionStatus>>,
}

impl AppApi {
    pub fn new(transport: Arc<TransportClient>) -> Self {
        let (realtime_tx, _) = tokio::sync::broadcast::channel(1024);
        let (status_tx, _) = tokio::sync::watch::channel(ConnectionStatus::Disconnected);
        Self {
            transport,
            realtime_tx: Arc::new(realtime_tx),
            status_tx: Arc::new(status_tx),
        }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<RealtimeEvent> {
        self.realtime_tx.subscribe()
    }

    pub fn publish_event(&self, event: RealtimeEvent) {
        let _ = self.realtime_tx.send(event);
    }

    /// Watch the realtime connection status (cf. Zed `Client::status`). Reactive — no polling.
    pub fn status(&self) -> tokio::sync::watch::Receiver<ConnectionStatus> {
        self.status_tx.subscribe()
    }

    /// Current connection status snapshot.
    pub fn connection_status(&self) -> ConnectionStatus {
        *self.status_tx.borrow()
    }

    /// Update the connection status — called by the transport connection manager.
    pub fn set_status(&self, status: ConnectionStatus) {
        let _ = self.status_tx.send(status);
    }

    pub async fn get_account(&self) -> Result<ApiAccount> {
        self.transport.get_account().await
    }

    pub async fn list_channel_descs(
        &self,
        clan_id: i64,
        channel_type: i32,
    ) -> Result<Vec<ApiChannelDesc>> {
        let _ = channel_type;
        self.transport.list_channel_descs(clan_id).await
    }

    pub async fn list_channel_by_user_id(&self) -> Result<Vec<ApiChannelDesc>> {
        self.transport.list_channel_by_user_id().await
    }

    pub async fn list_dm_channels(&self, page: i32) -> Result<Vec<ApiDirectChannel>> {
        let _ = page;
        self.transport.list_dm_channel_descs().await
    }

    pub async fn mark_as_read(
        &self,
        channel_id: i64,
        category_id: i64,
        clan_id: i64,
    ) -> Result<()> {
        self.transport
            .mark_as_read(channel_id, category_id, clan_id)
            .await
    }

    pub async fn list_clan_descs(&self) -> Result<Vec<ApiClanDesc>> {
        self.transport.list_clan_descs().await
    }

    pub async fn list_clan_users(
        &self,
        clan_id: i64,
    ) -> Result<Vec<mezon_proto::api::clan_user_list::ClanUser>> {
        let lists = self.transport.list_clan_users(clan_id).await?;
        Ok(lists.into_iter().flat_map(|list| list.clan_users).collect())
    }

    pub async fn list_channel_users(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_type: i32,
    ) -> Result<Vec<mezon_proto::api::channel_user_list::ChannelUser>> {
        let list = self
            .transport
            .list_channel_users(clan_id, channel_id, channel_type)
            .await?;
        Ok(list.channel_users)
    }

    pub async fn list_user_clans_by_user(&self) -> Result<Vec<mezon_proto::api::User>> {
        let list = self.transport.list_user_clans_by_user_id().await?;
        Ok(list.users)
    }

    pub async fn list_channel_users_uc(
        &self,
        channel_id: i64,
        limit: i32,
    ) -> Result<mezon_proto::api::AllUsersAddChannelResponse> {
        self.transport
            .list_channel_users_uc(channel_id, limit)
            .await
    }

    pub async fn create_clan_desc(
        &self,
        clan_name: &str,
        logo: &str,
        banner: &str,
    ) -> Result<ApiClanDesc> {
        self.transport
            .create_clan_desc(clan_name, logo, banner)
            .await
    }

    pub async fn is_open(&self) -> bool {
        self.transport.is_open().await
    }

    pub async fn ping_roundtrip(&self) -> Result<()> {
        self.transport.ping_roundtrip().await
    }

    pub async fn list_channel_messages(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        direction: i32,
        limit: u32,
    ) -> Result<Vec<ApiMessage>> {
        self.transport
            .list_channel_messages(clan_id, channel_id, message_id, direction, limit)
            .await
    }

    pub async fn join_chat(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_type: i32,
        is_public: bool,
    ) -> Result<()> {
        self.transport
            .join_chat(clan_id, channel_id, channel_type, is_public)
            .await
    }

    pub async fn send_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
    ) -> Result<ApiMessage> {
        self.transport
            .send_channel_message(clan_id, channel_id, content, is_public, mode)
            .await
    }

    pub async fn create_channel(
        &self,
        clan_id: i64,
        channel_label: &str,
        channel_type: u32,
        category_id: Option<i64>,
        parent_id: Option<i64>,
    ) -> Result<ApiChannelDesc> {
        self.transport
            .create_channel(clan_id, channel_label, channel_type, category_id, parent_id)
            .await
    }

    /// Create a category in a clan; returns its id.
    pub async fn create_category(&self, clan_id: i64, category_name: &str) -> Result<String> {
        let category = self
            .transport
            .create_category_desc(category_name, clan_id)
            .await?;
        Ok(category.category_id.to_string())
    }

    pub async fn add_channel_users(&self, channel_id: i64, user_ids: Vec<String>) -> Result<()> {
        self.transport.add_channel_users(channel_id, user_ids).await
    }

    /// Send a channel message, uploading each `media_url` as an attachment first.
    pub async fn send_message_with_media(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        media_urls: &[String],
    ) -> Result<ApiMessage> {
        let attachments: Vec<_> = {
            use futures::StreamExt as _;
            futures::stream::iter(media_urls.iter().map(|url| self.upload_media_from_url(url)))
                .buffer_unordered(4)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>>>()?
        };
        self.transport
            .send_channel_message_with_attachments(
                clan_id,
                channel_id,
                content,
                is_public,
                mode,
                attachments,
            )
            .await
    }

    async fn upload_bytes(
        &self,
        filename: &str,
        filetype: &str,
        size: i32,
        width: i32,
        height: i32,
        data: Vec<u8>,
    ) -> Result<String> {
        let upload = self
            .transport
            .upload_attachment_file(filename, filetype, size, width, height)
            .await?;
        crate::transport_runtime::put_bytes_to_url(&upload.url, data).await?;
        Ok(upload
            .url
            .split('?')
            .next()
            .unwrap_or(&upload.url)
            .to_string())
    }

    async fn upload_media_from_url(
        &self,
        url: &str,
    ) -> Result<mezon_proto::api::MessageAttachment> {
        let (data, content_type) = crate::transport_runtime::fetch_bytes(url).await?;
        let filetype = content_type.unwrap_or_else(|| "application/octet-stream".to_string());
        let ext = match filetype.split('/').nth(1) {
            Some(e) if !e.is_empty() => e,
            _ => "bin",
        };
        let stem = url
            .split('?')
            .next()
            .unwrap_or(url)
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("media");
        let filename = if stem.contains('.') {
            sanitize_filename(stem)
        } else {
            sanitize_filename(&format!("{stem}.{ext}"))
        };
        let size = clamp_i32(data.len());

        let (width, height) = if filetype.starts_with("image/") {
            image_dimensions(&data)
        } else {
            (0, 0)
        };

        let url = self
            .upload_bytes(&filename, &filetype, size, width, height, data)
            .await?;

        Ok(mezon_proto::api::MessageAttachment {
            filename,
            size,
            url,
            filetype,
            width,
            height,
            thumbnail: String::new(),
            duration: 0,
        })
    }

    pub async fn update_user(&self, display_name: &str, avatar_url: &str) -> Result<()> {
        self.transport.update_user(display_name, avatar_url).await
    }

    pub async fn update_account(
        &self,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
        about_me: Option<&str>,
    ) -> Result<()> {
        self.transport
            .update_account(display_name, avatar_url, about_me)
            .await
    }

    pub async fn upload_attachment_file(
        &self,
        filename: &str,
        filetype: &str,
        size: i32,
        width: i32,
        height: i32,
    ) -> Result<mezon_proto::api::UploadAttachment> {
        self.transport
            .upload_attachment_file(filename, filetype, size, width, height)
            .await
    }

    pub async fn get_user_clan_profile(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::ClanProfile> {
        self.transport.get_user_profile_on_clan(clan_id).await
    }

    pub async fn update_user_clan_profile(
        &self,
        clan_id: i64,
        nick_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<()> {
        self.transport
            .update_user_profile_by_clan(clan_id, nick_name, avatar_url)
            .await
    }

    pub async fn check_duplicate_clan_name(&self, name: &str, condition_id: &str) -> Result<bool> {
        let cond: i64 = condition_id
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid condition_id {condition_id:?}: {e}"))?;
        let resp = self.transport.check_duplicate_name(name, 0, cond).await?;
        Ok(resp.is_duplicate)
    }

    pub async fn check_duplicate_clan_nickname(
        &self,
        clan_id: i64,
        nick_name: &str,
    ) -> Result<bool> {
        let resp = self
            .transport
            .check_duplicate_name(nick_name, CHECK_NAME_TYPE_NICKNAME, clan_id)
            .await?;
        Ok(resp.is_duplicate)
    }

    pub async fn upload_avatar(&self, path: &Path) -> Result<String> {
        let data = crate::transport_runtime::read_file(path.to_path_buf()).await?;

        let raw_filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("avatar")
            .to_string();
        let filename = sanitize_filename(&raw_filename);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png")
            .to_string();
        let filetype = format!("image/{}", ext);
        let size = clamp_i32(data.len());
        let (width, height) = image_dimensions(&data);

        tracing::info!(
            "upload_avatar: file read ok filename={} filetype={} size={} width={} height={}",
            filename,
            filetype,
            size,
            width,
            height
        );

        let permanent_url = self
            .upload_bytes(&filename, &filetype, size, width, height, data)
            .await?;

        tracing::info!("Avatar upload complete: url={}", permanent_url);

        Ok(permanent_url)
    }

    pub async fn list_categories_typed(&self, clan_id: i64) -> Result<Vec<ApiCategoryDesc>> {
        self.transport.list_categories_typed(clan_id).await
    }

    pub async fn list_clan_badge_count(&self) -> Result<Vec<(String, i32, bool)>> {
        self.transport.list_clan_badge_count().await
    }

    pub async fn get_notification_clan(&self, clan_id: i64) -> Result<i32> {
        self.transport.get_notification_clan(clan_id).await
    }

    pub async fn list_notifications(
        &self,
        clan_id: &str,
        limit: i32,
        notification_id: &str,
        category: i32,
        direction: i32,
    ) -> Result<Vec<crate::InboxNotification>> {
        self.transport
            .list_notifications(clan_id, limit, notification_id, category, direction)
            .await
    }

    pub async fn delete_notifications(&self, ids: &[&str], category: i32) -> Result<()> {
        self.transport.delete_notifications(ids, category).await
    }

    pub async fn list_sd_topics(
        &self,
        clan_id: &str,
        limit: i32,
    ) -> Result<Vec<crate::TopicDiscussion>> {
        self.transport.list_sd_topics(clan_id, limit).await
    }

    pub async fn get_topic_detail(&self, topic_id: &str) -> Result<crate::TopicDiscussion> {
        self.transport.get_topic_detail(topic_id).await
    }

    pub async fn list_channel_badge_counts(&self, clan_id: i64) -> Result<Vec<ApiChannelDesc>> {
        self.transport.list_channel_badge_counts(clan_id).await
    }

    pub async fn list_voice_channel_users(&self, clan_id: i64) -> Result<Vec<ApiVoiceChannelUser>> {
        self.transport.list_voice_channel_users(clan_id).await
    }

    pub async fn generate_meet_token(&self, channel_id: &str, room_name: &str) -> Result<String> {
        self.transport
            .generate_meet_token(channel_id, room_name)
            .await
    }

    pub async fn list_channel_apps(&self, clan_id: i64) -> Result<Vec<ApiChannelApp>> {
        self.transport.list_channel_apps(clan_id).await
    }

    pub async fn list_favorite_channels(&self, clan_id: i64) -> Result<Vec<String>> {
        let resp = self.transport.get_list_favorite_channel(clan_id).await?;
        Ok(resp
            .channel_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect())
    }

    pub async fn add_channel_favorite(&self, channel_id: i64, clan_id: i64) -> Result<()> {
        self.transport
            .add_channel_favorite(channel_id, clan_id)
            .await
    }

    pub async fn remove_channel_favorite(&self, channel_id: i64, clan_id: i64) -> Result<()> {
        self.transport
            .remove_channel_favorite(channel_id, clan_id)
            .await
    }

    pub async fn list_loged_device(&self) -> Result<Vec<mezon_proto::api::LogedDevice>> {
        self.transport.list_loged_device().await
    }

    pub async fn session_logout(&self, token: &str, refresh_token: &str) -> Result<()> {
        self.transport.session_logout(token, refresh_token).await
    }

    pub async fn logout_device(
        &self,
        token: &str,
        refresh_token: &str,
        device_id: &str,
    ) -> Result<()> {
        self.transport
            .logout_device(token, refresh_token, device_id)
            .await
    }
}
