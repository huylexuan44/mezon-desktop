use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};

pub const MATTE_WIDTH: usize = 384;
pub const MATTE_HEIGHT: usize = 216;

static MATTE_MODEL_BYTES: &[u8] = include_bytes!("../assets/mezon_matte.onnx");
static BG1_BYTES: &[u8] = include_bytes!("../assets/vtbg/bg1.png");
static BG2_BYTES: &[u8] = include_bytes!("../assets/vtbg/bg2.png");
static BG3_BYTES: &[u8] = include_bytes!("../assets/vtbg/bg3.png");
static BG4_BYTES: &[u8] = include_bytes!("../assets/vtbg/bg4.png");
static BG5_BYTES: &[u8] = include_bytes!("../assets/vtbg/bg5.png");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum VirtualBackgroundMode {
    #[default]
    None = 0,
    Blur = 1,
    Bg1 = 2,
    Bg2 = 3,
    Bg3 = 4,
    Bg4 = 5,
    Bg5 = 6,
}

impl VirtualBackgroundMode {
    pub fn from_u8(val: u8) -> Self {
        match val {
            1 => Self::Blur,
            2 => Self::Bg1,
            3 => Self::Bg2,
            4 => Self::Bg3,
            5 => Self::Bg4,
            6 => Self::Bg5,
            _ => Self::None,
        }
    }

    pub fn as_u8(&self) -> u8 {
        *self as u8
    }

    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::None)
    }
}

pub struct PreloadedBackgrounds {
    pub bg1: Vec<u8>,
    pub bg2: Vec<u8>,
    pub bg3: Vec<u8>,
    pub bg4: Vec<u8>,
    pub bg5: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl PreloadedBackgrounds {
    pub fn load_scaled(target_w: u32, target_h: u32) -> Self {
        let decode_and_scale = |bytes: &[u8]| -> Vec<u8> {
            match image::load_from_memory(bytes) {
                Ok(img) => {
                    let scaled =
                        img.resize_exact(target_w, target_h, image::imageops::FilterType::Triangle);
                    let mut bgra = Vec::with_capacity((target_w * target_h * 4) as usize);
                    for pixel in scaled.to_rgba8().pixels() {
                        bgra.push(pixel[2]); // B
                        bgra.push(pixel[1]); // G
                        bgra.push(pixel[0]); // R
                        bgra.push(pixel[3]); // A
                    }
                    bgra
                }
                Err(e) => {
                    tracing::warn!("failed to decode vtbg image: {e}");
                    vec![128; (target_w * target_h * 4) as usize]
                }
            }
        };

        Self {
            bg1: decode_and_scale(BG1_BYTES),
            bg2: decode_and_scale(BG2_BYTES),
            bg3: decode_and_scale(BG3_BYTES),
            bg4: decode_and_scale(BG4_BYTES),
            bg5: decode_and_scale(BG5_BYTES),
            width: target_w,
            height: target_h,
        }
    }

    pub fn get_bgra(&self, mode: VirtualBackgroundMode) -> Option<&[u8]> {
        match mode {
            VirtualBackgroundMode::Bg1 => Some(&self.bg1),
            VirtualBackgroundMode::Bg2 => Some(&self.bg2),
            VirtualBackgroundMode::Bg3 => Some(&self.bg3),
            VirtualBackgroundMode::Bg4 => Some(&self.bg4),
            VirtualBackgroundMode::Bg5 => Some(&self.bg5),
            _ => None,
        }
    }
}

pub struct MezonMatteProcessor {
    session: Option<Session>,
    r1: Vec<f32>,
    r2: Vec<f32>,
    r3: Vec<f32>,
    r4: Vec<f32>,
    input_tensor_buf: Vec<f32>,
    pha_buf: Vec<f32>,
    cached_bg: Option<PreloadedBackgrounds>,
    small_blur: Vec<u8>,
}

impl Default for MezonMatteProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl MezonMatteProcessor {
    pub fn new() -> Self {
        let session = match Session::builder()
            .and_then(|b| b.with_intra_threads(2))
            .and_then(|b| b.commit_from_memory(MATTE_MODEL_BYTES))
        {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(
                    "Failed to initialize ONNX MezonMatte session from memory: {e}, attempting runtime file fallback"
                );
                let paths = [
                    "crates/mezon-voice/assets/mezon_matte.onnx",
                    "assets/mezon_matte.onnx",
                ];
                let mut loaded = None;
                for p in paths {
                    if let Ok(s) = Session::builder()
                        .and_then(|b| b.with_intra_threads(2))
                        .and_then(|b| b.commit_from_file(p))
                    {
                        loaded = Some(s);
                        break;
                    }
                }
                if loaded.is_none() {
                    tracing::warn!(
                        "Failed to initialize ONNX MezonMatte session from runtime paths"
                    );
                }
                loaded
            }
        };

