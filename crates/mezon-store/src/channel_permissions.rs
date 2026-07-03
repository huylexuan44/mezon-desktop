use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus};
use mezon_proto::api;

use crate::KeyedCache;
use crate::ids::{ChannelId, ClanId};

pub const PERMISSION_MANAGE_THREAD: &str = "manage-thread";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ChannelPermissionKey {
    clan_id: ClanId,
    channel_id: ChannelId,
}

#[derive(Debug, Clone)]
pub enum ChannelPermissionsEvent {
    Changed {
        clan_id: ClanId,
        channel_id: ChannelId,
    },
}

pub struct ChannelPermissionsStore {
    cache: KeyedCache<ChannelPermissionKey, HashMap<String, bool>>,
    loading: HashSet<ChannelPermissionKey>,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
}

struct GlobalChannelPermissionsStore(Entity<ChannelPermissionsStore>);
impl Global for GlobalChannelPermissionsStore {}

impl EventEmitter<ChannelPermissionsEvent> for ChannelPermissionsStore {}

impl ChannelPermissionsStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalChannelPermissionsStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalChannelPermissionsStore>().0.clone()
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            cache: KeyedCache::new(None),
            loading: HashSet::new(),
            api,
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
                    if this.update(cx, |this, _| this.invalidate()).is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    fn invalidate(&mut self) {
        self.cache.mark_all_stale();
    }

    pub fn has_permission(&self, slug: &str, clan_id: ClanId, channel_id: ChannelId) -> bool {
        let key = ChannelPermissionKey {
            clan_id,
            channel_id,
        };
        self.cache
            .get(&key)
            .and_then(|perms| perms.get(slug).copied())
            .unwrap_or(false)
    }

    pub fn ensure_loaded(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let key = ChannelPermissionKey {
            clan_id,
            channel_id,
        };
        if self.cache.is_fresh(&key, crate::CACHE_TTL) {
            return;
        }
        self.fetch(clan_id, channel_id, cx);
    }

    fn fetch(&mut self, clan_id: ClanId, channel_id: ChannelId, cx: &mut Context<Self>) {
        let key = ChannelPermissionKey {
            clan_id,
            channel_id,
        };
        if self.loading.contains(&key) {
            return;
        }
        self.loading.insert(key);
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_user_permission_in_channel(clan_id.get(), channel_id.get())
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading.remove(&key);
                match result {
                    Ok(resp) => {
                        let perms = permissions_from_response(&resp);
                        this.cache.insert(key, perms, None);
                        cx.emit(ChannelPermissionsEvent::Changed {
                            clan_id,
                            channel_id,
                        });
                        cx.notify();
                    }
                    Err(e) => {
                        tracing::error!(
                            "list_user_permission_in_channel failed for {channel_id}: {e}"
                        );
                    }
                }
            });
        })
        .detach();
    }
}

fn permissions_from_response(
    resp: &api::UserPermissionInChannelListResponse,
) -> HashMap<String, bool> {
    let mut map = HashMap::new();
    if let Some(list) = &resp.permissions {
        for perm in &list.permissions {
            if !perm.slug.is_empty() {
                map.insert(perm.slug.clone(), perm.active != 0);
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_from_response_maps_active_flag() {
        let resp = api::UserPermissionInChannelListResponse {
            permissions: Some(api::PermissionList {
                max_level_permission: 0,
                permissions: vec![
                    api::Permission {
                        slug: "manage-thread".into(),
                        active: 1,
                        ..Default::default()
                    },
                    api::Permission {
                        slug: "send-message".into(),
                        active: 0,
                        ..Default::default()
                    },
                ],
            }),
            ..Default::default()
        };
        let perms = permissions_from_response(&resp);
        assert_eq!(perms.get("manage-thread"), Some(&true));
        assert_eq!(perms.get("send-message"), Some(&false));
    }
}
