use crate::channel::ChannelList;
use crate::ids::{ChannelId, ClanId, UserId};
use std::path::Path;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::transport::ApiClanDesc;
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};

use crate::realtime::{RealtimeDispatch, RealtimeKind};

#[derive(Debug, Clone)]
pub struct Clan {
    pub id: ClanId,
    pub creator_id: UserId,
    pub name: String,
    pub avatar_url: Option<String>,
    pub banner_url: Option<String>,
    pub badge_count: u32,
    pub has_unread: bool,
    pub muted: bool,
    pub welcome_channel_id: Option<ChannelId>,
    pub status: i32,
    pub is_onboarding: bool,
    pub is_community: bool,
    pub prevent_anonymous: bool,
}

impl From<ApiClanDesc> for Clan {
    fn from(c: ApiClanDesc) -> Self {
        let avatar_url = (!c.logo.is_empty()).then_some(c.logo);
        let banner_url = (!c.banner.is_empty()).then_some(c.banner);
        let welcome_channel_id =
            (c.welcome_channel_id != 0).then_some(ChannelId(c.welcome_channel_id));
        Self {
            id: ClanId(c.clan_id),
            creator_id: UserId(c.creator_id),
            name: c.clan_name,
            avatar_url,
            banner_url,
            badge_count: 0,
            has_unread: false,
            muted: false,
            welcome_channel_id,
            status: 0,
            is_onboarding: false,
            is_community: false,
            prevent_anonymous: false,
        }
    }
}

/// Typed events emitted by [`ClanList`] — the analog of Zed's `ChannelEvent`
/// (`channel_store.rs:144`). Other stores/views `cx.subscribe` to react to specific changes.
#[derive(Debug, Clone)]
pub enum ClanEvent {
    /// The active clan changed (or was cleared).
    ActiveClanChanged(Option<ClanId>),
    /// A clan was removed (server push).
    Deleted(ClanId),
}

/// Clan store — owns the clan list, fetches it over REST, and self-subscribes to realtime
/// clan events.
///
/// Native analog of Zed's `ChannelStore` (`crates/channel/src/channel_store.rs`): registered as
/// a [`Global`] (`init`/`global`), an [`EventEmitter`] of [`ClanEvent`], reacting to server
/// pushes in `handle_event`, holding its subscription `Task` so it cancels on drop.
pub struct ClanList {
    pub clans: Vec<Clan>,
    pub active_clan_id: Option<ClanId>,
    api: Arc<AppApi>,
    loading: bool,
    _connection_watch: Task<()>,
}

struct GlobalClanList(Entity<ClanList>);
impl Global for GlobalClanList {}

impl EventEmitter<ClanEvent> for ClanList {}

