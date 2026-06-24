use gpui::{
    Anchor, App, ClickEvent, CursorStyle, Entity, Hsla, IntoElement, RenderOnce, Window, div,
    point, prelude::*, px,
};
use ui::prelude::*;
use ui::{PopoverMenu, PopoverMenuHandle};

use crate::chat::layout::ChatLayout;
use crate::chat::threads_popover::{ThreadsPopoverPanel, thread_popover_on_open};
use crate::components::primitives::{
    Button, ButtonVariant, ButtonVariants, Icon, IconName, Sizable, Size,
};
use crate::theme::Theme;

type ThreadTriggerClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

pub struct ChannelHeader {
    name: String,
    dm: bool,
    show_threads: bool,
    layout: Option<Entity<ChatLayout>>,
    thread_handle: Option<PopoverMenuHandle<ThreadsPopoverPanel>>,
}

impl ChannelHeader {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            dm: false,
            show_threads: false,
            layout: None,
            thread_handle: None,
        }
    }

    pub fn dm(mut self, dm: bool) -> Self {
        self.dm = dm;
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

    pub fn thread_popover(
        mut self,
        handle: PopoverMenuHandle<ThreadsPopoverPanel>,
    ) -> Self {
        self.thread_handle = Some(handle);
        self
    }

    pub fn render(self, theme: &Theme, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let bg_hover = theme.bg_hover;
        let icon_color = theme.text_muted;
        let layout = self.layout;
        let thread_handle = self.thread_handle;
        let show_threads = self.show_threads;
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
            .h(px(50.))
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
            .child(div().flex().flex_row().items_center().gap_1().children(
                actions.into_iter().filter_map(move |(id, icon)| {
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

                    Some(
                        div()
                            .id(id)
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(32.))
                            .h(px(32.))
                            .rounded_md()
                            .cursor_pointer()
                            .hover(move |s| s.bg(bg_hover))
                            .child(Icon::new(icon).size(px(20.)).text_color(icon_color))
                            .into_any_element(),
                    )
                }),
            ))
    }
}

#[derive(IntoElement)]
struct ThreadPopoverTrigger {
    open: bool,
    icon_color: Hsla,
    on_click: Option<ThreadTriggerClickHandler>,
}

impl ThreadPopoverTrigger {
    fn new(theme: &Theme, open: bool) -> Self {
        Self {
            open,
            icon_color: theme.text_muted.into(),
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
        let mut button = Button::new("hdr-thread-trigger")
            .with_size(Size::Small)
            .icon(
                Icon::new(IconName::ThreadIcon)
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
