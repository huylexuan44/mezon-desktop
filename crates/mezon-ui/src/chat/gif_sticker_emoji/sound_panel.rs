use std::collections::HashSet;

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FontWeight, ScrollStrategy, SharedString,
    Subscription, UniformListScrollHandle, Window, div, img, prelude::*, px, uniform_list,
};
use mezon_store::{StickerEvent, StickerStore, VoiceStore};
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use crate::components::primitives::{Icon, IconName};
use crate::image_cache::{
    AVATAR_ENTRY_MAX_BYTES, AVATAR_IMAGE_CACHE_BYTES, AVATAR_IMAGE_CACHE_CAPACITY, LruImageCache,
};
use crate::theme::{ActiveTheme, Theme};

const PANEL_H: f32 = 400.;
const RAIL_W: f32 = 44.;
const ROW_PX: f32 = 52.;
const ACCENT_BLUE: u32 = 0x5865f2;

#[derive(Clone)]
struct PickerSound {
    row_id: SharedString,
    name: SharedString,
    name_lc: SharedString,
    url: SharedString,
}

struct SoundCategory {
    clan_id: SharedString,
    name: SharedString,
    rail_id: SharedString,
    logo: SharedString,
    sounds: Vec<PickerSound>,
}

enum PickerRow {
    Header { cat_ix: usize, collapsed: bool },
    Sounds(PickerSound, Option<PickerSound>),
}

pub enum SoundPanelEvent {
    Picked { url: String, filename: String },
}

pub struct SoundPanel {
    query: String,
    categories: Vec<SoundCategory>,
    rows: Vec<PickerRow>,
    header_row: Vec<usize>,
    collapsed: HashSet<String>,
    selected: Option<String>,
    last_preview: Option<String>,
    scroll: UniformListScrollHandle,
    image_cache: Entity<LruImageCache>,
    _sticker_sub: Option<Subscription>,
    _voice_observe: Subscription,
}

impl EventEmitter<SoundPanelEvent> for SoundPanel {}

impl SoundPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let sticker_sub = StickerStore::try_global(cx).map(|store| {
            store.update(cx, |store, cx| store.ensure_loaded(cx));
            cx.subscribe(&store, |this, _store, _event: &StickerEvent, cx| {
                this.rebuild_snapshot(cx);
                cx.notify();
            })
        });
        let last_preview = VoiceStore::global(cx)
            .read(cx)
            .previewing_sound()
            .map(str::to_string);
        let voice_observe = cx.observe(&VoiceStore::global(cx), |this, store, cx| {
            let current = store.read(cx).previewing_sound().map(str::to_string);
            if this.last_preview != current {
                this.last_preview = current;
                cx.notify();
            }
        });
        let image_cache = cx.new(|cx| {
            LruImageCache::avatar_thumbnail(
                "gse-sound-panel",
                AVATAR_IMAGE_CACHE_CAPACITY,
                AVATAR_IMAGE_CACHE_BYTES,
                AVATAR_ENTRY_MAX_BYTES,
                cx,
            )
        });
        let mut panel = Self {
            query: String::new(),
            categories: Vec::new(),
            rows: Vec::new(),
            header_row: Vec::new(),
            collapsed: HashSet::new(),
            selected: None,
            last_preview,
            scroll: UniformListScrollHandle::new(),
            image_cache,
            _sticker_sub: sticker_sub,
            _voice_observe: voice_observe,
        };
        panel.rebuild_snapshot(cx);
        panel
    }

    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query;
            self.rebuild();
            cx.notify();
        }
    }

    fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    fn rebuild_snapshot(&mut self, cx: &App) {
        let mut categories: Vec<SoundCategory> = Vec::new();
        if let Some(store) = StickerStore::try_global(cx) {
            for sound in store.read(cx).sounds() {
                let item = PickerSound {
                    row_id: SharedString::from(format!("gse-sound-{}", sound.id)),
                    name: sound.shortname.clone().into(),
                    name_lc: sound.shortname.to_lowercase().into(),
                    url: sound.src.clone().into(),
                };
                match categories
                    .iter_mut()
                    .find(|c| c.clan_id.as_ref() == sound.clan_id)
                {
                    Some(cat) => cat.sounds.push(item),
                    None => categories.push(SoundCategory {
                        clan_id: sound.clan_id.clone().into(),
                        name: sound.clan_name.clone().into(),
                        rail_id: SharedString::from(format!("gse-sound-rail-{}", sound.clan_id)),
                        logo: sound.logo.clone().into(),
                        sounds: vec![item],
                    }),
                }
            }
        }
        self.categories = categories;
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let needle = self.query.trim().to_lowercase();
        let mut rows = Vec::new();
        let mut header_row = Vec::new();
        let flat = self.searching();
        for (cat_ix, cat) in self.categories.iter().enumerate() {
            let collapsed = !flat && self.collapsed.contains(cat.clan_id.as_ref());
            if !flat {
                header_row.push(rows.len());
                rows.push(PickerRow::Header { cat_ix, collapsed });
                if collapsed {
                    continue;
                }
                for pair in cat.sounds.chunks(2) {
                    rows.push(PickerRow::Sounds(pair[0].clone(), pair.get(1).cloned()));
                }
                continue;
            }
            let matching: Vec<PickerSound> = cat
                .sounds
                .iter()
                .filter(|sound| sound.name_lc.contains(&needle))
                .cloned()
                .collect();
            for pair in matching.chunks(2) {
                rows.push(PickerRow::Sounds(pair[0].clone(), pair.get(1).cloned()));
            }
        }
        self.rows = rows;
        self.header_row = header_row;
    }

    fn toggle_collapse(&mut self, clan_id: String, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&clan_id) {
            self.collapsed.insert(clan_id);
        }
        self.rebuild();
        cx.notify();
    }
}

