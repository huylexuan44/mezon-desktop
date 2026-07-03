use std::sync::Arc;

use gpui::{
    Anchor, App, ClickEvent, Context, CursorStyle, Entity, Hsla, IntoElement, Render, RenderOnce,
    SharedString, Subscription, WeakEntity, Window, div, point, prelude::*, px,
};
use mezon_store::{Settings, ThreadsStore};
use ui::{Clickable, PopoverMenu, PopoverMenuHandle, Toggleable};

use crate::app::window_controls;
use crate::chat::layout::ChatLayout;
use crate::chat::pinned_popover::{PinnedPopoverPanel, pin_popover_on_open};
use crate::chat::threads_popover::{ThreadsPopoverPanel, thread_popover_on_open};
use crate::components::primitives::{
    Button, ButtonVariant, ButtonVariants, Icon, IconName, Sizable, Size,
};
use crate::theme::{ActiveTheme, Theme};

type ToggleHandler = Arc<dyn Fn(&mut Window, &mut App)>;
type ThreadTriggerClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

pub struct ChannelHeader {
    name: String,
    dm: bool,
    members_action: bool,
    members_active: bool,
    on_toggle_members: Option<ToggleHandler>,
    show_threads: bool,
    layout: Option<Entity<ChatLayout>>,
    thread_handle: Option<PopoverMenuHandle<ThreadsPopoverPanel>>,
    pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
    settings: Option<Entity<Settings>>,
}

impl ChannelHeader {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            dm: false,
            members_action: true,
            members_active: false,
            on_toggle_members: None,
            show_threads: false,
            layout: None,
            thread_handle: None,
            pin_handle: None,
            settings: None,
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

    pub fn layout(mut self, layout: Entity<ChatLayout>) -> Self {
        self.layout = Some(layout);
        self
    }

    pub fn show_threads(mut self, show: bool) -> Self {
        self.show_threads = show;
        self
    }

    pub fn thread_popover(mut self, handle: PopoverMenuHandle<ThreadsPopoverPanel>) -> Self {
        self.thread_handle = Some(handle);
        self
    }

    pub fn pin_popover(
        mut self,
        handle: PopoverMenuHandle<PinnedPopoverPanel>,
        settings: Entity<Settings>,
    ) -> Self {
        self.pin_handle = Some(handle);
        self.settings = Some(settings);
        self
    }

    pub fn render(&self, theme: &Theme) -> impl IntoElement {
        let bg_hover = theme.bg_hover;
        let bg_active = theme.bg_tertiary;
        let icon_color = theme.text_muted;
        let icon_active = theme.text_primary;
        let layout = self.layout.clone();
        let thread_handle = self.thread_handle.clone();
        let show_threads = self.show_threads;
        let pin_handle = self.pin_handle.clone();
        let settings = self.settings.clone();
        let actions = [
            ("hdr-canvas", IconName::CanvasIcon),
            ("hdr-timeline", IconName::History),
            ("hdr-thread", IconName::ThreadIcon),
            ("hdr-members", IconName::MemberList),
            ("hdr-pin", IconName::PinRight),
            ("hdr-bell", IconName::Bell),
            ("hdr-gallery", IconName::ImageThumbnail),
            ("hdr-files", IconName::FileIcon),
            ("hdr-inbox", IconName::Inbox),
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
                            .text_base()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(self.name.clone()),
                    ),
            )
            .child(div().flex_1())
            .child(div().flex().flex_row().items_center().gap_1().children(
                actions.into_iter().filter_map(move |(id, icon)| {
                    if id == "hdr-members" && !self.members_action {
                        return None;
                    }
                    if id == "hdr-thread" {
                        if !show_threads {
                            return None;
                        }
                        let (Some(handle), Some(layout)) =
                            (thread_handle.clone(), layout.clone())
                        else {
                            return None;
                        };
                        let menu_handle = handle.clone();
                        return Some(
                            PopoverMenu::new("hdr-thread-popover")
                                .with_handle(handle)
                                .anchor(Anchor::TopRight)
                                .attach(Anchor::BottomRight)
                                .offset(point(px(0.), px(9.)))
                                .on_open(thread_popover_on_open(layout.clone()))
                                .menu({
                                    let layout = layout.clone();
                                    move |window, cx| {
                                        layout.update(cx, |layout, cx| {
                                            layout.ensure_thread_search_input(window, cx);
                                        });
                                        let search_input =
                                            layout.read(cx).thread_search_input.clone()?;
                                        Some(cx.new(|cx| {
                                            ThreadsPopoverPanel::new(
                                                layout.clone(),
                                                search_input,
                                                menu_handle.clone(),
                                                window,
                                                cx,
                                            )
                                        }))
                                    }
                                })
                                .trigger(ThreadPopoverTrigger::new(theme, false))
                                .into_any_element(),
                        );
                    }
                    if id == "hdr-pin"
                        && let (Some(handle), Some(settings)) =
                            (pin_handle.clone(), settings.clone())
                    {
                        let menu_handle = handle.clone();
                        return Some(
                            PopoverMenu::new("hdr-pin-popover")
                                .with_handle(handle)
                                .anchor(Anchor::TopRight)
                                .attach(Anchor::BottomRight)
                                .offset(point(px(0.), px(9.)))
                                .on_open(pin_popover_on_open())
                                .menu({
                                    let settings = settings.clone();
                                    move |window, cx| {
                                        Some(cx.new(|cx| {
                                            PinnedPopoverPanel::new(
                                                settings.clone(),
                                                menu_handle.clone(),
                                                window,
                                                cx,
                                            )
                                        }))
                                    }
                                })
                                .trigger(PinPopoverTrigger::new(theme, false))
                                .into_any_element(),
                        );
                    }

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
                    if is_members && let Some(handler) = self.on_toggle_members.clone() {
                        button = button.on_click(move |_, window, cx| handler(window, cx));
                    }
                    Some(button.into_any_element())
                }),
            ))
    }
}

