use std::rc::Rc;

use gpui::{
    App, ClickEvent, MouseDownEvent, Pixels, Point, SharedString, Window, anchored, deferred, div,
    prelude::*, px,
};

use super::icon::{Icon, IconName};
use super::stack::{h_flex, v_flex};
use crate::theme::ActiveTheme;

type MenuHandler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;
type DismissHandler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

enum Item {
    Entry {
        label: SharedString,
        icon: Option<IconName>,
        danger: bool,
        on_click: MenuHandler,
    },
    Separator,
}

#[derive(IntoElement, Default)]
pub struct ContextMenu {
    items: Vec<Item>,
    on_dismiss: Option<DismissHandler>,
}

impl ContextMenu {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn item(
        mut self,
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(Item::Entry {
            label: label.into(),
            icon: None,
            danger: false,
            on_click: Rc::new(on_click),
        });
        self
    }

    pub fn item_icon(
        mut self,
        label: impl Into<SharedString>,
        icon: IconName,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(Item::Entry {
            label: label.into(),
            icon: Some(icon),
            danger: false,
            on_click: Rc::new(on_click),
        });
        self
    }

    pub fn danger_item(
        mut self,
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(Item::Entry {
            label: label.into(),
            icon: None,
            danger: true,
            on_click: Rc::new(on_click),
        });
        self
    }

    pub fn danger_item_icon(
        mut self,
        label: impl Into<SharedString>,
        icon: IconName,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(Item::Entry {
            label: label.into(),
            icon: Some(icon),
            danger: true,
            on_click: Rc::new(on_click),
        });
        self
    }

    pub fn separator(mut self) -> Self {
        self.items.push(Item::Separator);
        self
    }

    pub fn on_dismiss(mut self, on_dismiss: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let bg = theme.tokens.bg_theme_contexify;
        let border = theme.border;
        let text = theme.text_primary;
        let muted = theme.text_secondary;
        let hover = theme.bg_hover;
        let danger = theme.status_dnd;
        let dismiss = self.on_dismiss.clone();

        let mut panel = v_flex()
            .min_w(px(220.))
            .p(px(6.))
            .rounded_md()
            .border_1()
            .border_color(border)
            .bg(bg)
            .shadow_lg()
            .occlude();

        if let Some(dismiss) = dismiss.clone() {
            panel = panel.on_mouse_down_out(move |_: &MouseDownEvent, window, cx| {
                dismiss(window, cx);
            });
        }

        for (index, item) in self.items.into_iter().enumerate() {
            match item {
                Item::Separator => {
                    panel = panel.child(div().my(px(5.)).h(px(1.)).w_full().bg(border));
                }
                Item::Entry {
                    label,
                    icon,
                    danger: is_danger,
                    on_click,
                } => {
                    let dismiss = dismiss.clone();
                    let label_color = if is_danger { danger } else { text };
                    let icon_color = if is_danger { danger } else { muted };
                    panel = panel.child(
                        h_flex()
                            .id(("context-menu-item", index))
                            .w_full()
                            .gap_2()
                            .items_center()
                            .px(px(10.))
                            .py(px(8.))
                            .rounded(px(4.))
                            .text_sm()
                            .text_color(label_color)
                            .cursor_pointer()
                            .hover(|s| s.bg(hover))
                            .when_some(icon, |row, icon| {
                                row.child(Icon::new(icon).size_4().text_color(icon_color))
                            })
                            .child(label)
                            .on_click(move |_: &ClickEvent, window, cx| {
                                on_click(window, cx);
                                if let Some(dismiss) = &dismiss {
                                    dismiss(window, cx);
                                }
                            }),
                    );
                }
            }
        }

        panel
    }
}

pub fn context_menu_at(position: Point<Pixels>, menu: ContextMenu) -> impl IntoElement {
    deferred(anchored().position(position).snap_to_window().child(menu))
}