impl Render for SoundPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let entity = cx.entity();
        let show_rail = !self.searching() && !self.categories.is_empty();

        let rail = show_rail.then(|| {
            let mut rail = div()
                .id("gse-sound-rail")
                .flex()
                .flex_col()
                .items_center()
                .gap_4()
                .w(px(RAIL_W))
                .h_full()
                .py_4()
                .flex_shrink_0()
                .bg(theme.bg_tertiary)
                .overflow_y_scroll();
            for (cat_ix, cat) in self.categories.iter().enumerate() {
                let ent = entity.clone();
                let clan_id = cat.clan_id.to_string();
                let active = self.selected.as_deref() == Some(cat.clan_id.as_ref());
                let mut btn = div()
                    .id(cat.rail_id.clone())
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(28.))
                    .cursor_pointer()
                    .overflow_hidden();
                if active {
                    btn = btn.rounded(px(16.)).bg(gpui::rgb(ACCENT_BLUE));
                } else {
                    btn = btn
                        .rounded(px(24.))
                        .bg(theme.bg_secondary)
                        .hover(|s| s.rounded(px(16.)).bg(gpui::rgb(ACCENT_BLUE)));
                }
                if !cat.logo.is_empty() {
                    btn = btn.child(
                        img(cat.logo.clone())
                            .size_full()
                            .object_fit(gpui::ObjectFit::Cover),
                    );
                } else {
                    btn = btn.child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if active {
                                gpui::white()
                            } else {
                                theme.tokens.text_theme_primary.into()
                            })
                            .child(initial_letter(&cat.name)),
                    );
                }
                let btn = btn.on_click(move |_, _, cx| {
                    ent.update(cx, |this, cx| {
                        this.selected = Some(clan_id.clone());
                        if let Some(&row) = this.header_row.get(cat_ix) {
                            this.scroll.scroll_to_item(row, ScrollStrategy::Top);
                        }
                        cx.notify();
                    });
                });
                rail = rail.child(btn);
            }
            rail
        });

        let count = self.rows.len();
        let list_entity = entity.clone();
        let previewing: Option<SharedString> = VoiceStore::global(cx)
            .read(cx)
            .previewing_sound()
            .map(|url| SharedString::from(url.to_string()));
        let list = uniform_list("gse-sound-list", count, move |range, _window, cx| {
            let theme = cx.theme().clone();
            let this = list_entity.read(cx);
            range
                .map(|ix| match this.rows.get(ix) {
                    Some(PickerRow::Header { cat_ix, collapsed }) => {
                        match this.categories.get(*cat_ix) {
                            Some(cat) => render_header(&theme, cat, *collapsed, &list_entity),
                            None => div().h(px(ROW_PX)).into_any_element(),
                        }
                    }
                    Some(PickerRow::Sounds(left, right)) => {
                        render_sound_row(&theme, left, right.as_ref(), &previewing, &list_entity)
                    }
                    None => div().h(px(ROW_PX)).into_any_element(),
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.scroll)
        .h_full()
        .flex_1();

        let content: AnyElement = if self.categories.is_empty() {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .opacity(0.5)
                .child(
                    Icon::new(IconName::Speaker)
                        .size(px(32.))
                        .text_color(theme.tokens.text_theme_primary),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.tokens.text_theme_primary)
                        .child("No sounds available"),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .px_2()
                .child(list)
                .custom_scrollbars(
                    Scrollbars::always_visible(ScrollAxes::Vertical)
                        .tracked_scroll_handle(&self.scroll),
                    window,
                    cx,
                )
                .into_any_element()
        };

        div()
            .image_cache(self.image_cache.clone())
            .w_full()
            .h(px(PANEL_H))
            .flex()
            .flex_row()
            .overflow_hidden()
            .when_some(rail, |el, rail| el.child(rail))
            .child(content)
    }
}

fn initial_letter(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
}

fn render_header(
    theme: &Theme,
    cat: &SoundCategory,
    collapsed: bool,
    entity: &Entity<SoundPanel>,
) -> AnyElement {
    let ent = entity.clone();
    let clan_id = cat.clan_id.to_string();
    let chevron = if collapsed {
        IconName::ChevronRight
    } else {
        IconName::ChevronDown
    };
    let logo: AnyElement = if !cat.logo.is_empty() {
        img(cat.logo.clone())
            .size(px(16.))
            .rounded_full()
            .object_fit(gpui::ObjectFit::Cover)
            .into_any_element()
    } else {
        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(16.))
            .rounded_full()
            .bg(theme.bg_secondary)
            .text_size(px(8.))
            .font_weight(FontWeight::BOLD)
            .text_color(theme.tokens.text_secondary)
            .child(initial_letter(&cat.name))
            .into_any_element()
    };
    div()
        .id(SharedString::from(format!("gse-sound-cat-{}", cat.clan_id)))
        .h(px(ROW_PX))
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .child(logo)
        .child(
            div()
                .max_w(px(300.))
                .truncate()
                .text_size(px(12.))
                .font_weight(FontWeight::BOLD)
                .text_color(theme.tokens.text_secondary)
                .child(cat.name.to_uppercase()),
        )
        .child(
            Icon::new(chevron)
                .size(px(12.))
                .text_color(theme.tokens.text_secondary),
        )
        .child(div().h(px(1.)).flex_1().ml_2().bg(theme.border))
        .on_click(move |_, _, cx| {
            ent.update(cx, |this, cx| this.toggle_collapse(clan_id.clone(), cx));
        })
        .into_any_element()
}

