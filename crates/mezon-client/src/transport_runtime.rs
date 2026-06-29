//! Transport runtime wrapper with dedicated tokio runtime.
//!
//! Similar to how `ReqwestClient` manages its own tokio runtime via `static OnceLock<Runtime>`,
//! this allows transport operations to work when called from GPUI's smol-based executor.

use crate::abridged_tcp_adapter::AbridgedTcpAdapter;
use crate::transport::MezonTransport;
use anyhow::Result;
use futures::AsyncReadExt as _;
use http_client::{AsyncBody, HttpClient, http};
use reqwest_client::ReqwestClient;
use std::sync::OnceLock;
use tokio::runtime::Runtime;

static TRANSPORT_RUNTIME: OnceLock<Runtime> = OnceLock::new();
static HTTP_CLIENT: OnceLock<ReqwestClient> = OnceLock::new();

pub(crate) fn http_client() -> &'static ReqwestClient {
    HTTP_CLIENT.get_or_init(new_http_client)
}

pub fn new_http_client() -> ReqwestClient {
    let _guard = runtime().enter();
    ReqwestClient::new()
}

fn runtime() -> &'static Runtime {
    TRANSPORT_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("mezon-transport")
            .build()
            .expect("Failed to build transport runtime")
    })
}

pub fn handle() -> tokio::runtime::Handle {
    runtime().handle().clone()
}

pub async fn put_bytes_to_url(url: &str, data: Vec<u8>) -> Result<()> {
    tracing::debug!("put_bytes_to_url: PUTting {} bytes", data.len());
    let client = http_client();
    let request = http::Request::builder()
        .method(http::Method::PUT)
        .uri(url)
        .header("Content-Type", "application/octet-stream")
        .body(AsyncBody::from(data))?;
    let response = client.send(request).await?;
    let status = response.status();
    tracing::debug!("put_bytes_to_url: response status={}", status);
    if !status.is_success() {
        tracing::error!("put_bytes_to_url: HTTP PUT failed with status {}", status);
        anyhow::bail!("HTTP PUT failed with status {}", status);
    }
    Ok(())
}

const MAX_FETCH_BYTES: u64 = 64 * 1024 * 1024;

pub async fn fetch_bytes(url: &str) -> Result<(Vec<u8>, Option<String>)> {
    let client = http_client();
    let request = http::Request::builder()
        .method(http::Method::GET)
        .uri(url)
        .body(AsyncBody::empty())?;
    let mut response = client.send(request).await?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("HTTP GET failed with status {}", status);
    }
    let content_type = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let mut bytes: Vec<u8> = Vec::new();
    let mut limited = response.body_mut().take(MAX_FETCH_BYTES + 1);
    limited.read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > MAX_FETCH_BYTES {
        anyhow::bail!("response exceeds {MAX_FETCH_BYTES}-byte cap");
    }
    Ok((bytes, content_type))
}

pub async fn read_file(path: std::path::PathBuf) -> Result<Vec<u8>> {
    runtime()
        .spawn_blocking(move || std::fs::read(&path))
        .await
        .map_err(|e| anyhow::anyhow!("file read task failed: {e}"))?
        .map_err(Into::into)
}

#[derive(Clone)]
pub struct TransportClient {
    inner: std::sync::Arc<MezonTransport>,
}

impl TransportClient {
    pub fn new(base_path: String) -> Self {
        let adapter = Box::new(AbridgedTcpAdapter::new());
        let transport = MezonTransport::new(adapter, base_path);
        Self {
            inner: std::sync::Arc::new(transport),
        }
    }

    pub async fn connect(
        &self,
        host: &str,
        port: Option<u16>,
        token: &str,
        on_event: impl Fn(crate::transport::RealtimeEvent) + Send + Sync + 'static,
        on_disconnected: impl Fn(bool) + Send + Sync + 'static,
    ) -> Result<()> {
        tracing::debug!("TransportClient::connect() starting");
        tracing::debug!("  Spawning connection task on dedicated transport runtime...");

        let transport = self.inner.clone();
        let host = host.to_string();
        let token = token.to_string();

        runtime()
            .spawn(async move {
                tracing::debug!("Inside transport runtime, calling MezonTransport::connect()...");
                let result = transport
                    .connect(&host, port, &token, on_event, on_disconnected)
                    .await;

                match &result {
                    Ok(_) => tracing::debug!("MezonTransport::connect() succeeded in runtime"),
                    Err(e) => {
                        tracing::error!("MezonTransport::connect() failed in runtime: {}", e)
                    }
                }

                result
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))??;

        tracing::debug!("TransportClient::connect() completed");
        Ok(())
    }

