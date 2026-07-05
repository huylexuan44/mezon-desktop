use gpui::{AnyElement, FontWeight, div, prelude::*, px};
use mezon_store::EmbedField;

use super::context::RowCtx;

pub fn render_embed_fields(fields: &[EmbedField], ctx: &RowCtx) -> AnyElement {
    let mut grid = div().mt_2().flex().flex_col().gap_2().w_full();
    for row_fields in group_fields(fields) {
        let multi_column = row_fields.len() > 1;
        let mut row = div().flex().flex_row().gap_4().w_full();
        for field in row_fields {
            row = row.child(render_field(field, multi_column, ctx));
        }
        grid = grid.child(row);
    }
    grid.into_any_element()
}

fn group_fields(fields: &[EmbedField]) -> Vec<Vec<&EmbedField>> {
    let mut rows: Vec<Vec<&EmbedField>> = Vec::new();
    for field in fields {
        if field.inline
            && let Some(last) = rows.last_mut()
            && last.first().is_some_and(|f| f.inline)
            && last.len() < 3
        {
            last.push(field);
        } else {
            rows.push(vec![field]);
        }
    }
    rows
}

fn render_field(field: &EmbedField, multi_column: bool, ctx: &RowCtx) -> AnyElement {
    let mut column = div().flex().flex_col().gap_1().min_w_0();
    if multi_column {
        column = column.flex_1();
    }
    column
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(14.))
                .text_color(ctx.theme.tokens.text_theme_message)
                .child(field.name.clone()),
        )
        .child(
            div()
                .text_size(px(14.))
                .text_color(ctx.theme.tokens.text_theme_message)
                .child(field.value.clone()),
        )
        .into_any_element()
}