fn render_sound_row(
    theme: &Theme,
    left: &PickerSound,
    right: Option<&PickerSound>,
    previewing: &Option<SharedString>,
    entity: &Entity<SoundPanel>,
) -> AnyElement {
    let mut row = div().h(px(ROW_PX)).flex().flex_row().items_start().gap_3();
    row = row.child(render_sound_cell(theme, left, previewing, entity));
    match right {
        Some(sound) => row = row.child(render_sound_cell(theme, sound, previewing, entity)),
        None => row = row.child(div().flex_1()),
    }
    row.into_any_element()
}

fn render_sound_cell(
    theme: &Theme,
    sound: &PickerSound,
    previewing: &Option<SharedString>,
    entity: &Entity<SoundPanel>,
) -> AnyElement {
    let is_previewing = previewing.as_ref() == Some(&sound.url);
    let preview_url = sound.url.clone();
    let send_url = sound.url.clone();
    let send_name = sound.name.clone();
    let ent = entity.clone();
    let green: gpui::Hsla = gpui::rgb(0x22c55e).into();
    let preview_icon = if is_previewing {
        IconName::PauseIcon
    } else {
        IconName::Speaker
    };
    let preview_color = if is_previewing {
        green
    } else {
        theme.tokens.text_secondary.into()
    };
    div()
        .id(sound.row_id.clone())
        .flex_1()
        .min_w_0()
        .h(px(40.))
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .p_2()
        .rounded_lg()
        .bg(theme.bg_secondary)
        .border_1()
        .border_color(gpui::transparent_black())
        .hover(|s| s.border_color(gpui::rgb(ACCENT_BLUE)))
        .child(
            div()
                .id(SharedString::from(format!("{}-preview", sound.row_id)))
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .size(px(32.))
                .rounded_full()
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover))
                .child(
                    Icon::new(preview_icon)
                        .size(px(16.))
                        .text_color(preview_color),
                )
                .on_click(move |_, _, cx| {
                    let url = preview_url.to_string();
                    VoiceStore::global(cx)
                        .update(cx, |store, cx| store.toggle_sound_preview(url, cx));
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.))
                .font_weight(FontWeight::BOLD)
                .text_color(theme.tokens.text_theme_primary)
                .child(sound.name.clone()),
        )
        .child(
            div()
                .id(SharedString::from(format!("{}-send", sound.row_id)))
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(
                    Icon::new(IconName::ResendMessageRightClick)
                        .size(px(20.))
                        .text_color(theme.tokens.text_theme_primary),
                )
                .on_click(move |_, _, cx| {
                    let url = send_url.to_string();
                    let filename = send_name.to_string();
                    ent.update(cx, |_this, cx| {
                        cx.emit(SoundPanelEvent::Picked { url, filename });
                    });
                }),
        )
        .into_any_element()
}
