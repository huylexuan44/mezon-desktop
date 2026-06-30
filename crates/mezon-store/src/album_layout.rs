use crate::config::{calculate_media_dimensions, media_available_width};

pub const SIDE_NONE: u8 = 0;
pub const SIDE_TOP: u8 = 1;
pub const SIDE_RIGHT: u8 = 2;
pub const SIDE_BOTTOM: u8 = 4;
pub const SIDE_LEFT: u8 = 8;

const SPACING: f32 = 2.0;
const MIN_WIDTH: f32 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlbumTile {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub sides: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlbumLayout {
    pub tiles: Vec<AlbumTile>,
    pub container_width: f32,
    pub container_height: f32,
}

struct LayoutParams {
    ratios: Vec<f32>,
    proportions: String,
    average_ratio: f32,
    max_width: f32,
    max_height: f32,
}

fn clamp(value: f32, min: f32, max: f32) -> f32 {
    value.max(min).min(max)
}

fn get_ratios(dims: &[(u32, u32)]) -> Vec<f32> {
    dims.iter()
        .map(|&(w, h)| {
            let d = calculate_media_dimensions(w, h, false, 0);
            d.width / d.height
        })
        .collect()
}

fn get_proportions(ratios: &[f32]) -> String {
    ratios
        .iter()
        .map(|&ratio| {
            if ratio > 1.2 {
                'w'
            } else if ratio < 0.8 {
                'n'
            } else {
                'q'
            }
        })
        .collect()
}

fn get_average_ratio(ratios: &[f32]) -> f32 {
    (ratios.iter().sum::<f32>() + 1.0) / ratios.len() as f32
}

fn accumulate(list: &[f32]) -> f32 {
    list.iter().sum()
}

fn crop_ratios(ratios: &[f32], average_ratio: f32) -> Vec<f32> {
    ratios
        .iter()
        .map(|&ratio| {
            if average_ratio > 1.1 {
                clamp(ratio, 1.0, 2.75)
            } else {
                clamp(ratio, 0.6667, 1.0)
            }
        })
        .collect()
}

fn container_size(tiles: &[AlbumTile]) -> (f32, f32) {
    let mut width = 0.0;
    let mut height = 0.0;
    for tile in tiles {
        if tile.sides & SIDE_RIGHT != 0 {
            width = tile.width + tile.x;
        }
        if tile.sides & SIDE_BOTTOM != 0 {
            height = tile.height + tile.y;
        }
    }
    (width, height)
}

pub fn calculate_album_layout(dims: &[(u32, u32)]) -> AlbumLayout {
    let ratios = get_ratios(dims);
    let proportions = get_proportions(&ratios);
    let average_ratio = get_average_ratio(&ratios);
    let count = ratios.len();
    let force_calc = ratios.iter().any(|&r| r > 2.0);
    let max_width = media_available_width(false);
    let params = LayoutParams {
        ratios,
        proportions,
        average_ratio,
        max_width,
        max_height: max_width,
    };

    let tiles = if count >= 5 || force_calc {
        layout_complex(&params)
    } else if count == 2 {
        layout_two(&params)
    } else if count == 3 {
        layout_three(&params)
    } else {
        layout_four(&params)
    };

    let (container_width, container_height) = container_size(&tiles);
    AlbumLayout {
        tiles,
        container_width,
        container_height,
    }
}

struct Attempt {
    line_counts: Vec<usize>,
    heights: Vec<f32>,
}

fn layout_complex(params: &LayoutParams) -> Vec<AlbumTile> {
    let LayoutParams {
        ratios: original_ratios,
        average_ratio,
        max_width,
        max_height,
        ..
    } = params;
    let average_ratio = *average_ratio;
    let max_width = *max_width;
    let max_height = *max_height;
    let ratios = crop_ratios(original_ratios, average_ratio);
    let count = original_ratios.len();
    let mut attempts: Vec<Attempt> = Vec::new();

    let multi_height = |offset: usize, attempt_count: usize| -> f32 {
        let sum = accumulate(&ratios[offset..offset + attempt_count]);
        (max_width - (attempt_count as f32 - 1.0) * SPACING) / sum
    };

    let push_attempt = |line_counts: Vec<usize>, attempts: &mut Vec<Attempt>| {
        let mut heights = Vec::new();
        let mut offset = 0;
        for &current_count in &line_counts {
            heights.push(multi_height(offset, current_count));
            offset += current_count;
        }
        attempts.push(Attempt {
            line_counts,
            heights,
        });
    };

    for first in 1..count {
        let second = count - first;
        if first <= 3 && second <= 3 {
            push_attempt(vec![first, second], &mut attempts);
        }
    }

    for first in 1..count.saturating_sub(1) {
        for second in 1..count - first {
            let third = count - first - second;
            let second_limit = if average_ratio < 0.85 { 4 } else { 3 };
            if first <= 3 && second <= second_limit && third <= 3 {
                push_attempt(vec![first, second, third], &mut attempts);
            }
        }
    }

    for first in 1..count.saturating_sub(1) {
        for second in 1..count - first {
            for third in 1..count - first - second {
                let fourth = count - first - second - third;
                if first <= 3 && second <= 3 && third <= 3 && fourth <= 4 {
                    push_attempt(vec![first, second, third, fourth], &mut attempts);
                }
            }
        }
    }

    if attempts.is_empty() {
        let full_rows = count.div_ceil(3).saturating_sub(1);
        let mut line_counts = vec![3usize; full_rows];
        line_counts.push(count - full_rows * 3);
        push_attempt(line_counts, &mut attempts);
    }

    let mut optimal_index = 0;
    let mut optimal_diff = f32::MAX;
    for (i, attempt) in attempts.iter().enumerate() {
        let line_count = attempt.line_counts.len();
        let total_height = accumulate(&attempt.heights) + SPACING * (line_count as f32 - 1.0);
        let min_line_height = attempt.heights.iter().copied().fold(f32::MAX, f32::min);
        let bad1 = if min_line_height < MIN_WIDTH {
            1.5
        } else {
            1.0
        };
        let mut bad2 = 1.0;
        for line in 1..line_count {
            if attempt.line_counts[line - 1] > attempt.line_counts[line] {
                bad2 = 1.5;
                break;
            }
        }
        let diff = (total_height - max_height).abs() * bad1 * bad2;
        if i == 0 || diff < optimal_diff {
            optimal_index = i;
            optimal_diff = diff;
        }
    }

    let optimal = &attempts[optimal_index];
    let row_count = optimal.line_counts.len();
    let mut result = Vec::with_capacity(count);
    let mut index = 0;
    let mut y = 0.0;
    for row in 0..row_count {
        let col_count = optimal.line_counts[row];
        let line_height = optimal.heights[row];
        let height = line_height.round();
        let mut x = 0.0;
        for col in 0..col_count {
            let sides = SIDE_NONE
                | if row == 0 { SIDE_TOP } else { SIDE_NONE }
                | if row == row_count - 1 {
                    SIDE_BOTTOM
                } else {
                    SIDE_NONE
                }
                | if col == 0 { SIDE_LEFT } else { SIDE_NONE }
                | if col == col_count - 1 {
                    SIDE_RIGHT
                } else {
                    SIDE_NONE
                };
            let ratio = ratios[index];
            let width = if col == col_count - 1 {
                max_width - x
            } else {
                (ratio * line_height).round()
            };
            result.push(AlbumTile {
                x,
                y,
                width,
                height,
                sides,
            });
            x += width + SPACING;
            index += 1;
        }
        y += height + SPACING;
    }
    result
}

fn layout_two(params: &LayoutParams) -> Vec<AlbumTile> {
    let r = &params.ratios;
    if params.proportions == "ww" && params.average_ratio > 1.4 && r[1] - r[0] < 0.2 {
        layout_two_top_bottom(params)
    } else if params.proportions == "ww" || params.proportions == "qq" {
        layout_two_left_right_equal(params)
    } else {
        layout_two_left_right(params)
    }
}

fn layout_two_top_bottom(params: &LayoutParams) -> Vec<AlbumTile> {
    let r = &params.ratios;
    let max_width = params.max_width;
    let height = (max_width / r[0])
        .min((max_width / r[1]).min((params.max_height - SPACING) / 2.0))
        .round();
    vec![
        AlbumTile {
            x: 0.0,
            y: 0.0,
            width: max_width,
            height,
            sides: SIDE_LEFT | SIDE_TOP | SIDE_RIGHT,
        },
        AlbumTile {
            x: 0.0,
            y: height + SPACING,
            width: max_width,
            height,
            sides: SIDE_LEFT | SIDE_BOTTOM | SIDE_RIGHT,
        },
    ]
}

fn layout_two_left_right_equal(params: &LayoutParams) -> Vec<AlbumTile> {
    let r = &params.ratios;
    let width = (params.max_width - SPACING) / 2.0;
    let height = (width / r[0])
        .min((width / r[1]).min(params.max_height))
        .round();
    vec![
        AlbumTile {
            x: 0.0,
            y: 0.0,
            width,
            height,
            sides: SIDE_TOP | SIDE_LEFT | SIDE_BOTTOM,
        },
        AlbumTile {
            x: width + SPACING,
            y: 0.0,
            width,
            height,
            sides: SIDE_TOP | SIDE_RIGHT | SIDE_BOTTOM,
        },
    ]
}

fn layout_two_left_right(params: &LayoutParams) -> Vec<AlbumTile> {
    let r = &params.ratios;
    let max_width = params.max_width;
    let minimal_width = (1.5 * MIN_WIDTH).round();
    let second_width = (0.4 * (max_width - SPACING))
        .max((max_width - SPACING) / r[0] / (1.0 / r[0] + 1.0 / r[1]))
        .round()
        .min(max_width - SPACING - minimal_width);
    let first_width = max_width - second_width - SPACING;
    let height = params
        .max_height
        .min((first_width / r[0]).min(second_width / r[1]).round());
    vec![
        AlbumTile {
            x: 0.0,
            y: 0.0,
            width: first_width,
            height,
            sides: SIDE_TOP | SIDE_LEFT | SIDE_BOTTOM,
        },
        AlbumTile {
            x: first_width + SPACING,
            y: 0.0,
            width: second_width,
            height,
            sides: SIDE_TOP | SIDE_RIGHT | SIDE_BOTTOM,
        },
    ]
}

fn layout_three(params: &LayoutParams) -> Vec<AlbumTile> {
    if params.proportions.as_bytes().first() == Some(&b'n') {
        layout_three_left_and_other(params)
    } else {
        layout_three_top_and_other(params)
    }
}

fn layout_three_left_and_other(params: &LayoutParams) -> Vec<AlbumTile> {
    let r = &params.ratios;
    let max_width = params.max_width;
    let first_height = params.max_height;
    let third_height = ((params.max_height - SPACING) / 2.0)
        .min((r[1] * (max_width - SPACING)) / (r[2] + r[1]))
        .round();
    let second_height = first_height - third_height - SPACING;
    let right_width = MIN_WIDTH.max(
        ((max_width - SPACING) / 2.0)
            .min((third_height * r[2]).min(second_height * r[1]))
            .round(),
    );
    let left_width = (first_height * r[0])
        .round()
        .min(max_width - SPACING - right_width);
    vec![
        AlbumTile {
            x: 0.0,
            y: 0.0,
            width: left_width,
            height: first_height,
            sides: SIDE_TOP | SIDE_LEFT | SIDE_BOTTOM,
        },
        AlbumTile {
            x: left_width + SPACING,
            y: 0.0,
            width: right_width,
            height: second_height,
            sides: SIDE_TOP | SIDE_RIGHT,
        },
        AlbumTile {
            x: left_width + SPACING,
            y: second_height + SPACING,
            width: right_width,
            height: third_height,
            sides: SIDE_BOTTOM | SIDE_RIGHT,
        },
    ]
}

fn layout_three_top_and_other(params: &LayoutParams) -> Vec<AlbumTile> {
    let r = &params.ratios;
    let max_width = params.max_width;
    let first_width = max_width;
    let first_height = (first_width / r[0])
        .min(0.66 * (params.max_height - SPACING))
        .round();
    let second_width = (max_width - SPACING) / 2.0;
    let second_height = (params.max_height - first_height - SPACING)
        .min((second_width / r[1]).min(second_width / r[2]).round());
    let third_width = first_width - second_width - SPACING;
    vec![
        AlbumTile {
            x: 0.0,
            y: 0.0,
            width: first_width,
            height: first_height,
            sides: SIDE_LEFT | SIDE_TOP | SIDE_RIGHT,
        },
        AlbumTile {
            x: 0.0,
            y: first_height + SPACING,
            width: second_width,
            height: second_height,
            sides: SIDE_BOTTOM | SIDE_LEFT,
        },
        AlbumTile {
            x: second_width + SPACING,
            y: first_height + SPACING,
            width: third_width,
            height: second_height,
            sides: SIDE_BOTTOM | SIDE_RIGHT,
        },
    ]
}

fn layout_four(params: &LayoutParams) -> Vec<AlbumTile> {
    if params.proportions.as_bytes().first() == Some(&b'w') {
        layout_four_top_and_other(params)
    } else {
        layout_four_left_and_other(params)
    }
}

fn layout_four_top_and_other(params: &LayoutParams) -> Vec<AlbumTile> {
    let r = &params.ratios;
    let max_width = params.max_width;
    let w = max_width;
    let h0 = (w / r[0]).min(0.66 * (params.max_height - SPACING)).round();
    let h = ((max_width - 2.0 * SPACING) / (r[1] + r[2] + r[3])).round();
    let w0 = MIN_WIDTH.max((0.4 * (max_width - 2.0 * SPACING)).min(h * r[1]).round());
    let w2 = MIN_WIDTH
        .max(0.33 * (max_width - 2.0 * SPACING))
        .max(h * r[3])
        .round();
    let w1 = w - w0 - w2 - 2.0 * SPACING;
    let h1 = (params.max_height - h0 - SPACING).min(h);
    vec![
        AlbumTile {
            x: 0.0,
            y: 0.0,
            width: w,
            height: h0,
            sides: SIDE_LEFT | SIDE_TOP | SIDE_RIGHT,
        },
        AlbumTile {
            x: 0.0,
            y: h0 + SPACING,
            width: w0,
            height: h1,
            sides: SIDE_BOTTOM | SIDE_LEFT,
        },
        AlbumTile {
            x: w0 + SPACING,
            y: h0 + SPACING,
            width: w1,
            height: h1,
            sides: SIDE_BOTTOM,
        },
        AlbumTile {
            x: w0 + SPACING + w1 + SPACING,
            y: h0 + SPACING,
            width: w2,
            height: h1,
            sides: SIDE_RIGHT | SIDE_BOTTOM,
        },
    ]
}

fn layout_four_left_and_other(params: &LayoutParams) -> Vec<AlbumTile> {
    let r = &params.ratios;
    let max_width = params.max_width;
    let h = params.max_height;
    let w0 = (h * r[0]).min(0.6 * (max_width - SPACING)).round();
    let w = ((params.max_height - 2.0 * SPACING) / (1.0 / r[1] + 1.0 / r[2] + 1.0 / r[3])).round();
    let h0 = (w / r[1]).round();
    let h1 = (w / r[2]).round();
    let h2 = h - h0 - h1 - 2.0 * SPACING;
    let w1 = MIN_WIDTH.max((max_width - w0 - SPACING).min(w));
    vec![
        AlbumTile {
            x: 0.0,
            y: 0.0,
            width: w0,
            height: h,
            sides: SIDE_TOP | SIDE_LEFT | SIDE_BOTTOM,
        },
        AlbumTile {
            x: w0 + SPACING,
            y: 0.0,
            width: w1,
            height: h0,
            sides: SIDE_TOP | SIDE_RIGHT,
        },
        AlbumTile {
            x: w0 + SPACING,
            y: h0 + SPACING,
            width: w1,
            height: h1,
            sides: SIDE_RIGHT,
        },
        AlbumTile {
            x: w0 + SPACING,
            y: h0 + h1 + 2.0 * SPACING,
            width: w1,
            height: h2,
            sides: SIDE_BOTTOM | SIDE_RIGHT,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_square_images_use_left_right_equal() {
        let layout = calculate_album_layout(&[(100, 100), (100, 100)]);
        assert_eq!(layout.tiles.len(), 2);
        assert_eq!(layout.tiles[0].width, 231.0);
        assert_eq!(layout.tiles[0].height, 231.0);
        assert_eq!(layout.tiles[0].x, 0.0);
        assert_eq!(layout.tiles[1].x, 233.0);
        assert_eq!(layout.tiles[0].sides, SIDE_TOP | SIDE_LEFT | SIDE_BOTTOM);
        assert_eq!(layout.tiles[1].sides, SIDE_TOP | SIDE_RIGHT | SIDE_BOTTOM);
        assert_eq!(
            (layout.container_width, layout.container_height),
            (464.0, 231.0)
        );
    }

    #[test]
    fn two_wide_images_use_top_bottom() {
        let layout = calculate_album_layout(&[(1600, 900), (1600, 900)]);
        assert_eq!(layout.tiles.len(), 2);
        assert_eq!(layout.tiles[0].width, 464.0);
        assert_eq!(layout.tiles[1].y, layout.tiles[0].height + SPACING);
        assert_eq!(layout.tiles[0].sides, SIDE_LEFT | SIDE_TOP | SIDE_RIGHT);
        assert_eq!(layout.tiles[1].sides, SIDE_LEFT | SIDE_BOTTOM | SIDE_RIGHT);
        assert_eq!(layout.container_width, 464.0);
    }

    fn assert_container_invariants(layout: &AlbumLayout, expected_tiles: usize) {
        assert_eq!(layout.tiles.len(), expected_tiles);
        for tile in &layout.tiles {
            assert!(tile.width > 0.0, "tile width must be positive: {tile:?}");
            assert!(tile.height > 0.0, "tile height must be positive: {tile:?}");
            assert!(tile.x + tile.width <= layout.container_width + 0.5);
            assert!(tile.y + tile.height <= layout.container_height + 0.5);
        }
        assert!(layout.container_width <= media_available_width(false) + 0.5);
    }

    #[test]
    fn three_images_layout_is_consistent() {
        let layout = calculate_album_layout(&[(800, 600), (800, 600), (800, 600)]);
        assert_container_invariants(&layout, 3);
    }

    #[test]
    fn four_images_layout_is_consistent() {
        let layout = calculate_album_layout(&[(800, 600), (600, 800), (800, 600), (600, 800)]);
        assert_container_invariants(&layout, 4);
    }

    #[test]
    fn five_images_use_complex_layouter() {
        let layout =
            calculate_album_layout(&[(800, 600), (600, 800), (800, 600), (600, 800), (800, 600)]);
        assert_container_invariants(&layout, 5);
    }

    #[test]
    fn panorama_triggers_force_calc_complex() {
        let layout = calculate_album_layout(&[(2400, 600), (800, 600)]);
        assert_eq!(layout.tiles.len(), 2);
        assert!(layout.container_width <= media_available_width(false) + 0.5);
    }
}
