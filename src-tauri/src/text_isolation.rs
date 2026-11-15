use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use opencv::{
    core::{self, Mat, Point, Rect, Scalar, Size, Vector},
    imgcodecs, imgproc,
    prelude::*,
};
use serde::Serialize;
use std::time::Instant;
use wis_core::Region;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedLine {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IsolationStep {
    pub name: &'static str,
    pub preview_image_data_url: String,
    pub elapsed_micros: Option<u128>,
}

pub struct IsolationResult {
    pub lines: Vec<DetectedLine>,
    pub steps: Vec<IsolationStep>,
}

struct StageFrame {
    name: &'static str,
    image: Mat,
    elapsed_micros: Option<u128>,
}

struct IsolationContext {
    current: Mat,
    original_bgr: Option<Mat>,
    edges: Option<Mat>,
    candidates: Vec<Rect>,
    history: Vec<StageFrame>,
    keep_history: bool,
    crop_origin: (u32, u32),
    crop_size: (u32, u32),
    search_bounds: Region,
    frame_size: (u32, u32),
    logical_size: (u32, u32),
}

impl IsolationContext {
    fn replace_current(
        &mut self,
        name: &'static str,
        image: Mat,
        elapsed_micros: u128,
    ) -> Result<(), String> {
        if self.keep_history {
            self.history.push(StageFrame {
                name,
                image: image.try_clone().map_err(|error| error.to_string())?,
                elapsed_micros: Some(elapsed_micros),
            });
        }
        self.current = image;
        Ok(())
    }
}

pub fn isolate_text(
    frame: &[u8],
    frame_size: (u32, u32),
    logical_size: (u32, u32),
    search_bounds: Region,
    keep_history: bool,
) -> Result<IsolationResult, String> {
    let mut ctx = context_from_frame(frame, frame_size, logical_size, search_bounds, keep_history)?;
    ctx = to_bgr(ctx)?;
    ctx = gaussian_blur(ctx)?;
    ctx = color_channel_gradient(ctx)?;
    ctx = relaxed_otsu_threshold(ctx)?;
    ctx = connect_horizontal_glyphs(ctx)?;
    ctx = find_glyph_candidates(ctx)?;

    let lines = candidates_to_lines(&ctx);
    let steps = encode_history(ctx.history)?;
    Ok(IsolationResult { lines, steps })
}

fn context_from_frame(
    frame: &[u8],
    frame_size: (u32, u32),
    logical_size: (u32, u32),
    search_bounds: Region,
    keep_history: bool,
) -> Result<IsolationContext, String> {
    let (frame_width, frame_height) = frame_size;
    let (logical_width, logical_height) = logical_size;
    if frame_width == 0 || frame_height == 0 || logical_width == 0 || logical_height == 0 {
        return Err("capture and logical dimensions must be non-zero".to_owned());
    }
    let expected = frame_width as usize * frame_height as usize * 4;
    if frame.len() < expected {
        return Err(format!(
            "BGRA frame is too short: expected {expected} bytes, received {}",
            frame.len()
        ));
    }

    let scale = |value: u32, captured: u32, logical: u32| {
        ((value as u64 * captured as u64 + logical as u64 / 2) / logical as u64) as u32
    };
    let left = scale(search_bounds.x, frame_width, logical_width).min(frame_width);
    let top = scale(search_bounds.y, frame_height, logical_height).min(frame_height);
    let right = scale(
        search_bounds.x.saturating_add(search_bounds.width),
        frame_width,
        logical_width,
    )
    .min(frame_width);
    let bottom = scale(
        search_bounds.y.saturating_add(search_bounds.height),
        frame_height,
        logical_height,
    )
    .min(frame_height);
    if right <= left || bottom <= top {
        return Err("the isolation area is outside the captured screen".to_owned());
    }

    let source = Mat::from_slice(&frame[..expected]).map_err(|error| error.to_string())?;
    let source = source
        .reshape(4, frame_height as i32)
        .map_err(|error| error.to_string())?;
    let roi = Mat::roi(
        &source,
        Rect::new(
            left as i32,
            top as i32,
            (right - left) as i32,
            (bottom - top) as i32,
        ),
    )
    .map_err(|error| error.to_string())?;
    let current = roi.try_clone().map_err(|error| error.to_string())?;

    let mut ctx = IsolationContext {
        current,
        original_bgr: None,
        edges: None,
        candidates: Vec::new(),
        history: Vec::new(),
        keep_history,
        crop_origin: (left, top),
        crop_size: (right - left, bottom - top),
        search_bounds,
        frame_size,
        logical_size,
    };
    if keep_history {
        ctx.history.push(StageFrame {
            name: "captured-region",
            image: ctx.current.try_clone().map_err(|error| error.to_string())?,
            elapsed_micros: None,
        });
    }
    Ok(ctx)
}

fn to_bgr(mut ctx: IsolationContext) -> Result<IsolationContext, String> {
    let started = Instant::now();
    let mut bgr = Mat::default();
    imgproc::cvt_color_def(&ctx.current, &mut bgr, imgproc::COLOR_BGRA2BGR)
        .map_err(|error| error.to_string())?;
    ctx.original_bgr = Some(bgr.try_clone().map_err(|error| error.to_string())?);
    ctx.replace_current("bgr", bgr, started.elapsed().as_micros())?;
    Ok(ctx)
}

fn gaussian_blur(mut ctx: IsolationContext) -> Result<IsolationContext, String> {
    let started = Instant::now();
    let mut smoothed = Mat::default();
    imgproc::gaussian_blur_def(&ctx.current, &mut smoothed, Size::new(3, 3), 0.0)
        .map_err(|error| error.to_string())?;
    ctx.replace_current("gaussian-blur-3x3", smoothed, started.elapsed().as_micros())?;
    Ok(ctx)
}

fn color_channel_gradient(mut ctx: IsolationContext) -> Result<IsolationContext, String> {
    let started = Instant::now();
    let kernel =
        imgproc::get_structuring_element(imgproc::MORPH_RECT, Size::new(3, 3), Point::new(-1, -1))
            .map_err(|error| error.to_string())?;
    let mut gradient = Mat::default();
    for channel_index in 0..3 {
        let mut channel = Mat::default();
        core::extract_channel(&ctx.current, &mut channel, channel_index)
            .map_err(|error| error.to_string())?;
        let mut channel_gradient = Mat::default();
        imgproc::morphology_ex(
            &channel,
            &mut channel_gradient,
            imgproc::MORPH_GRADIENT,
            &kernel,
            Point::new(-1, -1),
            1,
            core::BORDER_CONSTANT,
            Scalar::all(0.0),
        )
        .map_err(|error| error.to_string())?;
        if channel_index == 0 {
            gradient = channel_gradient;
        } else {
            let mut combined = Mat::default();
            core::max(&gradient, &channel_gradient, &mut combined)
                .map_err(|error| error.to_string())?;
            gradient = combined;
        }
    }
    ctx.replace_current(
        "color-channel-gradient",
        gradient,
        started.elapsed().as_micros(),
    )?;
    Ok(ctx)
}

fn relaxed_otsu_threshold(mut ctx: IsolationContext) -> Result<IsolationContext, String> {
    let started = Instant::now();
    let mut edges = Mat::default();
    let otsu_threshold = imgproc::threshold(
        &ctx.current,
        &mut edges,
        0.0,
        255.0,
        imgproc::THRESH_BINARY | imgproc::THRESH_OTSU,
    )
    .map_err(|error| error.to_string())?;
    let relaxed_threshold = (otsu_threshold * 0.82).max(3.0);
    imgproc::threshold(
        &ctx.current,
        &mut edges,
        relaxed_threshold,
        255.0,
        imgproc::THRESH_BINARY,
    )
    .map_err(|error| error.to_string())?;
    ctx.edges = Some(edges.try_clone().map_err(|error| error.to_string())?);
    ctx.replace_current(
        "relaxed-otsu-threshold",
        edges,
        started.elapsed().as_micros(),
    )?;
    Ok(ctx)
}

fn connect_horizontal_glyphs(mut ctx: IsolationContext) -> Result<IsolationContext, String> {
    let started = Instant::now();
    const MAX_LOGICAL_LINE_GAP: u32 = 12;
    let maximum_line_gap = ((MAX_LOGICAL_LINE_GAP as u64 * ctx.crop_size.0 as u64
        + ctx.search_bounds.width as u64 / 2)
        / ctx.search_bounds.width.max(1) as u64)
        .max(1) as i32;
    let kernel_width = maximum_line_gap.clamp(5, 17) | 1;
    let line_kernel = imgproc::get_structuring_element(
        imgproc::MORPH_RECT,
        Size::new(kernel_width, 1),
        Point::new(-1, -1),
    )
    .map_err(|error| error.to_string())?;
    let mut connected = Mat::default();
    imgproc::morphology_ex(
        &ctx.current,
        &mut connected,
        imgproc::MORPH_CLOSE,
        &line_kernel,
        Point::new(-1, -1),
        1,
        core::BORDER_CONSTANT,
        Scalar::all(0.0),
    )
    .map_err(|error| error.to_string())?;
    ctx.replace_current(
        "horizontal-glyph-connection",
        connected,
        started.elapsed().as_micros(),
    )?;
    Ok(ctx)
}

fn find_glyph_candidates(mut ctx: IsolationContext) -> Result<IsolationContext, String> {
    let started = Instant::now();
    let edges = ctx
        .edges
        .as_ref()
        .ok_or_else(|| "edge stage is missing".to_owned())?;
    let bgr = ctx
        .original_bgr
        .as_ref()
        .ok_or_else(|| "BGR source stage is missing".to_owned())?;
    let mut contours: Vector<Vector<Point>> = Vector::new();
    imgproc::find_contours(
        &ctx.current,
        &mut contours,
        imgproc::RETR_EXTERNAL,
        imgproc::CHAIN_APPROX_SIMPLE,
        Point::new(0, 0),
    )
    .map_err(|error| error.to_string())?;

    let maximum_height = (ctx.crop_size.1 / 8).max(32) as i32;
    for index in 0..contours.len() {
        let contour = contours.get(index).map_err(|error| error.to_string())?;
        let rect = imgproc::bounding_rect(&contour).map_err(|error| error.to_string())?;
        let area = rect.width.saturating_mul(rect.height);
        if rect.width < 6 || rect.height < 4 || rect.height > maximum_height || area < 40 {
            continue;
        }
        let edge_roi = Mat::roi(edges, rect).map_err(|error| error.to_string())?;
        let edge_density = core::count_non_zero(&edge_roi).map_err(|error| error.to_string())?
            as f64
            / area as f64;
        let minimum_density = if area < 800 { 0.009 } else { 0.015 };
        if edge_density < minimum_density || edge_density > 0.72 {
            continue;
        }
        let color_roi = Mat::roi(bgr, rect).map_err(|error| error.to_string())?;
        let mut mean = Mat::default();
        let mut deviation = Mat::default();
        core::mean_std_dev(&color_roi, &mut mean, &mut deviation, &core::no_array())
            .map_err(|error| error.to_string())?;
        let strongest_deviation = deviation
            .data_typed::<f64>()
            .map_err(|error| error.to_string())?
            .iter()
            .copied()
            .fold(0.0, f64::max);
        if strongest_deviation < 3.5 || has_excessive_local_color(bgr, rect)? {
            continue;
        }
        let global_x = ctx.crop_origin.0.saturating_add(rect.x.max(0) as u32);
        let global_y = ctx.crop_origin.1.saturating_add(rect.y.max(0) as u32);
        let global_right = global_x.saturating_add(rect.width.max(0) as u32);
        let stop_left = ctx.frame_size.0.saturating_div(2).saturating_sub(72);
        let stop_right = ctx.frame_size.0.saturating_div(2).saturating_add(72);
        if global_y < 64 && global_right > stop_left && global_x < stop_right {
            continue;
        }
        ctx.candidates.push(rect);
    }
    ctx.candidates.sort_by_key(|rect| (rect.y, rect.x));
    ctx.candidates.truncate(256);

    if ctx.keep_history {
        let mut overlay = bgr.try_clone().map_err(|error| error.to_string())?;
        for rect in &ctx.candidates {
            imgproc::rectangle(
                &mut overlay,
                *rect,
                Scalar::new(114.0, 255.0, 181.0, 0.0),
                1,
                imgproc::LINE_8,
                0,
            )
            .map_err(|error| error.to_string())?;
        }
        ctx.replace_current(
            "accepted-glyph-candidates",
            overlay,
            started.elapsed().as_micros(),
        )?;
    }
    Ok(ctx)
}

fn candidates_to_lines(ctx: &IsolationContext) -> Vec<DetectedLine> {
    let scale = |value: u32, captured: u32, logical: u32| {
        ((value as u64 * logical as u64 + captured as u64 / 2) / captured as u64) as u32
    };
    let mut lines = Vec::with_capacity(ctx.candidates.len());
    for rect in &ctx.candidates {
        let local_left = rect.x.saturating_sub(4).max(0) as u32;
        let local_top = rect.y.saturating_sub(2).max(0) as u32;
        let local_right = (rect.x + rect.width + 4).max(0) as u32;
        let local_bottom = (rect.y + rect.height + 2).max(0) as u32;
        let left = (ctx.crop_origin.0 + local_left).min(ctx.frame_size.0);
        let top = (ctx.crop_origin.1 + local_top).min(ctx.frame_size.1);
        let right = (ctx.crop_origin.0 + local_right).min(ctx.frame_size.0);
        let bottom = (ctx.crop_origin.1 + local_bottom).min(ctx.frame_size.1);
        let x = scale(left, ctx.frame_size.0, ctx.logical_size.0);
        let y = scale(top, ctx.frame_size.1, ctx.logical_size.1);
        let scaled_right = scale(right, ctx.frame_size.0, ctx.logical_size.0);
        let scaled_bottom = scale(bottom, ctx.frame_size.1, ctx.logical_size.1);
        lines.push(DetectedLine {
            x,
            y,
            width: scaled_right.saturating_sub(x).max(1),
            height: scaled_bottom.saturating_sub(y).max(1),
        });
    }
    lines.sort_by_key(|line| (line.y, line.x));
    lines
}

fn has_excessive_local_color(bgr: &Mat, rect: Rect) -> Result<bool, String> {
    if rect.width < 3 || rect.height < 3 {
        return Ok(false);
    }
    let mut checked_windows = 0_u32;
    let mut colorful_windows = 0_u32;
    let sample_step = (rect.width.max(rect.height) / 64).max(2);
    let mut center_y = 1;
    while center_y < rect.height - 1 {
        let mut center_x = 1;
        while center_x < rect.width - 1 {
            let mut colors = [0_u16; 9];
            let mut color_count = 0_usize;
            for offset_y in -1..=1 {
                for offset_x in -1..=1 {
                    let pixel = *bgr
                        .at_2d::<core::Vec3b>(
                            rect.y + center_y + offset_y,
                            rect.x + center_x + offset_x,
                        )
                        .map_err(|error| error.to_string())?;
                    let color = ((pixel[0] as u16 >> 5) << 6)
                        | ((pixel[1] as u16 >> 5) << 3)
                        | (pixel[2] as u16 >> 5);
                    if !colors[..color_count].contains(&color) {
                        colors[color_count] = color;
                        color_count += 1;
                    }
                }
            }
            checked_windows += 1;
            if color_count >= 6 {
                colorful_windows += 1;
            }
            center_x += sample_step;
        }
        center_y += sample_step;
    }
    Ok(checked_windows >= 6 && colorful_windows * 100 > checked_windows * 28)
}

fn encode_history(history: Vec<StageFrame>) -> Result<Vec<IsolationStep>, String> {
    history
        .into_iter()
        .map(|stage| {
            let scale = (480.0 / stage.image.cols() as f64)
                .min(240.0 / stage.image.rows() as f64)
                .min(1.0);
            let preview_size = Size::new(
                ((stage.image.cols() as f64 * scale).round() as i32).max(1),
                ((stage.image.rows() as f64 * scale).round() as i32).max(1),
            );
            let mut preview = Mat::default();
            imgproc::resize(
                &stage.image,
                &mut preview,
                preview_size,
                0.0,
                0.0,
                imgproc::INTER_AREA,
            )
            .map_err(|error| error.to_string())?;
            let mut encoded = Vector::new();
            imgcodecs::imencode(".png", &preview, &mut encoded, &Vector::new())
                .map_err(|error| error.to_string())?;
            Ok(IsolationStep {
                name: stage.name,
                preview_image_data_url: format!(
                    "data:image/png;base64,{}",
                    BASE64.encode(encoded.as_slice())
                ),
                elapsed_micros: stage.elapsed_micros,
            })
        })
        .collect()
}
