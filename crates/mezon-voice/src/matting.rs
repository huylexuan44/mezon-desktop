use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};

pub const MODEL_WIDTH: usize = 256;
pub const MODEL_HEIGHT: usize = 144;
pub const MATTE_WIDTH: usize = MODEL_WIDTH;
pub const MATTE_HEIGHT: usize = MODEL_HEIGHT;

static SEGMENTATION_MODEL_BYTES: &[u8] = include_bytes!("../assets/selfie_segmentation.onnx");
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

pub struct SelfieSegmentationProcessor {
    session: Option<Session>,
    input_tensor_buf: Vec<f32>,
    pha_buf: Vec<f32>,
    prev_pha: Vec<f32>,
    has_prev_frame: bool,
    cached_bg: Option<PreloadedBackgrounds>,
    small_blur: Vec<u8>,
}

pub type MezonMatteProcessor = SelfieSegmentationProcessor;

impl Default for SelfieSegmentationProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl SelfieSegmentationProcessor {
    pub fn new() -> Self {
        let session = match Session::builder()
            .and_then(|b| b.with_intra_threads(2))
            .and_then(|b| b.commit_from_memory(SEGMENTATION_MODEL_BYTES))
        {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(
                    "Failed to initialize ONNX SelfieSegmentation session from memory: {e}, attempting runtime file fallback"
                );
                let paths = [
                    "crates/mezon-voice/assets/selfie_segmentation.onnx",
                    "assets/selfie_segmentation.onnx",
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
                        "Failed to initialize ONNX SelfieSegmentation session from runtime paths"
                    );
                }
                loaded
            }
        };

        Self {
            session,
            input_tensor_buf: vec![0.0f32; 3 * MODEL_HEIGHT * MODEL_WIDTH],
            pha_buf: vec![0.0f32; MODEL_HEIGHT * MODEL_WIDTH],
            prev_pha: vec![0.0f32; MODEL_HEIGHT * MODEL_WIDTH],
            has_prev_frame: false,
            cached_bg: None,
            small_blur: Vec::new(),
        }
    }

    pub fn reset_states(&mut self) {
        self.has_prev_frame = false;
        self.prev_pha.fill(0.0);
        self.pha_buf.fill(0.0);
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

        // 1. Prepare input tensor: resize BGRA to MODEL_WIDTH x MODEL_HEIGHT normalized RGB [1, 3, 144, 256]
        let step_x = width as f32 / MODEL_WIDTH as f32;
        let step_y = height as f32 / MODEL_HEIGHT as f32;

        let hw = MODEL_HEIGHT * MODEL_WIDTH;
        let r_offset = 0;
        let g_offset = hw;
        let b_offset = hw * 2;

        for my in 0..MODEL_HEIGHT {
            let sy = ((my as f32 * step_y) as usize).min(height - 1);
            let row_offset = sy * width * 4;
            let m_row_offset = my * MODEL_WIDTH;

            for mx in 0..MODEL_WIDTH {
                let sx = ((mx as f32 * step_x) as usize).min(width - 1);
                let src_idx = row_offset + sx * 4;
                // bgra_pixels layout: B, G, R, A
                let b = bgra_pixels[src_idx] as f32 * 0.003921569;
                let g = bgra_pixels[src_idx + 1] as f32 * 0.003921569;
                let r = bgra_pixels[src_idx + 2] as f32 * 0.003921569;

                let dst_idx = m_row_offset + mx;
                self.input_tensor_buf[r_offset + dst_idx] = r;
                self.input_tensor_buf[g_offset + dst_idx] = g;
                self.input_tensor_buf[b_offset + dst_idx] = b;
            }
        }

        // 2. Run ONNX neural inference
        let input_tensor = match Tensor::from_array((
            [1, 3, MODEL_HEIGHT as i64, MODEL_WIDTH as i64],
            self.input_tensor_buf.clone(),
        )) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("segmentation tensor create failed: {e}");
                self.fail_closed(bgra_pixels, width, height, mode);
                return;
            }
        };

        let inference_ok = if let Some(session) = self.session.as_mut() {
            match session.run(ort::inputs!["pixel_values" => input_tensor]) {
                Ok(outputs) => {
                    // Extract alpha mask output: "alphas"
                    match outputs
                        .get("alphas")
                        .and_then(|v| v.try_extract_tensor::<f32>().ok())
                    {
                        Some((_shape, alphas)) if alphas.len() == self.pha_buf.len() => {
                            // 3. Post-process mask: S-curve choke & motion-adaptive temporal smoothing
                            // Matches the MediaPipe enhancement logic in Mezon Web / Meet:
                            // Background / Foreground gate with smooth transition between choke_low and choke_high
                            let choke_low = 0.28f32;
                            let choke_high = 0.65f32;
                            let inv_choke_range = 1.0 / (choke_high - choke_low);

                            for (i, &raw) in alphas.iter().enumerate().take(self.pha_buf.len()) {
                                let cur = if raw <= 0.03 {
                                    0.0
                                } else if raw >= 0.97 {
                                    1.0
                                } else {
                                    let t = ((raw - choke_low) * inv_choke_range).clamp(0.0, 1.0);
                                    t * t * (3.0 - 2.0 * t)
                                };

                                if !self.has_prev_frame {
                                    self.pha_buf[i] = cur;
                                } else {
                                    let prev = self.prev_pha[i];
                                    if cur < prev - 0.02 {
                                        // Fast release: person moved away, reveal virtual background immediately without ghosting
                                        self.pha_buf[i] = cur;
                                    } else {
                                        // Stationary / subtle edge: smooth transition to stabilize jitter
                                        let d = (cur - prev).abs();
                                        let beta = 0.8 * (1.0 - (d / 0.15).min(1.0));
                                        self.pha_buf[i] = beta * prev + (1.0 - beta) * cur;
                                    }
                                }
                            }
                            self.prev_pha.copy_from_slice(&self.pha_buf);
                            self.has_prev_frame = true;
                            true
                        }
                        _ => {
                            tracing::warn!("failed to extract 'alphas' output or length mismatch");
                            false
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("segmentation inference failed: {e}");
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

        // 4. Composite background (Blur or Image bg1..bg5)
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
                if o_idx + 3 < bgra_pixels.len() {
                    self.small_blur[s_idx] = bgra_pixels[o_idx];
                    self.small_blur[s_idx + 1] = bgra_pixels[o_idx + 1];
                    self.small_blur[s_idx + 2] = bgra_pixels[o_idx + 2];
                    self.small_blur[s_idx + 3] = 255;
                }
            }
        }

        // 2-pass 3x3 box blur on downscaled image
        box_blur_bgra(&mut self.small_blur, sw, sh, 3);
        box_blur_bgra(&mut self.small_blur, sw, sh, 3);

        // Bilinear blend onto original image using smoothed alpha
        let scale_x = (MODEL_WIDTH as f32 - 1.0) / (width as f32 - 1.0).max(1.0);
        let scale_y = (MODEL_HEIGHT as f32 - 1.0) / (height as f32 - 1.0).max(1.0);

        for y in 0..height {
            let fy = (y as f32 * scale_y).clamp(0.0, (MODEL_HEIGHT - 1) as f32);
            let y0 = fy as usize;
            let y1 = (y0 + 1).min(MODEL_HEIGHT - 1);
            let dy = fy - y0 as f32;

            let row0 = y0 * MODEL_WIDTH;
            let row1 = y1 * MODEL_WIDTH;
            let sy = (y / 4).min(sh - 1);
            let s_row = sy * sw * 4;
            let row = y * width * 4;

            for x in 0..width {
                let fx = (x as f32 * scale_x).clamp(0.0, (MODEL_WIDTH - 1) as f32);
                let x0 = fx as usize;
                let x1 = (x0 + 1).min(MODEL_WIDTH - 1);
                let dx = fx - x0 as f32;

                let a00 = self.pha_buf[row0 + x0];
                let a10 = self.pha_buf[row0 + x1];
                let a01 = self.pha_buf[row1 + x0];
                let a11 = self.pha_buf[row1 + x1];

                let alpha = ((a00 * (1.0 - dx) + a10 * dx) * (1.0 - dy)
                    + (a01 * (1.0 - dx) + a11 * dx) * dy)
                    .clamp(0.0, 1.0);

                if alpha < 0.999 {
                    let sx = (x / 4).min(sw - 1);
                    let blur_idx = s_row + sx * 4;
                    let out_idx = row + x * 4;

                    if out_idx + 3 <= bgra_pixels.len() && blur_idx + 3 <= self.small_blur.len() {
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

        let scale_x = (MODEL_WIDTH as f32 - 1.0) / (width as f32 - 1.0).max(1.0);
        let scale_y = (MODEL_HEIGHT as f32 - 1.0) / (height as f32 - 1.0).max(1.0);

        for y in 0..height {
            let fy = (y as f32 * scale_y).clamp(0.0, (MODEL_HEIGHT - 1) as f32);
            let y0 = fy as usize;
            let y1 = (y0 + 1).min(MODEL_HEIGHT - 1);
            let dy = fy - y0 as f32;

            let row0 = y0 * MODEL_WIDTH;
            let row1 = y1 * MODEL_WIDTH;
            let row = y * width * 4;

            for x in 0..width {
                let fx = (x as f32 * scale_x).clamp(0.0, (MODEL_WIDTH - 1) as f32);
                let x0 = fx as usize;
                let x1 = (x0 + 1).min(MODEL_WIDTH - 1);
                let dx = fx - x0 as f32;

                let a00 = self.pha_buf[row0 + x0];
                let a10 = self.pha_buf[row0 + x1];
                let a01 = self.pha_buf[row1 + x0];
                let a11 = self.pha_buf[row1 + x1];

                let alpha = ((a00 * (1.0 - dx) + a10 * dx) * (1.0 - dy)
                    + (a01 * (1.0 - dx) + a11 * dx) * dy)
                    .clamp(0.0, 1.0);

                if alpha < 0.999 {
                    let idx = row + x * 4;
                    if idx + 3 <= bgra_pixels.len() && idx + 3 <= bg.len() {
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
