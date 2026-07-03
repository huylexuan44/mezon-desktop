use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::{
    TransportClient,
    transport::{
        ApiAccount, ApiCategoryDesc, ApiChannelApp, ApiChannelAttachment, ApiChannelDesc,
        ApiClanDesc, ApiDirectChannel, ApiMessage, ApiPinMessage, ApiThreadDesc,
        ApiVoiceChannelUser, RealtimeEvent,
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

#[derive(Debug, Clone)]
pub struct UploadFile {
    pub filename: String,
    pub filetype: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct UrlAttachment {
    pub url: String,
    pub filename: String,
    pub filetype: String,
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
        self.transport.list_dm_channel_descs(page).await
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

    pub async fn update_clan_desc(
        &self,
        request: mezon_proto::api::UpdateClanDescRequest,
    ) -> Result<()> {
        self.transport.update_clan_desc(request).await
    }

    pub async fn get_system_message_by_clan_id(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::SystemMessage> {
        self.transport.get_system_message_by_clan_id(clan_id).await
    }

    pub async fn update_system_message(
        &self,
        request: mezon_proto::api::SystemMessageRequest,
    ) -> Result<()> {
        self.transport.update_system_message(request).await
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
    ) -> Result<crate::transport::ListChannelMessagesResult> {
        self.transport
            .list_channel_messages(clan_id, channel_id, message_id, direction, limit)
            .await
    }

    pub async fn list_thread_descs(
        &self,
        channel_id: &str,
        clan_id: &str,
        page: i32,
    ) -> Result<Vec<ApiThreadDesc>> {
        self.transport
            .list_thread_descs(channel_id, clan_id, page)
            .await
    }

    pub async fn search_thread(
        &self,
        clan_id: &str,
        channel_id: &str,
        label: &str,
    ) -> Result<Vec<ApiThreadDesc>> {
        self.transport
            .search_thread(clan_id, channel_id, label)
            .await
    }

    pub async fn check_duplicate_thread_name(
        &self,
        name: &str,
        parent_channel_id: &str,
    ) -> Result<bool> {
        self.transport
            .check_duplicate_thread_name(name, parent_channel_id)
            .await
    }

    pub async fn get_pin_messages_list(
        &self,
        channel_id: &str,
        clan_id: &str,
    ) -> Result<Vec<ApiPinMessage>> {
        self.transport
            .get_pin_messages_list(channel_id, clan_id)
            .await
    }

    pub async fn delete_pin_message(
        &self,
        id: &str,
        message_id: &str,
        channel_id: &str,
        clan_id: &str,
    ) -> Result<()> {
        self.transport
            .delete_pin_message(id, message_id, channel_id, clan_id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_channel_attachments(
        &self,
        clan_id: i64,
        channel_id: i64,
        file_type: &str,
        state: i32,
        limit: i32,
        before: u32,
        after: u32,
    ) -> Result<Vec<ApiChannelAttachment>> {
        self.transport
            .list_channel_attachment(
                clan_id,
                channel_id,
                file_type.to_string(),
                state,
                limit,
                before,
                after,
            )
            .await
    }

    pub async fn create_pin_message(
        &self,
        message_id: i64,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<()> {
        self.transport
            .create_pin_message(message_id, channel_id, clan_id)
            .await?;
        Ok(())
    }

    pub async fn update_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        content: &str,
    ) -> Result<()> {
        self.transport
            .update_channel_message(clan_id, channel_id, message_id, content)
            .await
    }

    pub async fn vote_poll(
        &self,
        poll_id: i64,
        message_id: i64,
        channel_id: i64,
        answer_indices: Vec<i32>,
    ) -> Result<mezon_proto::api::VotePollResponse> {
        self.transport
            .vote_poll(poll_id, message_id, channel_id, answer_indices)
            .await
    }

    pub async fn get_poll(
        &self,
        poll_id: i64,
        message_id: i64,
        channel_id: i64,
    ) -> Result<mezon_proto::api::GetPollResponse> {
        self.transport
            .get_poll(poll_id, message_id, channel_id)
            .await
    }

    pub async fn close_poll(&self, poll_id: i64, message_id: i64, channel_id: i64) -> Result<()> {
        self.transport
            .close_poll(poll_id, message_id, channel_id)
            .await
    }

    pub async fn delete_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
    ) -> Result<()> {
        self.transport
            .delete_channel_message(clan_id, channel_id, message_id)
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

    pub async fn join_clan_chat(&self, clan_id: i64) -> Result<()> {
        self.transport.join_clan_chat(clan_id).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn react_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        emoji_id: i64,
        emoji: &str,
        count: i32,
        message_sender_id: i64,
        mode: i32,
        is_public: bool,
        remove: bool,
    ) -> Result<()> {
        self.transport
            .react_channel_message(
                clan_id,
                channel_id,
                message_id,
                emoji_id,
                emoji,
                count,
                message_sender_id,
                mode,
                is_public,
                remove,
            )
            .await
    }

    pub async fn write_last_seen_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        mode: i32,
        timestamp_seconds: u32,
        badge_count: i32,
    ) -> Result<()> {
        self.transport
            .write_last_seen_message(
                clan_id,
                channel_id,
                message_id,
                mode,
                timestamp_seconds,
                badge_count,
            )
            .await
    }

    pub async fn list_clan_users_status(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::ClanUserStatusList> {
        self.transport.list_clan_users_status(clan_id).await
    }

    pub async fn list_user_online(
        &self,
        clan_id: i64,
        limit: i32,
        page: i32,
    ) -> Result<Vec<mezon_proto::api::User>> {
        let resp = self
            .transport
            .list_user_online(clan_id, limit, page)
            .await?;
        Ok(resp.users)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        mentions: Vec<crate::transport::OutgoingMention>,
        hashtags: Vec<crate::transport::OutgoingHashtag>,
        emojis: Vec<crate::transport::OutgoingEmoji>,
    ) -> Result<ApiMessage> {
        self.transport
            .send_channel_message(
                clan_id, channel_id, content, is_public, mode, mentions, hashtags, emojis,
            )
            .await
    }

    /// Send a message as a reply to another message.
    #[allow(clippy::too_many_arguments)]
    pub async fn send_channel_message_reply(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        reply: crate::transport::OutgoingReply,
        mentions: Vec<crate::transport::OutgoingMention>,
        hashtags: Vec<crate::transport::OutgoingHashtag>,
        emojis: Vec<crate::transport::OutgoingEmoji>,
    ) -> Result<ApiMessage> {
        self.transport
            .send_channel_message_reply(
                clan_id, channel_id, content, is_public, mode, reply, mentions, hashtags, emojis,
            )
            .await
    }

    pub async fn list_emojis_by_user_id(&self) -> Result<Vec<mezon_proto::api::ClanEmoji>> {
        let resp = self.transport.list_emojis_by_user_id().await?;
        Ok(resp.emoji_list)
    }

    pub async fn emoji_recent_list(&self) -> Result<Vec<mezon_proto::api::EmojiRecent>> {
        let resp = self.transport.emoji_recent_list().await?;
        Ok(resp.emoji_recents)
    }

    pub async fn list_roles(
        &self,
        clan_id: i64,
        limit: i32,
        cursor: &str,
    ) -> Result<mezon_proto::api::RoleListEventResponse> {
        self.transport.list_roles(clan_id, limit, cursor).await
    }

    pub async fn get_list_permission(&self) -> Result<mezon_proto::api::PermissionList> {
        self.transport.get_list_permission().await
    }

    pub async fn get_clan_user_role(&self, clan_id: i64) -> Result<mezon_proto::api::RoleList> {
        self.transport.get_clan_user_role(clan_id, 0).await
    }

    pub async fn list_stickers_by_user_id(&self) -> Result<Vec<mezon_proto::api::ClanSticker>> {
        let resp = self.transport.list_stickers_by_user_id().await?;
        Ok(resp.stickers)
    }

    pub async fn create_channel(
        &self,
        clan_id: i64,
        channel_label: &str,
        channel_type: u32,
        category_id: Option<i64>,
        parent_id: Option<i64>,
        channel_private: i32,
    ) -> Result<ApiChannelDesc> {
        self.transport
            .create_channel(
                clan_id,
                channel_label,
                channel_type,
                category_id,
                parent_id,
                channel_private,
            )
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

    pub async fn send_message_with_attachments(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        files: Vec<UploadFile>,
    ) -> Result<ApiMessage> {
        let attachments: Vec<_> = {
            use futures::StreamExt as _;
            futures::stream::iter(files.into_iter().map(|file| self.upload_file(file)))
                .buffered(4)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>>>()?
        };
        let echo: Vec<crate::transport::ApiAttachment> = attachments
            .iter()
            .map(|a| crate::transport::ApiAttachment {
                url: a.url.clone(),
                filename: a.filename.clone(),
                filetype: a.filetype.clone(),
                width: a.width,
                height: a.height,
                thumbnail: a.thumbnail.clone(),
                duration: a.duration,
            })
            .collect();
        let mut sent = self
            .transport
            .send_channel_message_with_attachments(
                clan_id,
                channel_id,
                content,
                is_public,
                mode,
                attachments,
            )
            .await?;
        sent.attachments = echo;
        Ok(sent)
    }

    pub async fn send_message_with_attachment_urls(
        &self,
        clan_id: i64,
        channel_id: i64,
        is_public: bool,
        mode: i32,
        attachments: Vec<UrlAttachment>,
    ) -> Result<ApiMessage> {
        let proto: Vec<mezon_proto::api::MessageAttachment> = attachments
            .iter()
            .map(|a| mezon_proto::api::MessageAttachment {
                filename: a.filename.clone(),
                size: 0,
                url: a.url.clone(),
                filetype: a.filetype.clone(),
                width: 0,
                height: 0,
                thumbnail: String::new(),
                duration: 0,
            })
            .collect();
        let echo: Vec<crate::transport::ApiAttachment> = attachments
            .into_iter()
            .map(|a| crate::transport::ApiAttachment {
                url: a.url,
                filename: a.filename,
                filetype: a.filetype,
                width: 0,
                height: 0,
                thumbnail: String::new(),
                duration: 0,
            })
            .collect();
        let mut sent = self
            .transport
            .send_channel_message_with_attachments(clan_id, channel_id, "", is_public, mode, proto)
            .await?;
        sent.attachments = echo;
        Ok(sent)
    }

    async fn upload_file(&self, file: UploadFile) -> Result<mezon_proto::api::MessageAttachment> {
        let UploadFile {
            filename,
            filetype,
            data,
        } = file;
        let filename = sanitize_filename(&filename);
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

    pub async fn list_user_permission_in_channel(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<mezon_proto::api::UserPermissionInChannelListResponse> {
        self.transport
            .list_user_permission_in_channel(clan_id, channel_id)
            .await
    }
}