    pub async fn get_account(&self) -> Result<crate::transport::ApiAccount> {
        tracing::debug!("TransportClient::get_account() called");

        let transport = self.inner.clone();

        tracing::debug!("  Spawning on transport runtime...");
        let result = runtime()
            .spawn(async move {
                tracing::debug!("  Inside transport runtime task");
                transport.get_account().await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?;

        tracing::debug!("  Transport runtime task completed");
        result
    }

    pub async fn list_channel_descs(
        &self,
        clan_id: i64,
    ) -> Result<Vec<crate::transport::ApiChannelDesc>> {
        tracing::debug!("TransportClient::list_channel_descs() called");

        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_channel_descs(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_channel_by_user_id(&self) -> Result<Vec<crate::transport::ApiChannelDesc>> {
        tracing::debug!("TransportClient::list_channel_by_user_id() called");

        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_channel_by_user_id().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_dm_channel_descs(
        &self,
        page: i32,
    ) -> Result<Vec<crate::transport::ApiDirectChannel>> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_dm_channel_descs(page).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn mark_as_read(
        &self,
        channel_id: i64,
        category_id: i64,
        clan_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .mark_as_read(channel_id, category_id, clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_categories_typed(
        &self,
        clan_id: i64,
    ) -> Result<Vec<crate::transport::ApiCategoryDesc>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_categories_typed(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_channel_badge_counts(
        &self,
        clan_id: i64,
    ) -> Result<Vec<crate::transport::ApiChannelDesc>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_channel_badge_counts(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_voice_channel_users(
        &self,
        clan_id: i64,
    ) -> Result<Vec<crate::transport::ApiVoiceChannelUser>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_voice_channel_users(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_clan_users(
        &self,
        clan_id: i64,
    ) -> Result<Vec<mezon_proto::api::ClanUserList>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_clan_users(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_emojis_by_user_id(&self) -> Result<mezon_proto::api::EmojiListedResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_emojis_by_user_id().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_stickers_by_user_id(
        &self,
    ) -> Result<mezon_proto::api::StickerListedResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_stickers_by_user_id().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_channel_users(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_type: i32,
    ) -> Result<mezon_proto::api::ChannelUserList> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .list_channel_users(clan_id, channel_id, channel_type)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_user_clans_by_user_id(&self) -> Result<mezon_proto::api::AllUserClans> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_user_clans_by_user_id().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_channel_users_uc(
        &self,
        channel_id: i64,
        limit: i32,
    ) -> Result<mezon_proto::api::AllUsersAddChannelResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_channel_users_uc(channel_id, limit).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn generate_meet_token(&self, channel_id: &str, room_name: &str) -> Result<String> {
        let transport = self.inner.clone();
        let channel_id = channel_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid channel_id: {e}"))?;
        let room_name = room_name.to_string();
        runtime()
            .spawn(async move {
                transport
                    .generate_meet_token(channel_id, &room_name)
                    .await
                    .map(|resp| resp.token)
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_clan_badge_count(&self) -> Result<Vec<(String, i32, bool)>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_clan_badge_count_typed().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_notification_clan(&self, clan_id: i64) -> Result<i32> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.get_notification_clan(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_clan_descs(&self) -> Result<Vec<crate::transport::ApiClanDesc>> {
        tracing::debug!("TransportClient::list_clan_descs() called");

        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_clan_descs().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// Create a new clan.
    pub async fn create_clan_desc(
        &self,
        clan_name: &str,
        logo: &str,
        banner: &str,
    ) -> Result<crate::transport::ApiClanDesc> {
        tracing::debug!("TransportClient::create_clan_desc() called");

        let transport = self.inner.clone();
        let clan_name = clan_name.to_string();
        let logo = logo.to_string();
        let banner = banner.to_string();

        runtime()
            .spawn(async move { transport.create_clan_desc(&clan_name, &logo, &banner).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn ping_roundtrip(&self) -> Result<()> {
        tracing::debug!("TransportClient::ping_roundtrip() called");

        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.ping_roundtrip().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn is_open(&self) -> bool {
        self.inner.is_open().await
    }

    pub async fn list_channel_messages(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        direction: i32,
        limit: u32,
    ) -> Result<Vec<crate::transport::ApiMessage>> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move {
                transport
                    .list_channel_messages(clan_id, channel_id, message_id, direction, limit)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// List threads for a parent channel.
    pub async fn list_thread_descs(
        &self,
        channel_id: &str,
        clan_id: &str,
        page: i32,
    ) -> Result<Vec<crate::transport::ApiThreadDesc>> {
        use crate::transport::THREAD_LIST_LIMIT;
        let transport = self.inner.clone();
        let channel_id = channel_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid channel_id: {e}"))?;
        let clan_id = clan_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid clan_id: {e}"))?;

        runtime()
            .spawn(async move {
                transport
                    .list_thread_descs(channel_id, clan_id, THREAD_LIST_LIMIT, page, 0, None)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// Search threads by label within a parent channel.
    pub async fn search_thread(
        &self,
        clan_id: &str,
        channel_id: &str,
        label: &str,
    ) -> Result<Vec<crate::transport::ApiThreadDesc>> {
        let transport = self.inner.clone();
        let clan_id = clan_id.to_string();
        let channel_id = channel_id.to_string();
        let label = label.to_string();

        runtime()
            .spawn(async move { transport.search_thread(&clan_id, &channel_id, &label).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn join_chat(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_type: i32,
        is_public: bool,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .join_chat(clan_id, channel_id, channel_type, is_public)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn join_clan_chat(&self, clan_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.join_clan_chat(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_clan_users_status(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::ClanUserStatusList> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_clan_users_status(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_roles(
        &self,
        clan_id: i64,
        limit: i32,
        cursor: &str,
    ) -> Result<mezon_proto::api::RoleListEventResponse> {
        let transport = self.inner.clone();
        let cursor = cursor.to_string();
        runtime()
            .spawn(async move { transport.list_roles(clan_id, limit, &cursor).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
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
    ) -> Result<crate::transport::ApiMessage> {
        let transport = self.inner.clone();
        let content = content.to_string();

        runtime()
            .spawn(async move {
                transport
                    .send_channel_message(
                        clan_id, channel_id, &content, is_public, mode, mentions, hashtags, emojis,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

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
    ) -> Result<crate::transport::ApiMessage> {
        let transport = self.inner.clone();
        let content = content.to_string();
        runtime()
            .spawn(async move {
                transport
                    .send_channel_message_reply(
                        clan_id, channel_id, &content, is_public, mode, reply, mentions, hashtags,
                        emojis,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn send_channel_message_with_attachments(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        attachments: Vec<mezon_proto::api::MessageAttachment>,
    ) -> Result<crate::transport::ApiMessage> {
        let transport = self.inner.clone();
        let content = content.to_string();

        runtime()
            .spawn(async move {
                transport
                    .send_channel_message_with_attachments(
                        clan_id,
                        channel_id,
                        &content,
                        is_public,
                        mode,
                        attachments,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn create_channel(
        &self,
        clan_id: i64,
        channel_label: &str,
        channel_type: u32,
        category_id: Option<i64>,
        parent_id: Option<i64>,
        channel_private: i32,
    ) -> Result<crate::transport::ApiChannelDesc> {
        let transport = self.inner.clone();
        let channel_label = channel_label.to_string();

        runtime()
            .spawn(async move {
                transport
                    .create_channel(
                        clan_id,
                        &channel_label,
                        channel_type,
                        category_id,
                        parent_id,
                        channel_private,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn create_category_desc(
        &self,
        category_name: &str,
        clan_id: i64,
    ) -> Result<mezon_proto::api::CategoryDesc> {
        let transport = self.inner.clone();
        let category_name = category_name.to_string();

        runtime()
            .spawn(async move {
                transport
                    .create_category_desc(&category_name, clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// Add users to a channel.
    pub async fn add_channel_users(&self, channel_id: i64, user_ids: Vec<String>) -> Result<()> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move {
                let refs: Vec<&str> = user_ids.iter().map(String::as_str).collect();
                transport.add_channel_users(channel_id, &refs).await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn close(&self) -> Result<()> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.close().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))??;

        Ok(())
    }

    pub async fn update_user(&self, display_name: &str, avatar_url: &str) -> Result<()> {
        let transport = self.inner.clone();
        let display_name = display_name.to_string();
        let avatar_url = avatar_url.to_string();

        runtime()
            .spawn(async move { transport.update_user(&display_name, &avatar_url).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_loged_device(&self) -> Result<Vec<mezon_proto::api::LogedDevice>> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_loged_device().await.map(|l| l.devices) })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_account(
        &self,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
        about_me: Option<&str>,
    ) -> Result<()> {
        tracing::debug!("TransportClient::update_account() called");

        let transport = self.inner.clone();
        let display_name = display_name.map(str::to_string);
        let avatar_url = avatar_url.map(str::to_string);
        let about_me = about_me.map(str::to_string);

        runtime()
            .spawn(async move {
                transport
                    .update_account(
                        display_name.as_deref(),
                        avatar_url.as_deref(),
                        about_me.as_deref(),
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn upload_attachment_file(
        &self,
        filename: &str,
        filetype: &str,
        size: i32,
        width: i32,
        height: i32,
    ) -> Result<mezon_proto::api::UploadAttachment> {
        let transport = self.inner.clone();
        let filename = filename.to_string();
        let filetype = filetype.to_string();

        runtime()
            .spawn(async move {
                transport
                    .upload_attachment_file(&filename, &filetype, size, width, height)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_user_profile_on_clan(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::ClanProfile> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.get_user_profile_on_clan(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_user_profile_by_clan(
        &self,
        clan_id: i64,
        nick_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let nick_name = nick_name.to_string();
        let avatar_url = avatar_url.map(str::to_string);

        runtime()
            .spawn(async move {
                transport
                    .update_user_profile_by_clan(clan_id, &nick_name, avatar_url.as_deref())
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn check_duplicate_name(
        &self,
        name: &str,
        r#type: i32,
        condition_id: i64,
    ) -> Result<mezon_proto::api::CheckDuplicateNameResponse> {
        let transport = self.inner.clone();
        let name = name.to_string();

        runtime()
            .spawn(async move {
                transport
                    .check_duplicate_name(&name, r#type, condition_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// Check duplicate thread name within a parent channel.
    pub async fn check_duplicate_thread_name(
        &self,
        name: &str,
        parent_channel_id: &str,
    ) -> Result<bool> {
        let transport = self.inner.clone();
        let name = name.to_string();
        let parent_channel_id = parent_channel_id.to_string();

        runtime()
            .spawn(async move {
                transport
                    .check_duplicate_thread_name(&name, &parent_channel_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn session_logout(&self, token: &str, refresh_token: &str) -> Result<()> {
        let transport = self.inner.clone();
        let token = token.to_string();
        let refresh_token = refresh_token.to_string();

        runtime()
            .spawn(async move { transport.session_logout(&token, &refresh_token).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn logout_device(
        &self,
        token: &str,
        refresh_token: &str,
        device_id: &str,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let token = token.to_string();
        let refresh_token = refresh_token.to_string();
        let device_id = device_id.to_string();

        runtime()
            .spawn(async move {
                transport
                    .logout_device(&token, &refresh_token, &device_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_list_favorite_channel(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::ListFavoriteChannelResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.get_list_favorite_channel(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_channel_apps(
        &self,
        clan_id: i64,
    ) -> Result<Vec<crate::transport::ApiChannelApp>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_channel_apps(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn add_channel_favorite(&self, channel_id: i64, clan_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.add_channel_favorite(channel_id, clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn remove_channel_favorite(&self, channel_id: i64, clan_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.remove_channel_favorite(channel_id, clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }
}