pub struct ChatHeader {
    name: SharedString,
    dm: bool,
    members_action: bool,
    members_active: bool,
    pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
    layout: WeakEntity<ChatLayout>,
    settings: Entity<Settings>,
    _settings_observe: Subscription,
}

impl ChatHeader {
    pub fn new(
        layout: WeakEntity<ChatLayout>,
        settings: &Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _settings_observe = cx.observe(settings, |_, _, cx| cx.notify());
        Self {
            name: SharedString::default(),
            dm: false,
            members_action: true,
            members_active: false,
            pin_handle: None,
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
        pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
        cx: &mut Context<Self>,
    ) {
        let name = name.unwrap_or_else(|| self.name.clone());
        self.pin_handle = pin_handle;
        if self.name == name
            && self.dm == dm
            && self.members_action == members_action
            && self.members_active == members_active
        {
            return;
        }
        self.name = name;
        self.dm = dm;
        self.members_action = members_action;
        self.members_active = members_active;
        cx.notify();
    }
}

impl Render for ChatHeader {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let layout_weak = self.layout.clone();
        let settings = self.settings.clone();
        let show_threads = ThreadsStore::global(cx).read(cx).show_threads_popover(cx);
        let members_toggle = Arc::new(move |_window: &mut Window, cx: &mut App| {
            let _ = layout_weak.update(cx, |this, cx| this.toggle_member_list(cx));
        });
        let mut header = ChannelHeader::new(self.name.to_string())
            .dm(self.dm)
            .members_action(self.members_action)
            .members_active(self.members_active)
            .on_toggle_members(members_toggle)
            .show_threads(show_threads);
        if show_threads {
            if let Ok(thread_handle) = self
                .layout
                .read_with(cx, |layout, _| layout.thread_popover_handle.clone())
            {
                if let Some(layout) = self.layout.upgrade() {
                    header = header
                        .layout(layout)
                        .thread_popover(thread_handle);
                }
            }
        }
        if let Some(handle) = self.pin_handle.clone() {
            header = header.pin_popover(handle, settings);
        }
        header.render(theme).into_any_element()
    }
}

#[derive(IntoElement)]
struct ThreadPopoverTrigger {
    open: bool,
    icon_color: gpui::Rgba,
    icon_active: gpui::Rgba,
    bg_hover: gpui::Rgba,
    bg_active: gpui::Rgba,
    on_click: Option<ThreadTriggerClickHandler>,
}

impl ThreadPopoverTrigger {
    fn new(theme: &Theme, open: bool) -> Self {
        Self {
            open,
            icon_color: theme.text_muted,
            icon_active: theme.text_primary,
            bg_hover: theme.bg_hover,
            bg_active: theme.bg_tertiary,
            on_click: None,
        }
    }
}

impl Toggleable for ThreadPopoverTrigger {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }
}

impl Clickable for ThreadPopoverTrigger {
    fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    fn cursor_style(self, _cursor_style: CursorStyle) -> Self {
        self
    }
}

impl RenderOnce for ThreadPopoverTrigger {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let tint = if self.open {
            self.icon_active
        } else {
            self.icon_color
        };
        let bg_hover = self.bg_hover;
        let mut button = div()
            .id("hdr-thread-trigger")
            .flex()
            .items_center()
            .justify_center()
            .w(px(32.))
            .h(px(32.))
            .rounded_md()
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .occlude()
            .child(
                Icon::new(IconName::ThreadIcon)
                    .size(px(20.))
                    .text_color(tint),
            );
        if self.open {
            button = button.bg(self.bg_active);
        }
        if let Some(handler) = self.on_click {
            button.on_click(handler)
        } else {
            button
        }
    }
}

#[derive(IntoElement)]
struct PinPopoverTrigger {
    open: bool,
    icon_color: Hsla,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl PinPopoverTrigger {
    fn new(theme: &Theme, open: bool) -> Self {
        Self {
            open,
            icon_color: theme.text_muted.into(),
            on_click: None,
        }
    }
}

impl Toggleable for PinPopoverTrigger {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }
}

impl Clickable for PinPopoverTrigger {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    fn cursor_style(self, _cursor_style: CursorStyle) -> Self {
        self
    }
}

impl RenderOnce for PinPopoverTrigger {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut button = Button::new("hdr-pin-trigger").with_size(Size::Small).icon(
            Icon::new(IconName::PinRight)
                .size(px(20.))
                .text_color(self.icon_color),
        );
        button = if self.open {
            button.with_variant(ButtonVariant::Secondary)
        } else {
            button.ghost()
        };
        if let Some(handler) = self.on_click {
            button.on_click(handler)
        } else {
            button
        }
    }
}
