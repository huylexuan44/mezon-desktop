use std::rc::Rc;

use gpui::{App, ElementId, SharedString, Window, deferred, div, prelude::*, px};

use super::icon::{Icon, IconName};
use super::stack::{h_flex, v_flex};
use crate::theme::ActiveTheme;

type ToggleHandler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;
type SelectHandler = Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum DropdownTriggerStyle {
    #[default]
    Default,
    InputPrimary,
}

#[derive(IntoElement)]
pub struct Dropdown {
    id: ElementId,
    items: Vec<SharedString>,
    selected: Option<usize>,
    open: bool,
    placeholder: SharedString,
    trigger_style: DropdownTriggerStyle,
    on_toggle: Option<ToggleHandler>,
    on_select: Option<SelectHandler>,
}

impl Dropdown {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            selected: None,
            open: false,
            placeholder: "Select…".into(),
            trigger_style: DropdownTriggerStyle::Default,
            on_toggle: None,
            on_select: None,
        }
    }

    pub fn trigger_style(mut self, style: DropdownTriggerStyle) -> Self {
        self.trigger_style = style;
        self
    }

    pub fn items(mut self, items: Vec<SharedString>) -> Self {
        self.items = items;
        self
    }

    pub fn selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn on_toggle(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }

    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Dropdown {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let label = self
            .selected
            .and_then(|i| self.items.get(i).cloned())
            .unwrap_or_else(|| self.placeholder.clone());
        let toggle = self.on_toggle.clone();
        let on_select = self.on_select.clone();
        let open = self.open;
        let trigger_style = self.trigger_style;

        let mut trigger = h_flex()
            .id(self.id)
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .text_sm()
            .text_color(theme.text_primary)
            .cursor_pointer()
            .child(label)
            .child(
                Icon::new(IconName::ArrowDown)
                    .size_4()
                    .text_color(theme.text_muted),
            );
        match trigger_style {
            DropdownTriggerStyle::Default => {
                trigger = trigger.px(px(10.)).py(px(6.)).bg(theme.bg_tertiary);
            }
            DropdownTriggerStyle::InputPrimary => {
                trigger = trigger
                    .h(px(40.0))
                    .px(px(12.0))
                    .bg(theme.tokens.bg_theme_input_primary);
            }
        }
        let trigger = trigger.when_some(toggle.clone(), |el, handler| {
            el.on_click(move |_, window, cx| handler(window, cx))
        });

        div().relative().w_full().child(trigger).when(open, |this| {
            let toggle = toggle.clone();
            let on_select = on_select.clone();
            this.child(deferred(
                v_flex()
                    .absolute()
                    .top_full()
                    .left_0()
                    .right_0()
                    .mt(px(4.))
                    .p(px(4.))
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.bg_floating)
                    .shadow_lg()
                    .occlude()
                    .when_some(toggle, |el, handler| {
                        el.on_mouse_down_out(move |_, window, cx| handler(window, cx))
                    })
                    .children(self.items.into_iter().enumerate().map(|(index, item)| {
                        let selected = self.selected == Some(index);
                        let on_select = on_select.clone();
                        h_flex()
                            .id(("dropdown-item", index))
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px(px(8.))
                            .py(px(6.))
                            .rounded(px(4.))
                            .text_sm()
                            .text_color(theme.text_primary)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.bg_hover))
                            .child(item)
                            .when(selected, |el| {
                                el.child(
                                    Icon::new(IconName::Check).size_4().text_color(theme.brand),
                                )
                            })
                            .when_some(on_select, |el, handler| {
                                el.on_click(move |_, window, cx| handler(index, window, cx))
                            })
                    })),
            ))
        })
    }
}
