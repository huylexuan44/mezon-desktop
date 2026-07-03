use std::ops::Range;

use gpui::{
    Anchor, AnyElement, App, FontWeight, HighlightStyle, Hsla, InteractiveText, ObjectFit,
    SharedString, StyledText, UnderlineStyle, div, img, prelude::*, px, rems,
};
use mezon_store::{
    ChannelId, ChannelList, Message, MessageSpan, PlatformStore, UserId, is_here_user_id,
};

use super::context::RowCtx;
use crate::chat::user_profile_popover::{
    ClickableContainer, UserProfilePopover, profile_popover_menu,
};
use crate::router::{Route, navigate};
use crate::theme::Theme;

struct ContentRenderOptions {
    body_color: gpui::Rgba,
    mentions_only: bool,
    inline: bool,
}

pub fn render_message_content(msg: &Message, ctx: &RowCtx) -> AnyElement {
    render_message_content_with_options(
        msg,
        ctx,
        ContentRenderOptions {
            body_color: ctx.theme.tokens.text_theme_message,
            mentions_only: false,
            inline: false,
        },
    )
}

pub fn render_system_message_content(
    msg: &Message,
    ctx: &RowCtx,
    mentions_only: bool,
) -> AnyElement {
    render_message_content_with_options(
        msg,
        ctx,
        ContentRenderOptions {
            body_color: ctx.theme.tokens.text_theme_primary,
            mentions_only,
            inline: mentions_only,
        },
    )
}

fn render_message_content_with_options(
    msg: &Message,
    ctx: &RowCtx,
    options: ContentRenderOptions,
) -> AnyElement {
    if msg.spans.is_empty() && !msg.is_edited {
        return div().into_any_element();
    }
    let theme = ctx.theme;
    let body_color = options.body_color;

    if options.mentions_only {
        return render_mention_only_content(msg, ctx, body_color, options.inline);
    }

    let has_tokens = msg.spans.iter().any(|s| !matches!(s, MessageSpan::Text(_)));

    if !has_tokens && !msg.is_edited {
        return render_plain_text_spans(&msg.spans, body_color);
    }

    if is_link_only(&msg.spans) && !msg.is_edited {
        return render_link_only_spans(&msg.spans, theme);
    }

    let has_code_block = msg
        .spans
        .iter()
        .any(|s| matches!(s, MessageSpan::CodeBlock { .. }));
    let has_custom_emoji = msg
        .spans
        .iter()
        .any(|s| matches!(s, MessageSpan::Emoji { emoji_id, .. } if !emoji_id.is_empty()));
    if !options.inline && !has_code_block && !has_custom_emoji {
        return render_rich_styled(msg, ctx, body_color);
    }

    let mut row = rich_content_row(body_color, options.inline);
    for span in &msg.spans {
        row = append_span(row, span, ctx, body_color);
    }
    if msg.is_edited {
        row = row.child(edited_marker(theme, ctx.locale));
    }
    row.into_any_element()
}

#[derive(Clone)]
enum SpanAction {
    Mention(UserId),
    Channel(ChannelId),
    Link(SharedString),
}

