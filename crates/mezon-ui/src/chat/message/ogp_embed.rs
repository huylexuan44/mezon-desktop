use gpui::{AnyElement, ObjectFit, SharedString, div, img, prelude::*, px, rgb};
use mezon_store::Message;

use super::content::open_message_link;
use super::context::RowCtx;
use crate::components::primitives::{Icon, IconName};

const OGP_TITLE_COLOR: u32 = 0x3b82f6;
const OGP_TITLE_HOVER_COLOR: u32 = 0x60a5fa;

pub fn render_ogp_embed(msg: &Message, ctx: &RowCtx) -> Option<AnyElement> {
    let ogp = msg.ogp.as_ref()?;
    let theme = ctx.theme;
    let url = ogp.url.clone();
    let has_text = !ogp.title.is_empty() || !ogp.description.is_empty();

    let text_block = has_text.then(|| {
        let mut block = div().flex().flex_col().gap_0p5();
        if !ogp.title.is_empty() {
            block = block.child(
                div()
                    .id("ogp-title")
                    .text_size(px(14.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(OGP_TITLE_COLOR))
                    .hover(|s| s.text_color(rgb(OGP_TITLE_HOVER_COLOR)))
                    .child(ogp.title.clone()),
            );
        }
        if !ogp.description.is_empty() {
            block = block.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.tokens.text_theme_primary)
                    .opacity(0.9)
                    .child(ogp.description.clone()),
            );
        }
        block
    });

    let image_box = div()
        .w_full()
        .h(px(200.))
        .flex_shrink_0()
        .mt_1()
        .rounded(px(4.))
        .overflow_hidden()
        .bg(theme.tokens.theme_setting_primary)
        .child(ogp_image(ogp.image_proxied.clone(), theme.text_muted));

    Some(
        div()
            .id(SharedString::from(format!(
                "msg-ogp-{}",
                msg.row_anchor_id.0
            )))
            .relative()
            .max_w(px(350.))
            .mt_1()
            .mb_1()
            .rounded(px(8.))
            .bg(theme.tokens.theme_setting_nav)
            .cursor_pointer()
            .on_click(move |_, _, cx| open_message_link(url.clone(), cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(10.))
                    .when_some(text_block, |d, block| d.child(block))
                    .child(image_box),
            )
            .into_any_element(),
    )
}

fn ogp_image(src: SharedString, fallback_fg: gpui::Rgba) -> AnyElement {
    if src.is_empty() {
        return ogp_image_fallback(fallback_fg);
    }
    img(src)
        .w_full()
        .max_h(px(200.))
        .object_fit(ObjectFit::Contain)
        .with_fallback(move || ogp_image_fallback(fallback_fg))
        .into_any_element()
}

fn ogp_image_fallback(fallback_fg: gpui::Rgba) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .opacity(0.3)
        .child(
            Icon::new(IconName::ImageThumbnail)
                .size(px(32.))
                .text_color(fallback_fg),
        )
        .into_any_element()
}
