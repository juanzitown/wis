use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use leptess::LepTess;
use opencv::{
    core::{self, Mat, Rect, Scalar, Size, Vector},
    imgcodecs, imgproc,
    prelude::*,
};
use wis_core::Region;

pub fn recognize_region(
    tesseract: &mut LepTess,
    frame: &[u8],
    frame_size: (u32, u32),
    logical_size: (u32, u32),
    region: Region,
) -> (String, i32, Option<String>, Option<String>) {
    let (encoded, preview) = match preprocess_region(frame, frame_size, logical_size, region) {
        Ok(images) => images,
        Err(error) => return (String::new(), 0, None, Some(error)),
    };
    let preview_image_data_url = Some(format!(
        "data:image/png;base64,{}",
        BASE64.encode(preview.as_slice())
    ));
    let result = (|| -> Result<(String, i32), String> {
        tesseract
            .set_image_from_mem(encoded.as_slice())
            .map_err(|error| error.to_string())?;
        tesseract.set_source_resolution(300);
        let raw = tesseract
            .get_utf8_text()
            .map_err(|error| error.to_string())?;
        let text = raw
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        Ok((text, tesseract.mean_text_conf()))
    })();
    match result {
        Ok((text, confidence)) => (text, confidence, preview_image_data_url, None),
        Err(error) => (String::new(), 0, preview_image_data_url, Some(error)),
    }
}

fn preprocess_region(
    frame: &[u8],
    frame_size: (u32, u32),
    logical_size: (u32, u32),
    region: Region,
) -> Result<(Vector<u8>, Vector<u8>), String> {
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
    let left = scale(region.x, frame_width, logical_width).min(frame_width);
    let top = scale(region.y, frame_height, logical_height).min(frame_height);
    let right = scale(
        region.x.saturating_add(region.width),
        frame_width,
        logical_width,
    )
    .min(frame_width);
    let bottom = scale(
        region.y.saturating_add(region.height),
        frame_height,
        logical_height,
    )
    .min(frame_height);
    if right <= left || bottom <= top {
        return Err("the text area is outside the captured screen".to_owned());
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

    let mut enlarged = Mat::default();
    imgproc::resize(
        &roi,
        &mut enlarged,
        Size::new(roi.cols() * 2, roi.rows() * 2),
        0.0,
        0.0,
        imgproc::INTER_CUBIC,
    )
    .map_err(|error| error.to_string())?;
    let mut gray = Mat::default();
    imgproc::cvt_color_def(&enlarged, &mut gray, imgproc::COLOR_BGRA2GRAY)
        .map_err(|error| error.to_string())?;
    let mut binary = Mat::default();
    imgproc::threshold(
        &gray,
        &mut binary,
        0.0,
        255.0,
        imgproc::THRESH_BINARY | imgproc::THRESH_OTSU,
    )
    .map_err(|error| error.to_string())?;
    if core::count_non_zero(&binary).map_err(|error| error.to_string())?
        < (binary.rows() * binary.cols()) / 2
    {
        let mut inverted = Mat::default();
        core::bitwise_not(&binary, &mut inverted, &core::no_array())
            .map_err(|error| error.to_string())?;
        binary = inverted;
    }

    let mut padded = Mat::default();
    core::copy_make_border(
        &binary,
        &mut padded,
        16,
        16,
        16,
        16,
        core::BORDER_CONSTANT,
        Scalar::all(255.0),
    )
    .map_err(|error| error.to_string())?;
    let mut encoded = Vector::new();
    imgcodecs::imencode(".png", &padded, &mut encoded, &Vector::new())
        .map_err(|error| error.to_string())?;

    let preview_scale = (480.0 / padded.cols() as f64)
        .min(160.0 / padded.rows() as f64)
        .min(1.0);
    let preview_size = Size::new(
        ((padded.cols() as f64 * preview_scale).round() as i32).max(1),
        ((padded.rows() as f64 * preview_scale).round() as i32).max(1),
    );
    let mut preview_mat = Mat::default();
    imgproc::resize(
        &padded,
        &mut preview_mat,
        preview_size,
        0.0,
        0.0,
        imgproc::INTER_AREA,
    )
    .map_err(|error| error.to_string())?;
    let mut preview = Vector::new();
    imgcodecs::imencode(".png", &preview_mat, &mut preview, &Vector::new())
        .map_err(|error| error.to_string())?;
    Ok((encoded, preview))
}
