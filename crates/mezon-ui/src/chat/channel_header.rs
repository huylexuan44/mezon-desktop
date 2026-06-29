use std::sync::Arc;

use gpui::{
    Anchor, App, Context, Entity, Render, SharedString, Subscription, WeakEntity, Window, div,
    prelude::*, px,
};
use mezon_store::Settings;
use ui::{ButtonLike, PopoverMenu, PopoverMenuHandle, Toggleable};

use crate::app::window_controls;
use crate::chat::inbox::{InboxPopoverPanel, clan_has_inbox_badge};
use crate::components::primitives::{Icon, IconName};
use crate::theme::{ActiveTheme, Theme};

type ToggleHandler = Arc<dyn Fn(&mut Window, &mut App)>;

pub struct ChannelHeader {
    name: String,
    dm: bool,
    members_action: bool,
    members_active: bool,
    on_toggle_members: Option<ToggleHandler>,
    show_inbox: bool,
    inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
    clan_id: Option<String>,
    locale: Option<String>,
}

impl ChannelHeader {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            dm: false,
            members_action: true,
            members_active: false,
            on_toggle_members: None,
            show_inbox: true,
            inbox_handle: None,
            clan_id: None,
            locale: None,
        }
    }

    pub fn dm(mut self, dm: bool) -> Self {
        self.dm = dm;
        self
    }

    pub fn members_action(mut self, show: bool) -> Self {
        self.members_action = show;
        self
    }

    pub fn members_active(mut self, active: bool) -> Self {
        self.members_active = active;
        self
    }

    pub fn on_toggle_members(mut self, handler: ToggleHandler) -> Self {
        self.on_toggle_members = Some(handler);
        self
    }

    pub fn show_inbox(mut self, show: bool) -> Self {
        self.show_inbox = show;
        self
    }

    pub fn inbox_popover(mut self, handle: PopoverMenuHandle<InboxPopoverPanel>) -> Self {
        self.inbox_handle = Some(handle);
        self
    }

    pub fn inbox_context(mut self, clan_id: impl Into<String>, locale: impl Into<String>) -> Self {
        self.clan_id = Some(clan_id.into());
        self.locale = Some(locale.into());
        self
    }

    pub fn render(&self, theme: &Theme, window: &mut Window, cx: &App) -> impl IntoElement {
        let bg_hover = theme.bg_hover;
        let bg_active = theme.bg_tertiary;
        let icon_color = theme.text_muted;
        let icon_active = theme.text_primary;
        let actions = [
            ("hdr-canvas", IconName::CanvasIcon),
            ("hdr-timeline", IconName::History),
            ("hdr-thread", IconName::ThreadIcon),
            ("hdr-members", IconName::MemberList),
            ("hdr-pin", IconName::PinRight),
            ("hdr-bell", IconName::Bell),
            ("hdr-gallery", IconName::ImageThumbnail),
            ("hdr-files", IconName::FileIcon),
        ];

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .w_full()
            .h(px(window_controls::APP_HEADER_HEIGHT))
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.bg_primary)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .when(!self.dm, |this| {
                        this.child(
                            Icon::new(IconName::Hashtag)
                                .size(px(20.0))
                                .text_color(theme.text_muted),
                        )
                    })
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(self.name.clone()),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .children(
                        actions
                            .into_iter()
                            .filter(|(id, _)| *id != "hdr-members" || self.members_action)
                            .map(|(id, icon)| {
                                let is_members = id == "hdr-members";
                                let active = is_members && self.members_active;
                                let tint = if active { icon_active } else { icon_color };
                                let mut button = div()
                                    .id(id)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .w(px(32.))
                                    .h(px(32.))
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(bg_hover))
                                    .occlude()
                                    .child(Icon::new(icon).size(px(20.)).text_color(tint));
                                if active {
                                    button = button.bg(bg_active);
                                }
                                if is_members && let Some(handler) = self.on_toggle_members.clone()
                                {
                                    button =
                                        button.on_click(move |_, window, cx| handler(window, cx));
                                }
                                button
                            }),
                    )
                    .when(self.show_inbox && !self.dm, |row| {
                        row.child(self.render_inbox_button(theme, window, cx))
                    }),
            )
    }

    fn render_inbox_button(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &App,
    ) -> gpui::AnyElement {
        let _ = window;
        let Some(handle) = self.inbox_handle.clone() else {
            return div()
                .id("hdr-inbox")
                .flex()
                .items_center()
                .justify_center()
                .w(px(32.))
                .h(px(32.))
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover))
                .child(
                    Icon::new(IconName::Inbox)
                        .size(px(20.))
                        .text_color(theme.text_muted),
                )
                .into_any_element();
        };

        let show_badge = self
            .clan_id
            .as_deref()
            .is_some_and(|id| clan_has_inbox_badge(id, cx));
        let clan_id = self.clan_id.clone().unwrap_or_default();
        let locale = self.locale.clone().unwrap_or_else(|| "en".to_string());
        let mention_badge = theme.mention_badge;
        let is_open = handle.is_deployed();

        PopoverMenu::new("hdr-inbox-popover")
            .with_handle(handle.clone())
            .anchor(Anchor::TopRight)
            .attach(Anchor::BottomRight)
            .offset(gpui::point(px(0.), px(8.)))
            .menu({
                let handle = handle.clone();
                let clan_id = clan_id.clone();
                let locale = locale.clone();
                move |window, cx| {
                    Some(cx.new(|cx| {
                        InboxPopoverPanel::new(
                            clan_id.clone(),
                            locale.clone(),
                            handle.clone(),
                            window,
                            cx,
                        )
                    }))
                }
            })
            .trigger(
                ButtonLike::new("hdr-inbox-btn")
                    .toggle_state(is_open)
                    .child(
                        div()
                            .relative()
                            .child(Icon::new(IconName::Inbox).size(px(20.)).text_color(
                                if is_open {
                                    theme.interactive_active
                                } else {
                                    theme.text_muted
                                },
                            ))
                            .when(show_badge, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(0.))
                                        .right(px(0.))
                                        .w(px(8.))
                                        .h(px(8.))
                                        .rounded_full()
                                        .bg(mention_badge),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }
}

