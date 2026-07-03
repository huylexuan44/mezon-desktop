use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_client::{AppApi, ConnectionStatus};

use crate::AuthState;
use crate::clan::{ClanEvent, ClanList};
use crate::ids::{ClanId, UserId};

pub const PERMISSION_CLAN_OWNER: &str = "clan-owner";
pub const PERMISSION_ADMINISTRATOR: &str = "administrator";
pub const PERMISSION_MANAGE_CHANNEL: &str = "manage-channel";
pub const PERMISSION_MANAGE_CLAN: &str = "manage-clan";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClanSettingsPermissions {
    pub is_clan_owner: bool,
    pub has_manage_clan: bool,
    pub has_manage_channel: bool,
}

impl ClanSettingsPermissions {
    pub fn none() -> Self {
        Self {
            is_clan_owner: false,
            has_manage_clan: false,
            has_manage_channel: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum PermissionEvent {
    Changed { clan_id: Option<ClanId> },
}

pub struct PermissionStore {
    catalog: HashMap<String, i32>,
    catalog_loaded: bool,
    catalog_loading: bool,
    max_level_by_clan: HashMap<ClanId, i32>,
    loading_clans: HashSet<ClanId>,
    api: Arc<AppApi>,
    auth_state: Entity<AuthState>,
    _clan_sub: Subscription,
    _conn_watch: Task<()>,
}

struct GlobalPermissionStore(Entity<PermissionStore>);
impl Global for GlobalPermissionStore {}

impl EventEmitter<PermissionEvent> for PermissionStore {}

impl PermissionStore {
    pub fn init(api: Arc<AppApi>, auth_state: Entity<AuthState>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, auth_state, cx));
        cx.set_global(GlobalPermissionStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalPermissionStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalPermissionStore>()
            .map(|g| g.0.clone())
    }

    fn new(api: Arc<AppApi>, auth_state: Entity<AuthState>, cx: &mut Context<Self>) -> Self {
        let clan_sub = cx.subscribe(&ClanList::global(cx), |this, _clan, event, cx| {
            if let ClanEvent::ActiveClanChanged(Some(clan_id)) = event {
                this.load_clan_permissions(*clan_id, cx);
            }
        });

        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        Self {
            catalog: HashMap::new(),
            catalog_loaded: false,
            catalog_loading: false,
            max_level_by_clan: HashMap::new(),
            loading_clans: HashSet::new(),
            api,
            auth_state,
            _clan_sub: clan_sub,
            _conn_watch: conn_watch,
        }
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
                    if this
                        .update(cx, |this, cx| {
                            this.catalog_loaded = false;
                            this.max_level_by_clan.clear();
                            this.load_permission_catalog(cx);
                            if let Some(clan_id) = ClanList::global(cx).read(cx).active_clan_id {
                                this.load_clan_permissions(clan_id, cx);
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    fn current_user_id(&self, cx: &App) -> Option<UserId> {
        match self.auth_state.read(cx) {
            AuthState::Authenticated(session) | AuthState::Connecting(session) => {
                session.user_id.parse::<i64>().ok().map(UserId)
            }
            _ => None,
        }
    }

    fn is_clan_owner(&self, clan_id: ClanId, cx: &App) -> bool {
        let Some(user_id) = self.current_user_id(cx) else {
            return false;
        };
        ClanList::global(cx)
            .read(cx)
            .clan_by_id(clan_id)
            .is_some_and(|clan| clan.creator_id == user_id)
    }

    fn has_permission_level(&self, max_level: Option<i32>, slug: &str) -> bool {
        let Some(max_level) = max_level else {
            return false;
        };
        let Some(required_level) = self.catalog.get(slug) else {
            return false;
        };
        *required_level <= max_level
    }

    pub fn check_permission(&self, clan_id: ClanId, slug: &str, cx: &App) -> bool {
        if slug == PERMISSION_CLAN_OWNER {
            return self.is_clan_owner(clan_id, cx);
        }
        if self.is_clan_owner(clan_id, cx) {
            return true;
        }
        let max_level = self.max_level_by_clan.get(&clan_id).copied();
        self.has_permission_level(max_level, slug)
    }

    pub fn has_clan_permissions_loaded(&self, clan_id: ClanId, cx: &App) -> bool {
        self.is_clan_owner(clan_id, cx) || self.max_level_by_clan.contains_key(&clan_id)
    }

    pub fn clan_settings_permissions(&self, clan_id: ClanId, cx: &App) -> ClanSettingsPermissions {
        let is_clan_owner = self.is_clan_owner(clan_id, cx);
        if is_clan_owner {
            return ClanSettingsPermissions {
                is_clan_owner: true,
                has_manage_clan: true,
                has_manage_channel: true,
            };
        }
        let max_level = self.max_level_by_clan.get(&clan_id).copied();
        ClanSettingsPermissions {
            is_clan_owner: false,
            has_manage_clan: self.has_permission_level(max_level, PERMISSION_MANAGE_CLAN),
            has_manage_channel: self.has_permission_level(max_level, PERMISSION_MANAGE_CHANNEL),
        }
    }

    pub fn load_clan_permissions(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.load_permission_catalog(cx);
        if self.max_level_by_clan.contains_key(&clan_id) || !self.loading_clans.insert(clan_id) {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.get_clan_user_role(clan_id.get()).await;
            let _ = this.update(cx, |this, cx| {
                this.loading_clans.remove(&clan_id);
                match result {
                    Ok(role_list) => {
                        this.max_level_by_clan
                            .insert(clan_id, role_list.max_level_permission);
                        cx.emit(PermissionEvent::Changed {
                            clan_id: Some(clan_id),
                        });
                        cx.notify();
                    }
                    Err(err) => {
                        tracing::error!(
                            "get_clan_user_role failed for clan {}: {err}",
                            clan_id.get()
                        );
                    }
                }
            });
        })
        .detach();
    }

    fn load_permission_catalog(&mut self, cx: &mut Context<Self>) {
        if self.catalog_loaded || self.catalog_loading {
            return;
        }
        self.catalog_loading = true;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.get_list_permission().await;
            let _ = this.update(cx, |this, cx| {
                this.catalog_loading = false;
                match result {
                    Ok(list) => {
                        this.catalog = list
                            .permissions
                            .into_iter()
                            .filter(|p| !p.slug.is_empty())
                            .map(|p| (p.slug, p.level))
                            .collect();
                        this.catalog_loaded = true;
                        cx.emit(PermissionEvent::Changed { clan_id: None });
                        cx.notify();
                    }
                    Err(err) => tracing::error!("get_list_permission failed: {err}"),
                }
            });
        })
        .detach();
    }
}