fn render_rich_styled(msg: &Message, ctx: &RowCtx, body_color: gpui::Rgba) -> AnyElement {
    let theme = ctx.theme;
    let mention_color: Hsla = theme.tokens.mention_color.into();
    let mention_bg: Hsla = theme.tokens.mention_primary.into();
    let code_bg: Hsla = theme.tokens.bg_markdown_code.into();
    let link_color: Hsla = theme.tokens.mention_color.into();

    let mut text = String::new();
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut font_overrides: Vec<(Range<usize>, SharedString)> = Vec::new();
    let mut click_ranges: Vec<Range<usize>> = Vec::new();
    let mut actions: Vec<SpanAction> = Vec::new();

    for span in &msg.spans {
        match span {
            MessageSpan::Text(t) => text.push_str(t),
            MessageSpan::Emoji { name, .. } => text.push_str(name),
            MessageSpan::Bold(t) => {
                let start = text.len();
                text.push_str(t);
                highlights.push((
                    start..text.len(),
                    HighlightStyle {
                        font_weight: Some(FontWeight::BOLD),
                        ..Default::default()
                    },
                ));
            }
            MessageSpan::Code(t) => {
                let start = text.len();
                text.push_str(t);
                let range = start..text.len();
                highlights.push((
                    range.clone(),
                    HighlightStyle {
                        background_color: Some(code_bg),
                        ..Default::default()
                    },
                ));
                font_overrides.push((range, "monospace".into()));
            }
            MessageSpan::Link { text: t, url } => {
                let start = text.len();
                text.push_str(t);
                let range = start..text.len();
                highlights.push((
                    range.clone(),
                    HighlightStyle {
                        color: Some(link_color),
                        underline: Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(link_color),
                            wavy: false,
                        }),
                        ..Default::default()
                    },
                ));
                click_ranges.push(range);
                actions.push(SpanAction::Link(resolve_link_url(url, t).into()));
            }
            MessageSpan::Mention {
                display,
                user_id,
                role_id,
            } => {
                let start = text.len();
                text.push_str(display);
                let range = start..text.len();
                highlights.push((
                    range.clone(),
                    HighlightStyle {
                        color: Some(mention_color),
                        background_color: Some(mention_bg),
                        ..Default::default()
                    },
                ));
                if role_id.as_deref().is_none_or(str::is_empty)
                    && let Some(uid) = user_id
                        .as_deref()
                        .filter(|u| !u.is_empty() && *u != "0" && !is_here_user_id(u))
                        .and_then(|u| u.parse::<i64>().ok())
                        .map(UserId)
                {
                    click_ranges.push(range);
                    actions.push(SpanAction::Mention(uid));
                }
            }
            MessageSpan::Hashtag {
                display,
                channel_id,
            } => {
                let start = text.len();
                text.push_str(display);
                let range = start..text.len();
                highlights.push((
                    range.clone(),
                    HighlightStyle {
                        color: Some(mention_color),
                        background_color: Some(mention_bg),
                        ..Default::default()
                    },
                ));
                if let Some(channel_id) = channel_id.as_deref().and_then(parse_channel_id) {
                    click_ranges.push(range);
                    actions.push(SpanAction::Channel(channel_id));
                }
            }
            MessageSpan::CodeBlock { .. } => {}
        }
    }

    if msg.is_edited {
        text.push(' ');
        let start = text.len();
        text.push_str(mezon_i18n::t(ctx.locale, "message.edited"));
        highlights.push((
            start..text.len(),
            HighlightStyle {
                color: Some(theme.text_muted.into()),
                ..Default::default()
            },
        ));
    }

    let mut styled = StyledText::new(text).with_highlights(highlights);
    if !font_overrides.is_empty() {
        styled = styled.with_font_family_overrides(font_overrides);
    }

    let profile_context = ctx.profile_context;
    let settings = ctx.settings.clone();
    let host = ctx.video_host.clone();
    let interactive = InteractiveText::new(("msg-itext", msg.row_anchor_id.0 as usize), styled)
        .on_click(click_ranges, move |range_ix, window, cx| {
            let Some(action) = actions.get(range_ix) else {
                return;
            };
            match action {
                SpanAction::Link(url) => open_message_link(url.to_string(), cx),
                SpanAction::Channel(channel_id) => navigate_to_channel(*channel_id, cx),
                SpanAction::Mention(user_id) => {
                    let Some(context) = profile_context else {
                        return;
                    };
                    let position = window.mouse_position();
                    let popover = cx.new(|cx| {
                        UserProfilePopover::new(*user_id, context, settings.clone(), window, cx)
                    });
                    let _ = host.update(cx, move |this, cx| {
                        this.set_mention_popover(popover, position, cx);
                    });
                }
            }
        });

    div()
        .w_full()
        .min_w_0()
        .text_base()
        .line_height(rems(1.375))
        .text_color(body_color)
        .child(interactive)
        .into_any_element()
}

fn render_mention_only_content(
    msg: &Message,
    ctx: &RowCtx,
    body_color: gpui::Rgba,
    inline: bool,
) -> AnyElement {
    let has_mention = msg
        .spans
        .iter()
        .any(|s| matches!(s, MessageSpan::Mention { .. }));
    if !has_mention && !msg.is_edited {
        return div().into_any_element();
    }
    let mut row = rich_content_row(body_color, inline);
    for span in msg
        .spans
        .iter()
        .filter(|s| matches!(s, MessageSpan::Mention { .. }))
    {
        row = append_span(row, span, ctx, body_color);
    }
    if msg.is_edited {
        row = row.child(edited_marker(ctx.theme, ctx.locale));
    }
    row.into_any_element()
}