        Self {
            session,
            r1: vec![0.0f32; 16 * 108 * 192],
            r2: vec![0.0f32; 32 * 54 * 96],
            r3: vec![0.0f32; 64 * 27 * 48],
            r4: vec![0.0f32; 128 * 14 * 24],
            input_tensor_buf: vec![0.0f32; 3 * MATTE_HEIGHT * MATTE_WIDTH],
            pha_buf: vec![0.0f32; MATTE_HEIGHT * MATTE_WIDTH],
            cached_bg: None,
            small_blur: Vec::new(),
        }
    }

    pub fn reset_states(&mut self) {
        self.r1.fill(0.0);
        self.r2.fill(0.0);
        self.r3.fill(0.0);
        self.r4.fill(0.0);
    }

    pub fn fail_closed(
        &mut self,
        bgra_pixels: &mut [u8],
        width: usize,
        height: usize,
        mode: VirtualBackgroundMode,
    ) {
        if width == 0 || height == 0 || bgra_pixels.len() < width * height * 4 {
            return;
        }

        // For image background modes, replace the frame with the chosen background image
        if mode != VirtualBackgroundMode::Blur {
            if self
                .cached_bg
                .as_ref()
                .is_none_or(|bg| bg.width != width as u32 || bg.height != height as u32)
            {
                self.cached_bg = Some(PreloadedBackgrounds::load_scaled(
                    width as u32,
                    height as u32,
                ));
            }

            if let Some(bg) = self.cached_bg.as_ref().and_then(|c| c.get_bgra(mode)) {
                let len = (width * height * 4).min(bgra_pixels.len()).min(bg.len());
                bgra_pixels[..len].copy_from_slice(&bg[..len]);
                return;
            }
        }

        // For blur mode (or fallback if background image loading failed):
        // Heavily blur the entire frame so no private content is leaked.
        self.blur_entire_frame(bgra_pixels, width, height);
    }

    fn blur_entire_frame(&mut self, bgra_pixels: &mut [u8], width: usize, height: usize) {
        let sw = (width / 4).max(1);
        let sh = (height / 4).max(1);
        if bgra_pixels.len() < width * height * 4 {
            return;
        }

        self.small_blur.resize(sw * sh * 4, 0);

        // Downscale
        for sy in 0..sh {
            let oy = sy * 4;
            for sx in 0..sw {
                let ox = sx * 4;
                let s_idx = (sy * sw + sx) * 4;
                let o_idx = (oy * width + ox) * 4;
                if o_idx + 3 < bgra_pixels.len() {
                    self.small_blur[s_idx] = bgra_pixels[o_idx];
                    self.small_blur[s_idx + 1] = bgra_pixels[o_idx + 1];
                    self.small_blur[s_idx + 2] = bgra_pixels[o_idx + 2];
                    self.small_blur[s_idx + 3] = 255;
                }
            }
        }

        // 2-pass box blur on downscaled image
        box_blur_bgra(&mut self.small_blur, sw, sh, 5);
        box_blur_bgra(&mut self.small_blur, sw, sh, 5);

        // Upscale blurred result back to the original image
        let scale_x = sw as f32 / width as f32;
        let scale_y = sh as f32 / height as f32;
        for y in 0..height {
            let sy = ((y as f32 * scale_y) as usize).min(sh - 1);
            let s_row = sy * sw * 4;
            let row = y * width * 4;

            for x in 0..width {
                let sx = ((x as f32 * scale_x) as usize).min(sw - 1);
                let blur_idx = s_row + sx * 4;
                let out_idx = row + x * 4;

                if out_idx + 3 <= bgra_pixels.len() && blur_idx + 3 <= self.small_blur.len() {
                    bgra_pixels[out_idx] = self.small_blur[blur_idx];
                    bgra_pixels[out_idx + 1] = self.small_blur[blur_idx + 1];
                    bgra_pixels[out_idx + 2] = self.small_blur[blur_idx + 2];
                }
            }
        }
    }

