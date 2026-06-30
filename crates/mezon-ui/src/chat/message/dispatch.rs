use gpui::{AnyElement, App, div, prelude::*};
use mezon_store::{Message, MessageCode};

use super::context::RowCtx;
use super::parts::render_date_divider;
use super::system_row::{render_system_message, render_unread_break, render_welcome};
use super::time::format_date_divider;
use super::user_row::render_user_message;

/// Route a single message to the correct row renderer, mirroring the top-level
/// branch in React `ChannelMessage.tsx` (unread break -> welcome/indicator ->
/// system -> user). Also emits the date separator when the calendar day changes
/// and decides whether the row is visually grouped with the previous one.
pub fn render_message_item(
    messages: &[Message],
    ix: usize,
    ctx: &RowCtx,
    cx: &App,
) -> AnyElement {
    let Some(msg) = messages.get(ix) else {
        return div().into_any_element();
    };
    let prev = ix.checked_sub(1).and_then(|p| messages.get(p));
    let show_separator = prev.map(|p| p.day_label.as_str()) != Some(msg.day_label.as_str());
    let combined = mezon_store::message_combined_with_prev(prev, msg);
    let show_unread_break = ctx.unread_boundary_id.is_some_and(|id| id == msg.id);

    let row = match msg.code {
        MessageCode::Indicator => render_welcome(msg, ctx),
        code if code.is_system() => render_system_message(msg, ctx),
        _ => render_user_message(msg, combined, ctx, cx),
    };

    if !show_separator && !show_unread_break {
        return row;
    }

    let mut stack = div().flex().flex_col().w_full();
    if show_separator {
        let label = format_date_divider(msg.create_time, ctx.locale);
        stack = stack.child(render_date_divider(ctx.theme, &label));
    }
    if show_unread_break {
        stack = stack.child(render_unread_break(ctx.theme, ctx.locale));
    }
    stack.child(row).into_any_element()
}