fn rich_content_row(body_color: gpui::Rgba, inline: bool) -> gpui::Div {
    let mut row = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_baseline()
        .min_w_0()
        .text_base()
        .line_height(rems(1.375))
        .text_color(body_color);
    if inline {
        row = row.flex_shrink_0();
    } else {
        row = row.w_full().gap_x(px(4.));
    }
    row
}

fn edited_marker(theme: &Theme, locale: &str) -> AnyElement {
    div()
        .text_color(theme.text_muted)
        .text_size(px(9.))
        .child(mezon_i18n::t(locale, "message.edited"))
        .into_any_element()
}

pub fn append_system_mention_spans(mut row: gpui::Div, msg: &Message, ctx: &RowCtx) -> gpui::Div {
    let body_color = ctx.theme.tokens.text_theme_primary;
    for span in msg
        .spans
        .iter()
        .filter(|s| matches!(s, MessageSpan::Mention { .. }))
    {
        row = append_span(row, span, ctx, body_color);
    }
    row
}

fn append_span(
    mut row: gpui::Div,
    span: &MessageSpan,
    ctx: &RowCtx,
    body_color: gpui::Rgba,
) -> gpui::Div {
    let theme = ctx.theme;
    match span {
        MessageSpan::Text(text) => {
            for child in text_to_words(text, body_color) {
                row = row.child(child);
            }
            row
        }
        MessageSpan::Bold(text) => row.child(
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(body_color)
                .child(text.clone()),
        ),
        MessageSpan::Code(text) => row.child(
            div()
                .px_1()
                .rounded_sm()
                .bg(theme.tokens.bg_markdown_code)
                .text_color(body_color)
                .font_family("monospace")
                .child(text.clone()),
        ),
        MessageSpan::CodeBlock { text, .. } => row.child(
            div()
                .w_full()
                .my_1()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(theme.tokens.bg_markdown_code)
                .text_color(body_color)
                .font_family("monospace")
                .child(text.clone()),
        ),
        MessageSpan::Link { text, url } => {
            let resolved = resolve_link_url(url, text);
            for child in link_to_wrap_segments(text, resolved, theme.tokens.mention_color) {
                row = row.child(child);
            }
            row
        }
        MessageSpan::Mention {
            display,
            user_id,
            role_id,
        } => row.child(render_mention_chip(
            display.clone(),
            user_id.as_deref(),
            role_id.as_deref(),
            ctx,
        )),
        MessageSpan::Hashtag {
            display,
            channel_id,
        } => row.child(render_hashtag_chip(
            display.clone(),
            channel_id.as_deref(),
            ctx,
        )),
        MessageSpan::Emoji { name, emoji_id } => {
            row.child(render_emoji_span(name, emoji_id, body_color, ctx))
        }
    }
}

fn render_emoji_span(
    name: &SharedString,
    emoji_id: &str,
    body_color: gpui::Rgba,
    ctx: &RowCtx,
) -> AnyElement {
    let src = crate::util::imgproxy::emoji_url(ctx.app, emoji_id);
    if src.is_empty() {
        return div()
            .text_color(body_color)
            .child(name.clone())
            .into_any_element();
    }
    img(SharedString::from(src))
        .h(px(24.))
        .max_w(px(24.))
        .object_fit(ObjectFit::Contain)
        .with_fallback(super::reaction_detail::emoji_error_fallback(
            px(24.),
            ctx.theme.text_muted,
        ))
        .into_any_element()
}

