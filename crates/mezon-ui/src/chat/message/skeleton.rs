use gpui::{AnyElement, div, prelude::*, px, relative};

use crate::theme::Theme;

const SKELETON_ITEMS: [([f32; 5], [f32; 5], f32); 5] = [
    ([75., 68., 82., 91., 77.], [88., 71., 94., 83., 69.], 180.),
    ([82., 95., 73., 87., 91.], [76., 89., 84., 78., 93.], 220.),
    ([68., 84., 92., 77., 85.], [91., 73., 88., 95., 81.], 150.),
    ([91., 72., 86., 94., 79.], [84., 97., 76., 89., 85.], 200.),
    ([77., 89., 81., 93., 88.], [79., 92., 87., 74., 96.], 170.),
];

pub fn message_skeleton(theme: &Theme, rows: usize) -> AnyElement {
    let fill = theme.bg_hover;
    let rows = rows.min(SKELETON_ITEMS.len());

    let bar = move |height: f32, width: gpui::DefiniteLength| {
        div().h(px(height)).w(width).rounded(px(4.)).bg(fill)
    };

    let word_line = move |widths: [f32; 5], top_pad: bool| {
        let sum: f32 = widths.iter().sum();
        let mut line = div().flex().flex_row().gap_2().w_full();
        if top_pad {
            line = line.pt_2();
        }
        for w in widths {
            line = line.child(bar(16., relative(w / sum * 0.9)));
        }
        line
    };

    let mut container = div()
        .flex()
        .flex_col()
        .px_4()
        .py_2()
        .w(relative(0.6))
        .overflow_hidden();
    for (line1, line2, image_w) in SKELETON_ITEMS.iter().take(rows).copied() {
        container = container.child(
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap_3()
                .pb_4()
                .child(div().size(px(40.)).rounded_full().bg(fill).flex_none())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .py_2()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .pb_2()
                                .child(bar(16., px(96.).into()))
                                .child(bar(12., px(64.).into())),
                        )
                        .child(word_line(line1, false))
                        .child(word_line(line2, true))
                        .child(
                            div()
                                .mt_2()
                                .w(px(image_w))
                                .max_w(relative(1.0))
                                .h(px(120.))
                                .rounded_md()
                                .bg(fill),
                        ),
                ),
        );
    }
    container.into_any_element()
}
