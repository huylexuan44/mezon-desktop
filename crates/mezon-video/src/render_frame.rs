use std::sync::Arc;

use gpui::RenderImage;

pub type VideoFrame = Arc<RenderImage>;

pub fn bgra_to_frame(width: u32, height: u32, bgra: Vec<u8>) -> Option<VideoFrame> {
    if !crate::frame_util::is_valid_bgra_len(width, height, bgra.len()) {
        return None;
    }
    let buffer = image::RgbaImage::from_raw(width, height, bgra)?;
    let frame = image::Frame::new(buffer);
    Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}