fn render_mention_chip(
    display: impl Into<SharedString>,
    user_id: Option<&str>,
    role_id: Option<&str>,
    ctx: &RowCtx,
) -> AnyElement {
    let display = display.into();
    let theme = ctx.theme;
    let is_role = role_id.is_some_and(|r| !r.is_empty());
    let (bg, color, hover_bg, hover_color) = if is_role {
        (
            theme.tokens.bg_mention_evryone,
            theme.tokens.color_mention_evryone,
            theme.tokens.bg_mention_everyone_hover,
            theme.tokens.color_mention_everyone_hover,
        )
    } else {
        (
            theme.tokens.mention_primary,
            theme.tokens.mention_color,
            theme.tokens.bg_mention_hover,
            theme.tokens.color_mention_hover,
        )
    };

    let chip = div()
        .flex_none()
        .px(px(1.))
        .rounded_sm()
        .font_weight(FontWeight::MEDIUM)
        .bg(bg)
        .text_color(color)
        .hover(move |s| s.bg(hover_bg).text_color(hover_color))
        .child(display.clone());

    if is_role {
        return chip.into_any_element();
    }

    let Some(uid) = user_id.filter(|uid| !uid.is_empty() && *uid != "0" && !is_here_user_id(uid))
    else {
        return chip.into_any_element();
    };

    let Some(user_id) = uid.parse::<i64>().ok().map(UserId) else {
        return chip.into_any_element();
    };

    let Some(profile_ctx) = ctx.profile_context else {
        return chip.cursor_pointer().into_any_element();
    };

    let settings = ctx.settings.clone();
    let mention_key = user_id.get() as usize;
    profile_popover_menu(
        ("msg-mention-popover", mention_key),
        user_id,
        profile_ctx,
        settings,
    )
    .anchor(Anchor::BottomLeft)
    .attach(Anchor::TopLeft)
    .trigger(
        ClickableContainer::new(("msg-mention", mention_key))
            .flex_none()
            .cursor_pointer()
            .child(chip),
    )
    .into_any_element()
}

fn render_hashtag_chip(
    display: impl Into<SharedString>,
    channel_id: Option<&str>,
    ctx: &RowCtx,
) -> AnyElement {
    let display = display.into();
    let theme = ctx.theme;
    let bg = theme.tokens.mention_primary;
    let color = theme.tokens.mention_color;
    let hover_bg = theme.tokens.bg_mention_hover;
    let hover_color = theme.tokens.color_mention_hover;
    let parsed_channel = channel_id.and_then(parse_channel_id);

    match parsed_channel {
        Some(channel_id) => div()
            .id(("msg-hashtag", channel_id.get() as usize))
            .px(px(1.))
            .rounded_sm()
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .bg(bg)
            .text_color(color)
            .on_click(move |_, _, cx| navigate_to_channel(channel_id, cx))
            .hover(move |s| s.bg(hover_bg).text_color(hover_color))
            .child(display)
            .into_any_element(),
        None => div()
            .px(px(1.))
            .rounded_sm()
            .font_weight(FontWeight::MEDIUM)
            .bg(bg)
            .text_color(color)
            .hover(move |s| s.bg(hover_bg).text_color(hover_color))
            .child(display)
            .into_any_element(),
    }
}

fn parse_channel_id(raw: &str) -> Option<ChannelId> {
    raw.parse::<i64>()
        .ok()
        .map(ChannelId)
        .filter(|id| !id.is_zero())
}

fn navigate_to_channel(channel_id: ChannelId, cx: &mut App) {
    let Some(clan_id) = ChannelList::global(cx)
        .read(cx)
        .clan_id_for_channel(channel_id)
    else {
        return;
    };
    navigate(
        cx,
        Route::Channel {
            clan_id,
            channel_id,
        },
    );
}

fn is_link_only(spans: &[MessageSpan]) -> bool {
    spans.iter().any(|s| matches!(s, MessageSpan::Link { .. }))
        && spans.iter().all(|s| match s {
            MessageSpan::Link { .. } => true,
            MessageSpan::Text(text) => text.trim().is_empty(),
            _ => false,
        })
}

fn render_plain_text_spans(spans: &[MessageSpan], color: gpui::Rgba) -> AnyElement {
    let text: SharedString = match spans {
        [MessageSpan::Text(t)] => t.clone(),
        _ => spans
            .iter()
            .filter_map(|s| match s {
                MessageSpan::Text(t) => Some(t.as_ref()),
                _ => None,
            })
            .collect::<String>()
            .into(),
    };
    if text.is_empty() {
        return div().into_any_element();
    }
    if !text.contains('\n') {
        return div()
            .w_full()
            .min_w_0()
            .min_h(px(30.))
            .text_base()
            .line_height(rems(1.375))
            .text_color(color)
            .child(text)
            .into_any_element();
    }
    let mut col = div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .text_base()
        .line_height(rems(1.375));
    for line in text.split('\n') {
        col = col.child(
            div()
                .w_full()
                .min_w_0()
                .min_h(px(20.))
                .text_color(color)
                .child(line.to_string()),
        );
    }
    col.into_any_element()
}