    pub fn process_frame(
        &mut self,
        bgra_pixels: &mut [u8],
        width: usize,
        height: usize,
        mode: VirtualBackgroundMode,
    ) {
        if !mode.is_enabled() {
            return;
        }

        if self.session.is_none() {
            self.fail_closed(bgra_pixels, width, height, mode);
            return;
        }

        if width == 0 || height == 0 || bgra_pixels.len() < width * height * 4 {
            return;
        }

        // 1. Prepare input tensor: resize BGRA to 384x216 normalized RGB
        let step_x = width as f32 / MATTE_WIDTH as f32;
        let step_y = height as f32 / MATTE_HEIGHT as f32;

        let hw = MATTE_HEIGHT * MATTE_WIDTH;
        let r_offset = 0;
        let g_offset = hw;
        let b_offset = hw * 2;

        for my in 0..MATTE_HEIGHT {
            let sy = ((my as f32 * step_y) as usize).min(height - 1);
            let row_offset = sy * width * 4;
            let m_row_offset = my * MATTE_WIDTH;

            for mx in 0..MATTE_WIDTH {
                let sx = ((mx as f32 * step_x) as usize).min(width - 1);
                let src_idx = row_offset + sx * 4;
                let b = bgra_pixels[src_idx] as f32 / 255.0;
                let g = bgra_pixels[src_idx + 1] as f32 / 255.0;
                let r = bgra_pixels[src_idx + 2] as f32 / 255.0;

                let dst_idx = m_row_offset + mx;
                self.input_tensor_buf[r_offset + dst_idx] = r;
                self.input_tensor_buf[g_offset + dst_idx] = g;
                self.input_tensor_buf[b_offset + dst_idx] = b;
            }
        }

        // 2. Run ONNX neural inference
        let src_tensor = match Tensor::from_array((
            [1, 3, MATTE_HEIGHT as i64, MATTE_WIDTH as i64],
            self.input_tensor_buf.clone(),
        )) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("matte tensor create failed: {e}");
                self.fail_closed(bgra_pixels, width, height, mode);
                return;
            }
        };

        let r1_tensor = match Tensor::from_array(([1, 16, 108, 192], self.r1.clone())) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("r1 tensor create failed: {e}");
                self.fail_closed(bgra_pixels, width, height, mode);
                return;
            }
        };
        let r2_tensor = match Tensor::from_array(([1, 32, 54, 96], self.r2.clone())) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("r2 tensor create failed: {e}");
                self.fail_closed(bgra_pixels, width, height, mode);
                return;
            }
        };
        let r3_tensor = match Tensor::from_array(([1, 64, 27, 48], self.r3.clone())) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("r3 tensor create failed: {e}");
                self.fail_closed(bgra_pixels, width, height, mode);
                return;
            }
        };
        let r4_tensor = match Tensor::from_array(([1, 128, 14, 24], self.r4.clone())) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("r4 tensor create failed: {e}");
                self.fail_closed(bgra_pixels, width, height, mode);
                return;
            }
        };

        let inference_ok = if let Some(session) = self.session.as_mut() {
            match session.run(ort::inputs![
                "src" => src_tensor,
                "r1i" => r1_tensor,
                "r2i" => r2_tensor,
                "r3i" => r3_tensor,
                "r4i" => r4_tensor
            ]) {
                Ok(outputs) => {
                    // Extract alpha matte by name: "pha"
                    let pha_extracted = match outputs
                        .get("pha")
                        .and_then(|v| v.try_extract_tensor::<f32>().ok())
                    {
                        Some((_shape, pha)) if pha.len() == self.pha_buf.len() => {
                            self.pha_buf.copy_from_slice(pha);
                            true
                        }
                        _ => {
                            tracing::warn!("failed to extract 'pha' output or length mismatch");
                            false
                        }
                    };

                    if pha_extracted {
                        // Update recurrent states for temporal stability by name, checking lengths before copying
                        if let Some((_, r1o)) = outputs
                            .get("r1o")
                            .and_then(|v| v.try_extract_tensor::<f32>().ok())
                        {
                            if r1o.len() == self.r1.len() {
                                self.r1.copy_from_slice(r1o);
                            } else {
                                tracing::warn!(
                                    "r1o length mismatch: expected {}, got {}",
                                    self.r1.len(),
                                    r1o.len()
                                );
                            }
                        }
                        if let Some((_, r2o)) = outputs
                            .get("r2o")
                            .and_then(|v| v.try_extract_tensor::<f32>().ok())
                        {
                            if r2o.len() == self.r2.len() {
                                self.r2.copy_from_slice(r2o);
                            } else {
                                tracing::warn!(
                                    "r2o length mismatch: expected {}, got {}",
                                    self.r2.len(),
                                    r2o.len()
                                );
                            }
                        }
                        if let Some((_, r3o)) = outputs
                            .get("r3o")
                            .and_then(|v| v.try_extract_tensor::<f32>().ok())
                        {
                            if r3o.len() == self.r3.len() {
                                self.r3.copy_from_slice(r3o);
                            } else {
                                tracing::warn!(
                                    "r3o length mismatch: expected {}, got {}",
                                    self.r3.len(),
                                    r3o.len()
                                );
                            }
                        }
                        if let Some((_, r4o)) = outputs
                            .get("r4o")
                            .and_then(|v| v.try_extract_tensor::<f32>().ok())
                        {
                            if r4o.len() == self.r4.len() {
                                self.r4.copy_from_slice(r4o);
                            } else {
                                tracing::warn!(
                                    "r4o length mismatch: expected {}, got {}",
                                    self.r4.len(),
                                    r4o.len()
                                );
                            }
                        }
                        true
                    } else {
                        false
                    }
                }
                Err(e) => {
                    tracing::warn!("matte inference failed: {e}");
                    false
                }
            }
        } else {
            false
        };

        if !inference_ok {
            self.fail_closed(bgra_pixels, width, height, mode);
            return;
        }

        // 3. Composite background
        if mode == VirtualBackgroundMode::Blur {
            self.composite_blur(bgra_pixels, width, height);
        } else {
            self.composite_image(bgra_pixels, width, height, mode);
        }
    }

    fn composite_blur(&mut self, bgra_pixels: &mut [u8], width: usize, height: usize) {
        // Fast dual-pass bokeh blur:
        // Downscale to 1/4 size, apply box blur, and bilinearly sample
        let sw = width / 4;
        let sh = height / 4;
        if sw == 0 || sh == 0 {
            return;
        }

        self.small_blur.resize(sw * sh * 4, 0);

        // Downscale
        for sy in 0..sh {
            let oy = sy * 4;
            for sx in 0..sw {
                let ox = sx * 4;
                let s_idx = (sy * sw + sx) * 4;
                let o_idx = (oy * width + ox) * 4;
                self.small_blur[s_idx] = bgra_pixels[o_idx];
                self.small_blur[s_idx + 1] = bgra_pixels[o_idx + 1];
                self.small_blur[s_idx + 2] = bgra_pixels[o_idx + 2];
                self.small_blur[s_idx + 3] = 255;
            }
        }

        // 2-pass 3x3 box blur on downscaled image
        box_blur_bgra(&mut self.small_blur, sw, sh, 3);
        box_blur_bgra(&mut self.small_blur, sw, sh, 3);

        // Bilinear blend onto original image using pha
        let scale_mx = (MATTE_WIDTH as f32 - 1.0) / (width as f32 - 1.0).max(1.0);
        let scale_my = (MATTE_HEIGHT as f32 - 1.0) / (height as f32 - 1.0).max(1.0);

        for y in 0..height {
            let my = (y as f32 * scale_my) as usize;
            let m_row = my * MATTE_WIDTH;
            let sy = (y / 4).min(sh - 1);
            let s_row = sy * sw * 4;
            let row = y * width * 4;

            for x in 0..width {
                let mx = (x as f32 * scale_mx) as usize;
                let alpha = self.pha_buf[m_row + mx].clamp(0.0, 1.0);

                if alpha < 0.99 {
                    let sx = (x / 4).min(sw - 1);
                    let blur_idx = s_row + sx * 4;
                    let out_idx = row + x * 4;

                    let bg_b = self.small_blur[blur_idx] as f32;
                    let bg_g = self.small_blur[blur_idx + 1] as f32;
                    let bg_r = self.small_blur[blur_idx + 2] as f32;

                    let fg_b = bgra_pixels[out_idx] as f32;
                    let fg_g = bgra_pixels[out_idx + 1] as f32;
                    let fg_r = bgra_pixels[out_idx + 2] as f32;

                    let inv = 1.0 - alpha;
                    bgra_pixels[out_idx] = (fg_b * alpha + bg_b * inv) as u8;
                    bgra_pixels[out_idx + 1] = (fg_g * alpha + bg_g * inv) as u8;
                    bgra_pixels[out_idx + 2] = (fg_r * alpha + bg_r * inv) as u8;
                }
            }
        }
    }

    fn composite_image(
        &mut self,
        bgra_pixels: &mut [u8],
        width: usize,
        height: usize,
        mode: VirtualBackgroundMode,
    ) {
        if self
            .cached_bg
            .as_ref()
            .is_none_or(|bg| bg.width != width as u32 || bg.height != height as u32)
        {
            self.cached_bg = Some(PreloadedBackgrounds::load_scaled(
                width as u32,
                height as u32,
            ));
        }

        let bg = match self.cached_bg.as_ref().and_then(|c| c.get_bgra(mode)) {
            Some(b) => b,
            None => {
                self.blur_entire_frame(bgra_pixels, width, height);
                return;
            }
        };

        let scale_mx = (MATTE_WIDTH as f32 - 1.0) / (width as f32 - 1.0).max(1.0);
        let scale_my = (MATTE_HEIGHT as f32 - 1.0) / (height as f32 - 1.0).max(1.0);

        for y in 0..height {
            let my = (y as f32 * scale_my) as usize;
            let m_row = my * MATTE_WIDTH;
            let row = y * width * 4;

            for x in 0..width {
                let mx = (x as f32 * scale_mx) as usize;
                let alpha = self.pha_buf[m_row + mx].clamp(0.0, 1.0);

                if alpha < 0.99 {
                    let idx = row + x * 4;
                    let bg_b = bg[idx] as f32;
                    let bg_g = bg[idx + 1] as f32;
                    let bg_r = bg[idx + 2] as f32;

                    let fg_b = bgra_pixels[idx] as f32;
                    let fg_g = bgra_pixels[idx + 1] as f32;
                    let fg_r = bgra_pixels[idx + 2] as f32;

                    let inv = 1.0 - alpha;
                    bgra_pixels[idx] = (fg_b * alpha + bg_b * inv) as u8;
                    bgra_pixels[idx + 1] = (fg_g * alpha + bg_g * inv) as u8;
                    bgra_pixels[idx + 2] = (fg_r * alpha + bg_r * inv) as u8;
                }
            }
        }
    }
}

