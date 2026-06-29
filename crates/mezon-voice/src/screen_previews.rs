use crate::screen_targets::ScreenShareKind;

#[derive(Clone, Debug)]
pub struct ScreenSharePreview {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

const PREVIEW_MAX_WIDTH: u32 = 420;
const PREVIEW_MAX_HEIGHT: u32 = 236;

pub fn capture_screen_share_preview(kind: ScreenShareKind, id: u32) -> Option<ScreenSharePreview> {
    #[cfg(target_os = "macos")]
    {
        capture_macos_preview(kind, id)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (kind, id);
        None
    }
}

#[cfg(target_os = "macos")]
fn capture_macos_preview(kind: ScreenShareKind, id: u32) -> Option<ScreenSharePreview> {
    use core_graphics_helmer_fork::display::CGDisplay;
    use core_graphics_helmer_fork::geometry::{CGPoint, CGRect, CGSize};
    use core_graphics_helmer_fork::window::{
        self, kCGWindowImageBoundsIgnoreFraming, kCGWindowImageNominalResolution,
        kCGWindowListOptionIncludingWindow,
    };

    let image = match kind {
        ScreenShareKind::Window => {
            let bounds = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(0.0, 0.0));
            window::create_image(
                bounds,
                kCGWindowListOptionIncludingWindow,
                id,
                kCGWindowImageBoundsIgnoreFraming | kCGWindowImageNominalResolution,
            )?
        }
        ScreenShareKind::Display => CGDisplay::new(id).image()?,
    };

    cg_image_to_preview(&image)
}

#[cfg(target_os = "macos")]
fn cg_image_to_preview(
    image: &core_graphics_helmer_fork::image::CGImageRef,
) -> Option<ScreenSharePreview> {
    let width = image.width() as u32;
    let height = image.height() as u32;
    if width == 0 || height == 0 {
        return None;
    }

    let (thumb_w, thumb_h) = preview_dimensions(width, height);
    let bytes_per_row = image.bytes_per_row();
    if bytes_per_row < (width as usize * 4) {
        return None;
    }

    let data = image.data();
    let bytes = data.bytes();
    let mut rgba = vec![0u8; (thumb_w * thumb_h * 4) as usize];

    for y in 0..thumb_h as usize {
        let src_y = y * height as usize / thumb_h as usize;
        for x in 0..thumb_w as usize {
            let src_x = x * width as usize / thumb_w as usize;
            let src_offset = src_y * bytes_per_row + src_x * 4;
            let dst_offset = (y * thumb_w as usize + x) * 4;
            if src_offset + 3 >= bytes.len() {
                continue;
            }
            rgba[dst_offset] = bytes[src_offset];
            rgba[dst_offset + 1] = bytes[src_offset + 1];
            rgba[dst_offset + 2] = bytes[src_offset + 2];
            rgba[dst_offset + 3] = bytes[src_offset + 3];
        }
    }

    Some(ScreenSharePreview {
        width: thumb_w,
        height: thumb_h,
        rgba,
    })
}

fn preview_dimensions(width: u32, height: u32) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    let scale = (PREVIEW_MAX_WIDTH as f32 / width as f32)
        .min(PREVIEW_MAX_HEIGHT as f32 / height as f32)
        .min(1.0);
    let thumb_w = ((width as f32 * scale).round() as u32).max(1);
    let thumb_h = ((height as f32 * scale).round() as u32).max(1);
    (thumb_w, thumb_h)
}