fn render_link_only_spans(spans: &[MessageSpan], theme: &Theme) -> AnyElement {
    let link_color = theme.tokens.mention_color;
    let mut col = div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .text_base()
        .gap_1();
    for span in spans {
        let MessageSpan::Link { text, url } = span else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        let resolved = resolve_link_url(url, text);
        if !text.contains('\n') {
            col = col.child(link_block(resolved, text.clone(), link_color));
            continue;
        }
        for line in text.split('\n') {
            if line.is_empty() {
                continue;
            }
            col = col.child(link_block(resolved.clone(), line.to_string(), link_color));
        }
    }
    col.into_any_element()
}

fn resolve_link_url(url: &str, text: &str) -> String {
    if !url.is_empty() {
        return url.to_string();
    }
    text.to_string()
}

/// The first detected link in a message's rich-text spans, if any (for the
/// "···" context menu's copy/open-link items).
pub(crate) fn first_link(msg: &Message) -> Option<String> {
    msg.spans.iter().find_map(|span| match span {
        MessageSpan::Link { text, url } => Some(resolve_link_url(url, text)),
        _ => None,
    })
}

pub(crate) fn open_message_link(url: String, cx: &mut App) {
    if url.is_empty() {
        return;
    }
    if let Some(store) = PlatformStore::try_global(cx) {
        let _ = store.read(cx).open_url_external(&url);
    }
}

fn link_block(url: String, display: impl Into<SharedString>, color: gpui::Rgba) -> AnyElement {
    let id = SharedString::from(format!("msg-link-{url}"));
    let display = display.into();
    div()
        .id(id)
        .w_full()
        .min_w_0()
        .cursor_pointer()
        .text_color(color)
        .on_click(move |_, _, cx| open_message_link(url.clone(), cx))
        .child(display)
        .into_any_element()
}

fn link_segment(url: String, display: String, color: gpui::Rgba) -> AnyElement {
    let id = SharedString::from(format!("msg-link-{url}-{display}"));
    div()
        .id(id)
        .cursor_pointer()
        .text_color(color)
        .on_click(move |_, _, cx| open_message_link(url.clone(), cx))
        .child(display)
        .into_any_element()
}

fn text_to_words(text: &str, color: gpui::Rgba) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();
    let mut first_line = true;
    for line in text.split('\n') {
        if !first_line {
            out.push(div().w_full().h_0().into_any_element());
        }
        first_line = false;
        for word in line.split_whitespace() {
            out.push(
                div()
                    .text_color(color)
                    .child(word.to_string())
                    .into_any_element(),
            );
        }
    }
    out
}

fn link_to_wrap_segments(text: &str, url: String, color: gpui::Rgba) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();
    let mut first_line = true;
    for line in text.split('\n') {
        if !first_line {
            out.push(div().w_full().h_0().into_any_element());
        }
        first_line = false;
        if line.chars().any(char::is_whitespace) {
            for word in line.split_whitespace() {
                for segment in split_unbreakable(word) {
                    out.push(link_segment(url.clone(), segment, color));
                }
            }
        } else {
            for segment in split_unbreakable(line) {
                out.push(link_segment(url.clone(), segment, color));
            }
        }
    }
    out
}

fn split_unbreakable(text: &str) -> Vec<String> {
    const MAX_SEGMENT_LEN: usize = 32;
    let mut parts = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        buf.push(ch);
        if matches!(
            ch,
            '/' | '-' | '_' | '.' | '?' | '&' | '#' | '=' | '@' | ':'
        ) || buf.chars().count() >= MAX_SEGMENT_LEN
        {
            parts.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        parts.push(buf);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::parse_channel_id;
    use mezon_store::ChannelId;

    #[test]
    fn parse_channel_id_rejects_zero() {
        assert_eq!(parse_channel_id("0"), None);
        assert_eq!(parse_channel_id("12345"), Some(ChannelId(12345)));
    }
}