fn box_blur_bgra(pixels: &mut [u8], w: usize, h: usize, radius: usize) {
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    let mut temp = pixels.to_vec();

    // Horizontal pass
    for y in 0..h {
        let row_offset = y * w * 4;
        for x in 0..w {
            let min_x = x.saturating_sub(radius);
            let max_x = (x + radius).min(w - 1);
            let count = (max_x - min_x + 1) as u32;

            let mut b_acc = 0u32;
            let mut g_acc = 0u32;
            let mut r_acc = 0u32;

            for kx in min_x..=max_x {
                let idx = row_offset + kx * 4;
                b_acc += pixels[idx] as u32;
                g_acc += pixels[idx + 1] as u32;
                r_acc += pixels[idx + 2] as u32;
            }

            let dst_idx = row_offset + x * 4;
            temp[dst_idx] = (b_acc / count) as u8;
            temp[dst_idx + 1] = (g_acc / count) as u8;
            temp[dst_idx + 2] = (r_acc / count) as u8;
        }
    }

    // Vertical pass
    for y in 0..h {
        let min_y = y.saturating_sub(radius);
        let max_y = (y + radius).min(h - 1);
        let count = (max_y - min_y + 1) as u32;

        let row_offset = y * w * 4;
        for x in 0..w {
            let mut b_acc = 0u32;
            let mut g_acc = 0u32;
            let mut r_acc = 0u32;

            for ky in min_y..=max_y {
                let idx = ky * w * 4 + x * 4;
                b_acc += temp[idx] as u32;
                g_acc += temp[idx + 1] as u32;
                r_acc += temp[idx + 2] as u32;
            }

            let dst_idx = row_offset + x * 4;
            pixels[dst_idx] = (b_acc / count) as u8;
            pixels[dst_idx + 1] = (g_acc / count) as u8;
            pixels[dst_idx + 2] = (r_acc / count) as u8;
        }
    }
}