impl ClanList {
    /// Create the store and register it as the app-wide global. Cf. `ChannelStore::init`
    /// (`channel_store.rs:25`). Call once during app setup, before any view reads it.
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalClanList(entity.clone()));
        entity
    }

    /// The global clan store. Panics if [`ClanList::init`] hasn't run. Cf. `ChannelStore::global`.
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalClanList>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalClanList>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.clans.clear();
        self.loading = false;
        if self.active_clan_id.take().is_some() {
            cx.emit(ClanEvent::ActiveClanChanged(None));
        }
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        let connection_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            clans: Vec::new(),
            active_clan_id: None,
            api,
            loading: false,
            _connection_watch: connection_watch,
        }
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::ClanUpdated,
                RealtimeKind::ClanDeleted,
                RealtimeKind::AddClanUser,
                RealtimeKind::UserClanRemoved,
            ] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.handle_event(event, cx)
                });
            }
            dispatch.on_lagged(&entity, |this, cx| {
                tracing::warn!("ClanList realtime lagged — reloading clans");
                this.reload(cx);
            });
        });
    }

    fn spawn_connection_watch(api: Arc<AppApi>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut status_rx = api.status();
            let mut was_connected = false;
            loop {
                if status_rx.changed().await.is_err() {
                    break;
                }
                let connected = *status_rx.borrow() == ConnectionStatus::Connected;
                if connected && !was_connected {
                    was_connected = true;
                    // Reconnected — realtime pushes were missed while offline, so the cached list
                    // is stale: always refetch (not just when empty).
                    if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            const MAX_RETRIES: u32 = 3;
            let mut attempt = 0u32;
            let (clans, badges_result) = loop {
                let (clans_result, badges_result) =
                    tokio::join!(api.list_clan_descs(), api.list_clan_badge_count());
                match clans_result {
                    Ok(c) => break (c, badges_result),
                    Err(e) if attempt < MAX_RETRIES => {
                        attempt += 1;
                        tracing::warn!("Failed to load clans (attempt {attempt}): {e}, retrying");
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(2 * attempt as u64))
                            .await;
                    }
                    Err(e) => {
                        tracing::error!("Failed to load clans after {attempt} retries: {e}");
                        let _ = this.update(cx, |this, _| {
                            this.loading = false;
                        });
                        return;
                    }
                }
            };
            let badge_map: std::collections::HashMap<String, (i32, bool)> = badges_result
                .unwrap_or_else(|e| {
                    tracing::warn!("clan badge count fetch failed: {e}");
                    Vec::new()
                })
                .into_iter()
                .map(|(id, badge, has_unread)| (id, (badge, has_unread)))
                .collect();
            let mapped: Vec<Clan> = clans
                .into_iter()
                .map(|c| {
                    let mut clan = Clan::from(c);
                    if let Some(&(badge, has_unread)) = badge_map.get(&clan.id.to_string()) {
                        clan.badge_count = badge.max(0) as u32;
                        clan.has_unread = has_unread;
                    }
                    clan
                })
                .collect();
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.update_clans(mapped, cx);
                if let Some(clan_id) = this.active_clan_id {
                    this.fire_join_clan_chat(clan_id, cx);
                }
            });
        })
        .detach();
    }

    /// Apply a server-pushed realtime event. Cf. `ChannelStore::handle_update_channels`.
    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        match event {
            RealtimeEvent::ClanDeleted(e) => {
                let id = ClanId(e.clan_id);
                let before = self.clans.len();
                self.clans.retain(|c| c.id != id);
                if self.clans.len() != before {
                    cx.emit(ClanEvent::Deleted(id));
                    if self.active_clan_id == Some(id) {
                        let next = self.clans.first().map(|c| c.id);
                        self.active_clan_id = next;
                        cx.emit(ClanEvent::ActiveClanChanged(next));
                    }
                    cx.notify();
                }
            }
            RealtimeEvent::ClanUpdated(e) => {
                let name = (!e.clan_name.is_empty()).then_some(e.clan_name.clone());
                let welcome_channel_id =
                    (e.welcome_channel_id != 0).then_some(ChannelId(e.welcome_channel_id));
                let update = ClanUpdate {
                    name,
                    logo: e.logo.clone(),
                    banner: e.banner.clone(),
                    welcome_channel_id,
                    status: e.status,
                    is_onboarding: e.is_onboarding,
                    is_community: e.is_community,
                    prevent_anonymous: e.prevent_anonymous,
                };
                if update_clan(&mut self.clans, ClanId(e.clan_id), update) {
                    cx.notify();
                }
            }
            RealtimeEvent::AddClanUser(e) => {
                let id = ClanId(e.clan_id);
                if !self.clans.iter().any(|c| c.id == id) {
                    self.reload(cx);
                }
            }
            RealtimeEvent::UserClanRemoved(e) => {
                let id = ClanId(e.clan_id);
                let before = self.clans.len();
                self.clans.retain(|c| c.id != id);
                if self.clans.len() != before {
                    cx.emit(ClanEvent::Deleted(id));
                    if self.active_clan_id == Some(id) {
                        let next = self.clans.first().map(|c| c.id);
                        self.active_clan_id = next;
                        cx.emit(ClanEvent::ActiveClanChanged(next));
                    }
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    pub fn set_has_unread_message(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if let Some(clan) = self.clans.iter_mut().find(|c| c.id == clan_id)
            && !clan.muted
        {
            let was_unread = clan.has_unread;
            clan.has_unread = true;
            if !was_unread {
                cx.notify();
            }
        }
    }

    pub fn increment_clan_badge(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if let Some(clan) = self.clans.iter_mut().find(|c| c.id == clan_id)
            && !clan.muted
        {
            let was_badge = clan.badge_count;
            clan.badge_count = clan.badge_count.saturating_add(1);
            if was_badge != clan.badge_count {
                cx.notify();
            }
        }
    }

    pub fn set_has_unread(&mut self, clan_id: ClanId, has_unread: bool, cx: &mut Context<Self>) {
        if let Some(clan) = self.clans.iter_mut().find(|c| c.id == clan_id)
            && !clan.muted
            && clan.has_unread != has_unread
        {
            clan.has_unread = has_unread;
            cx.notify();
        }
    }

    pub fn sync_has_unread_from_channels(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        let channel_list = ChannelList::global(cx).read(cx);
        if !channel_list.is_clan_cache_loaded(clan_id) {
            return;
        }
        let has_unread = channel_list.clan_has_any_unread(clan_id);
        self.set_has_unread(clan_id, has_unread, cx);
    }

    pub fn decrement_badge(&mut self, clan_id: ClanId, amount: u32, cx: &mut Context<Self>) {
        if amount == 0 {
            return;
        }
        if let Some(clan) = self.clans.iter_mut().find(|c| c.id == clan_id) {
            let was_badge = clan.badge_count;
            clan.badge_count = clan.badge_count.saturating_sub(amount);
            if was_badge != clan.badge_count {
                cx.notify();
            }
        }
    }

    pub fn set_badge_count(&mut self, clan_id: ClanId, count: u32, cx: &mut Context<Self>) {
        if let Some(clan) = self.clans.iter_mut().find(|c| c.id == clan_id)
            && clan.badge_count != count
        {
            clan.badge_count = count;
            cx.notify();
        }
    }

    pub fn apply_badge_read(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if let Some(clan) = self.clans.iter_mut().find(|c| c.id == clan_id)
            && (clan.badge_count > 0 || clan.has_unread)
        {
            clan.badge_count = 0;
            clan.has_unread = false;
            cx.notify();
        }
    }

    pub fn active_clan(&self) -> Option<&Clan> {
        self.active_clan_id
            .as_ref()
            .and_then(|id| self.clans.iter().find(|c| c.id == *id))
    }

    pub fn clan(&self, clan_id: ClanId) -> Option<&Clan> {
        self.clans.iter().find(|c| c.id == clan_id)
    }

    pub fn active_clan_banner(&self) -> Option<&str> {
        self.active_clan().and_then(|c| c.banner_url.as_deref())
    }

    pub fn is_active_clan(&self, clan_id: ClanId) -> bool {
        self.active_clan_id == Some(clan_id)
    }

    pub fn welcome_channel_id(&self, clan_id: ClanId) -> Option<ChannelId> {
        self.clans
            .iter()
            .find(|c| c.id == clan_id)
            .and_then(|c| c.welcome_channel_id)
    }

    fn fire_join_clan_chat(&self, clan_id: ClanId, cx: &mut Context<Self>) {
        let api = self.api.clone();
        let id = clan_id.get();
        cx.spawn(async move |_, _| {
            if let Err(e) = api.join_clan_chat(id).await {
                tracing::error!("join_clan_chat failed for clan {id}: {e}");
            }
        })
        .detach();
    }

    pub fn select_clan(&mut self, id: ClanId, cx: &mut Context<Self>) {
        if self.active_clan_id == Some(id) {
            return;
        }
        self.active_clan_id = Some(id);
        self.fire_join_clan_chat(id, cx);
        cx.emit(ClanEvent::ActiveClanChanged(self.active_clan_id));
        cx.notify();
    }

    pub fn update_clans(&mut self, clans: Vec<Clan>, cx: &mut Context<Self>) {
        let prev_active = self.active_clan_id;
        self.clans = clans;
        if !self.clans.is_empty() {
            let active_still_valid = self
                .active_clan_id
                .as_ref()
                .is_some_and(|id| self.clans.iter().any(|c| c.id == *id));
            if !active_still_valid {
                self.active_clan_id = Some(self.clans[0].id);
            }
        }
        if self.active_clan_id != prev_active {
            cx.emit(ClanEvent::ActiveClanChanged(self.active_clan_id));
        }
        cx.notify();
    }

    pub fn create_clan(
        &mut self,
        name: String,
        logo: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<String, CreateClanError>> {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let trimmed = name.trim().to_string();
            let is_dup = api
                .check_duplicate_clan_name(&trimmed, "0")
                .await
                .map_err(|e| CreateClanError::Other(e.to_string()))?;
            if is_dup {
                return Err(CreateClanError::DuplicateName);
            }
            let desc = api
                .create_clan_desc(&trimmed, &logo, "")
                .await
                .map_err(|e| CreateClanError::Other(e.to_string()))?;
            let clan_id = desc.clan_id;
            this.update(cx, |this, cx| {
                apply_created_clan(&mut this.clans, desc);
                this.select_clan(ClanId(clan_id), cx);
            })
            .map_err(|_| CreateClanError::Other("store dropped".into()))?;
            Ok(clan_id.to_string())
        })
    }

    pub fn upload_clan_logo(
        &self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<String>> {
        let api = self.api.clone();
        let path = path.to_path_buf();
        cx.spawn(async move |_this, cx| {
            cx.background_executor()
                .spawn(async move { api.upload_avatar(&path).await })
                .await
        })
    }

    pub fn reorder_clans(&mut self, order: Vec<ClanId>, cx: &mut Context<Self>) {
        apply_clan_order(&mut self.clans, &order);
        cx.notify();
        cx.background_executor()
            .spawn(async move {
                let mut settings = crate::Settings::load_sync();
                settings.clan_order = order;
                settings.save_sync();
            })
            .detach();
    }

    pub fn apply_saved_order(&mut self, order: &[ClanId]) {
        apply_clan_order(&mut self.clans, order);
    }
}

fn apply_clan_order(clans: &mut Vec<Clan>, order: &[ClanId]) {
    if order.is_empty() {
        return;
    }
    let mut ordered: Vec<Clan> = Vec::with_capacity(clans.len());
    for id in order {
        if let Some(pos) = clans.iter().position(|c| c.id == *id) {
            ordered.push(clans.remove(pos));
        }
    }
    ordered.append(clans);
    *clans = ordered;
}

struct ClanUpdate {
    name: Option<String>,
    logo: String,
    banner: String,
    welcome_channel_id: Option<ChannelId>,
    status: i32,
    is_onboarding: bool,
    is_community: bool,
    prevent_anonymous: bool,
}

fn update_clan(clans: &mut [Clan], clan_id: ClanId, update: ClanUpdate) -> bool {
    let Some(clan) = clans.iter_mut().find(|c| c.id == clan_id) else {
        return false;
    };
    if let Some(name) = update.name {
        clan.name = name;
    }
    clan.avatar_url = (!update.logo.is_empty()).then_some(update.logo);
    if !update.banner.is_empty() {
        clan.banner_url = Some(update.banner);
    }
    if let Some(wc) = update.welcome_channel_id {
        clan.welcome_channel_id = Some(wc);
    }
    clan.status = update.status;
    clan.is_onboarding = update.is_onboarding;
    clan.is_community = update.is_community;
    clan.prevent_anonymous = update.prevent_anonymous;
    true
}

#[derive(Debug)]
pub enum CreateClanError {
    DuplicateName,
    Other(String),
}

impl std::fmt::Display for CreateClanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateName => write!(f, "A clan with that name already exists."),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

pub(crate) fn apply_created_clan(clans: &mut Vec<Clan>, desc: ApiClanDesc) {
    let clan = Clan::from(desc);
    if !clans.iter().any(|c| c.id == clan.id) {
        clans.push(clan);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_clan(id: i64, name: &str, avatar_url: Option<&str>) -> Clan {
        Clan {
            id: ClanId(id),
            creator_id: UserId(0),
            name: name.into(),
            avatar_url: avatar_url.map(|s| s.into()),
            banner_url: None,
            badge_count: 0,
            has_unread: false,
            muted: false,
            welcome_channel_id: None,
            status: 0,
            is_onboarding: false,
            is_community: false,
            prevent_anonymous: false,
        }
    }

    fn make_update(name: Option<&str>, logo: &str) -> ClanUpdate {
        ClanUpdate {
            name: name.map(|s| s.into()),
            logo: logo.into(),
            banner: String::new(),
            welcome_channel_id: None,
            status: 0,
            is_onboarding: false,
            is_community: false,
            prevent_anonymous: false,
        }
    }

    fn clans() -> Vec<Clan> {
        vec![
            make_clan(1, "One", None),
            make_clan(2, "Two", Some("old.png")),
        ]
    }

    #[test]
    fn update_clan_sets_name_and_logo() {
        let mut c = clans();
        assert!(update_clan(
            &mut c,
            ClanId(1),
            make_update(Some("NewName"), "logo.png")
        ));
        assert_eq!(c[0].name, "NewName");
        assert_eq!(c[0].avatar_url.as_deref(), Some("logo.png"));
    }

    #[test]
    fn update_clan_blank_name_keeps_name_and_empty_logo_clears_avatar() {
        let mut c = clans();
        assert!(update_clan(&mut c, ClanId(2), make_update(None, "")));
        assert_eq!(c[1].name, "Two");
        assert_eq!(c[1].avatar_url, None);
    }

    #[test]
    fn update_clan_unknown_is_noop() {
        let mut c = clans();
        assert!(!update_clan(
            &mut c,
            ClanId(999),
            make_update(Some("x"), "y")
        ));
    }

    #[test]
    fn update_clan_applies_all_fields() {
        let mut c = clans();
        let update = ClanUpdate {
            name: Some("NewName".into()),
            logo: "logo.png".into(),
            banner: "banner.png".into(),
            welcome_channel_id: Some(ChannelId(42)),
            status: 1,
            is_onboarding: true,
            is_community: true,
            prevent_anonymous: true,
        };
        assert!(update_clan(&mut c, ClanId(1), update));
        assert_eq!(c[0].name, "NewName");
        assert_eq!(c[0].avatar_url.as_deref(), Some("logo.png"));
        assert_eq!(c[0].banner_url.as_deref(), Some("banner.png"));
        assert_eq!(c[0].welcome_channel_id, Some(ChannelId(42)));
        assert_eq!(c[0].status, 1);
        assert!(c[0].is_onboarding);
        assert!(c[0].is_community);
        assert!(c[0].prevent_anonymous);
    }

    #[test]
    fn clan_from_api_desc_zeroes_badge_and_muted() {
        use mezon_client::transport::ApiClanDesc;
        let desc = ApiClanDesc {
            clan_id: 42,
            clan_name: "Alpha".into(),
            creator_id: 0,
            logo: "logo.png".into(),
            banner: String::new(),
            welcome_channel_id: 0,
        };
        let clan = Clan::from(desc);
        assert_eq!(clan.badge_count, 0);
        assert!(!clan.has_unread);
        assert!(!clan.muted);
        assert_eq!(clan.avatar_url.as_deref(), Some("logo.png"));
        assert!(clan.welcome_channel_id.is_none());
    }

    #[test]
    fn clan_from_api_desc_maps_creator_id() {
        use mezon_client::transport::ApiClanDesc;
        let desc = ApiClanDesc {
            clan_id: 42,
            clan_name: "Alpha".into(),
            creator_id: 7,
            logo: String::new(),
            banner: String::new(),
            welcome_channel_id: 0,
        };
        assert_eq!(Clan::from(desc).creator_id, UserId(7));
    }

    #[test]
    fn badge_map_applies_to_clans_on_reload() {
        let mut c = clans();
        let badge_map: std::collections::HashMap<String, (i32, bool)> = [
            ("1".to_string(), (3_i32, true)),
            ("99".to_string(), (5_i32, false)),
        ]
        .into_iter()
        .collect();
        for clan in &mut c {
            if let Some(&(badge, has_unread)) = badge_map.get(&clan.id.to_string()) {
                clan.badge_count = badge.max(0) as u32;
                clan.has_unread = has_unread;
            }
        }
        assert_eq!(c[0].badge_count, 3);
        assert!(c[0].has_unread);
        assert_eq!(c[1].badge_count, 0);
        assert!(!c[1].has_unread);
    }

    #[test]
    fn set_has_unread_message_sets_flag() {
        let mut c = clans();
        if let Some(clan) = c.iter_mut().find(|c| c.id == ClanId(1)) && !clan.muted {
            clan.has_unread = false;
        }
        if let Some(clan) = c.iter_mut().find(|c| c.id == ClanId(1)) && !clan.muted {
            clan.has_unread = true;
        }
        assert!(c[0].has_unread);
        assert_eq!(c[0].badge_count, 0);
    }

    #[test]
    fn increment_clan_badge_when_not_muted() {
        let mut c = clans();
        if let Some(clan) = c.iter_mut().find(|c| c.id == ClanId(1)) && !clan.muted {
            clan.badge_count = clan.badge_count.saturating_add(1);
        }
        assert_eq!(c[0].badge_count, 1);
    }

    #[test]
    fn increment_clan_badge_skipped_when_muted() {
        let mut c = clans();
        c[0].muted = true;
        if let Some(clan) = c.iter_mut().find(|cl| cl.id == ClanId(1)) && !clan.muted {
            clan.badge_count = clan.badge_count.saturating_add(1);
        }
        assert_eq!(c[0].badge_count, 0);
    }

    #[test]
    fn mark_as_read_resets_badge_and_unread() {
        use mezon_proto::realtime;
        let mut c = clans();
        c[0].badge_count = 7;
        c[0].has_unread = true;
        let evt = realtime::MarkAsRead {
            clan_id: 1,
            ..Default::default()
        };
        if let Some(clan) = c.iter_mut().find(|cl| cl.id == ClanId(evt.clan_id))
            && (clan.badge_count > 0 || clan.has_unread)
        {
            clan.badge_count = 0;
            clan.has_unread = false;
        }
        assert_eq!(c[0].badge_count, 0);
        assert!(!c[0].has_unread);
    }

    #[test]
    fn mark_as_read_unknown_clan_is_noop() {
        use mezon_proto::realtime;
        let mut c = clans();
        c[0].badge_count = 3;
        let evt = realtime::MarkAsRead {
            clan_id: 999,
            ..Default::default()
        };
        if let Some(clan) = c.iter_mut().find(|cl| cl.id == ClanId(evt.clan_id)) {
            clan.badge_count = 0;
            clan.has_unread = false;
        }
        assert_eq!(c[0].badge_count, 3);
    }

    #[test]
    fn apply_created_clan_inserts_new_clan() {
        use mezon_client::transport::ApiClanDesc;
        let mut clans = clans();
        let desc = ApiClanDesc {
            clan_id: 99,
            clan_name: "NewClan".into(),
            creator_id: 0,
            logo: "logo.png".into(),
            banner: String::new(),
            welcome_channel_id: 0,
        };
        apply_created_clan(&mut clans, desc);
        assert_eq!(clans.len(), 3);
        let inserted = clans.iter().find(|c| c.id == ClanId(99)).unwrap();
        assert_eq!(inserted.name, "NewClan");
        assert_eq!(inserted.avatar_url.as_deref(), Some("logo.png"));
        assert_eq!(inserted.badge_count, 0);
        assert!(!inserted.has_unread);
    }

    #[test]
    fn apply_created_clan_skips_duplicate_id() {
        use mezon_client::transport::ApiClanDesc;
        let mut clans = clans();
        let desc = ApiClanDesc {
            clan_id: 1,
            clan_name: "SameClan".into(),
            creator_id: 0,
            logo: String::new(),
            banner: String::new(),
            welcome_channel_id: 0,
        };
        apply_created_clan(&mut clans, desc);
        assert_eq!(clans.len(), 2);
        assert_eq!(clans[0].name, "One");
    }

    #[test]
    fn create_clan_error_display_duplicate_name() {
        let err = CreateClanError::DuplicateName;
        let msg = format!("{err}");
        assert!(msg.contains("already exists"));
    }

    #[test]
    fn create_clan_error_display_other() {
        let err = CreateClanError::Other("network timeout".into());
        let msg = format!("{err}");
        assert_eq!(msg, "network timeout");
    }
}
