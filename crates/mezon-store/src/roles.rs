use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_client::{AppApi, ConnectionStatus};

use crate::KeyedCache;
use crate::clan::{ClanEvent, ClanList};
use crate::ids::{ClanId, RoleId};
use crate::realtime::RealtimeDispatch;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone)]
pub enum RolesEvent {
    Changed { clan_id: ClanId },
}

pub struct RolesStore {
    cache: KeyedCache<ClanId, HashMap<RoleId, Role>>,
    loading: std::collections::HashSet<ClanId>,
    api: Arc<AppApi>,
    _clan_sub: Subscription,
    _conn_watch: Task<()>,
}

struct GlobalRolesStore(Entity<RolesStore>);
impl Global for GlobalRolesStore {}

impl EventEmitter<RolesEvent> for RolesStore {}

impl RolesStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalRolesStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalRolesStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalRolesStore>().map(|g| g.0.clone())
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on_lagged(&entity, |this, cx| {
                this.cache.mark_all_stale();
                this.refresh_active(cx);
            });
        });
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let clan_sub = cx.subscribe(&ClanList::global(cx), |this, _clan, event, cx| {
            if let ClanEvent::ActiveClanChanged(Some(clan_id)) = event {
                this.ensure_loaded(*clan_id, cx);
            }
        });

        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        Self {
            cache: KeyedCache::new(None),
            loading: std::collections::HashSet::new(),
            api,
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
                    if this.update(cx, |this, _| this.invalidate()).is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    fn refresh_active(&mut self, cx: &mut Context<Self>) {
        if let Some(clan_id) = ClanList::global(cx).read(cx).active_clan_id {
            self.fetch(clan_id, cx);
        }
    }

    fn invalidate(&mut self) {
        self.cache.mark_all_stale();
    }

    pub fn ensure_loaded(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if !self.cache.is_fresh(&clan_id, crate::CACHE_TTL) {
            self.fetch(clan_id, cx);
        }
    }

    fn fetch(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if !self.loading.insert(clan_id) {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.list_roles(clan_id.get(), 1000, "").await;
            let _ = this.update(cx, |this, cx| {
                this.loading.remove(&clan_id);
                match result {
                    Ok(resp) => {
                        let proto_roles = resp.roles.map(|rl| rl.roles).unwrap_or_default();
                        let roles = roles_map_from_proto(proto_roles);
                        tracing::info!(
                            "RolesStore: fetched {} roles for clan {clan_id}",
                            roles.len()
                        );
                        this.cache.insert(clan_id, roles, None);
                        cx.emit(RolesEvent::Changed { clan_id });
                        cx.notify();
                    }
                    Err(e) => tracing::error!("list_roles failed for {clan_id}: {e}"),
                }
            });
        })
        .detach();
    }

    pub fn roles_for(&self, clan_id: ClanId, role_ids: &[RoleId]) -> Vec<&Role> {
        let Some(map) = self.cache.get(&clan_id) else {
            return Vec::new();
        };
        role_ids.iter().filter_map(|id| map.get(id)).collect()
    }

    pub fn role(&self, clan_id: ClanId, role_id: RoleId) -> Option<&Role> {
        self.cache.get(&clan_id)?.get(&role_id)
    }
}

fn roles_map_from_proto(roles: Vec<mezon_proto::api::Role>) -> HashMap<RoleId, Role> {
    roles
        .into_iter()
        .filter_map(|r| {
            if r.id == 0 {
                return None;
            }
            Some((
                RoleId(r.id),
                Role {
                    name: r.title,
                    color: r.color,
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mezon_proto::api;

    fn roles_for_in(
        by_clan: &HashMap<ClanId, HashMap<RoleId, Role>>,
        clan_id: ClanId,
        role_ids: &[RoleId],
    ) -> Vec<Role> {
        let Some(map) = by_clan.get(&clan_id) else {
            return Vec::new();
        };
        role_ids
            .iter()
            .filter_map(|id| map.get(id))
            .cloned()
            .collect()
    }

    fn make_role(id: i64, title: &str, color: &str) -> api::Role {
        api::Role {
            id,
            title: title.into(),
            color: color.into(),
            ..Default::default()
        }
    }

    #[test]
    fn maps_proto_roles_to_domain() {
        let map = roles_map_from_proto(vec![
            make_role(1, "Admin", "#ff0000"),
            make_role(2, "Member", "#00ff00"),
        ]);
        assert_eq!(map.len(), 2);
        assert_eq!(map[&RoleId(1)].name, "Admin");
        assert_eq!(map[&RoleId(1)].color, "#ff0000");
        assert_eq!(map[&RoleId(2)].name, "Member");
    }

    #[test]
    fn skips_role_with_zero_id() {
        let map = roles_map_from_proto(vec![make_role(0, "Bad", ""), make_role(1, "Good", "blue")]);
        assert!(!map.contains_key(&RoleId(0)));
        assert!(map.contains_key(&RoleId(1)));
    }

    #[test]
    fn roles_for_returns_matching_roles() {
        let mut by_clan: HashMap<ClanId, HashMap<RoleId, Role>> = HashMap::new();
        by_clan.insert(
            ClanId(1),
            roles_map_from_proto(vec![
                make_role(10, "Admin", "#f00"),
                make_role(20, "Mod", "#0f0"),
            ]),
        );
        let result = roles_for_in(&by_clan, ClanId(1), &[RoleId(10), RoleId(20), RoleId(99)]);
        assert_eq!(result.len(), 2);
        assert!(result.iter().any(|r| r.name == "Admin"));
        assert!(result.iter().any(|r| r.name == "Mod"));
    }

    #[test]
    fn roles_for_returns_empty_for_unknown_clan() {
        let by_clan: HashMap<ClanId, HashMap<RoleId, Role>> = HashMap::new();
        assert!(roles_for_in(&by_clan, ClanId(99), &[RoleId(1)]).is_empty());
    }

    #[test]
    fn keyed_cache_reconnect_marks_stale_without_dropping_values_then_refetches() {
        let mut cache: KeyedCache<ClanId, HashMap<RoleId, Role>> = KeyedCache::new(None);
        cache.insert(
            ClanId(1),
            roles_map_from_proto(vec![make_role(10, "Admin", "#f00")]),
            None,
        );
        assert!(cache.is_fresh(&ClanId(1), crate::CACHE_TTL));

        cache.mark_all_stale();
        assert!(!cache.is_fresh(&ClanId(1), crate::CACHE_TTL));
        assert!(cache.get(&ClanId(1)).is_some());

        cache.insert(
            ClanId(1),
            roles_map_from_proto(vec![make_role(10, "Admin", "#f00")]),
            None,
        );
        assert!(cache.is_fresh(&ClanId(1), crate::CACHE_TTL));
    }
}