pub struct ChatHeader {
    name: SharedString,
    dm: bool,
    members_action: bool,
    members_active: bool,
    show_inbox: bool,
    inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
    clan_id: Option<String>,
    locale: Option<String>,
    layout: WeakEntity<crate::ChatLayout>,
    settings: Entity<Settings>,
    _settings_observe: Subscription,
}

impl ChatHeader {
    pub fn new(
        layout: WeakEntity<crate::ChatLayout>,
        settings: &Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _settings_observe = cx.observe(settings, |_, _, cx| cx.notify());
        Self {
            name: SharedString::default(),
            dm: false,
            members_action: true,
            members_active: false,
            show_inbox: true,
            inbox_handle: None,
            clan_id: None,
            locale: None,
            layout,
            settings: settings.clone(),
            _settings_observe,
        }
    }

    pub fn sync(
        &mut self,
        name: Option<SharedString>,
        dm: bool,
        members_action: bool,
        members_active: bool,
        show_inbox: bool,
        inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
        clan_id: Option<String>,
        locale: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let name = name.unwrap_or_else(|| self.name.clone());
        if self.name == name
            && self.dm == dm
            && self.members_action == members_action
            && self.members_active == members_active
            && self.show_inbox == show_inbox
            && self.clan_id == clan_id
            && self.locale == locale
        {
            return;
        }
        self.name = name;
        self.dm = dm;
        self.members_action = members_action;
        self.members_active = members_active;
        self.show_inbox = show_inbox;
        self.inbox_handle = inbox_handle;
        self.clan_id = clan_id;
        self.locale = locale;
        cx.notify();
    }
}

impl Render for ChatHeader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let layout = self.layout.clone();
        let mut header = ChannelHeader::new(self.name.to_string())
            .dm(self.dm)
            .members_action(self.members_action)
            .members_active(self.members_active)
            .show_inbox(self.show_inbox)
            .on_toggle_members(Arc::new(move |_window: &mut Window, cx: &mut App| {
                let _ = layout.update(cx, |this, cx| this.toggle_member_list(cx));
            }));
        if let (Some(handle), Some(clan_id), Some(locale)) = (
            self.inbox_handle.clone(),
            self.clan_id.clone(),
            self.locale.clone(),
        ) {
            header = header.inbox_popover(handle).inbox_context(clan_id, locale);
        }
        header.render(theme, window, cx).into_any_element()
    }
}
