//! Local PP-OCRv5 inference for image attachments.
//!
//! The model files are intentionally kept outside the Rust binary. Tauri bundles
//! them as resources and this module loads the matching ONNX Runtime DLL from
//! that same directory on first use.

use serde::Serialize;
#[cfg(not(windows))]
use std::path::Path;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrResult {
    pub text: String,
    pub line_count: usize,
    pub duration_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    #[error("local OCR is only available on Windows")]
    UnsupportedPlatform,
    #[error("invalid image data: {0}")]
    InvalidImage(String),
    #[error("OCR resources are unavailable: {0}")]
    Resources(String),
    #[error("OCR inference failed: {0}")]
    Inference(String),
}

#[cfg(not(windows))]
pub fn recognize_data_url(_data_url: &str, _resource_dir: &Path) -> Result<OcrResult, OcrError> {
    Err(OcrError::UnsupportedPlatform)
}

#[cfg(windows)]
mod windows_backend {
    use super::{OcrError, OcrResult};
    use base64::Engine as _;
    use image::{DynamicImage, GenericImageView, imageops::FilterType};
    use ndarray::Array4;
    use ort::{session::Session, value::TensorRef};
    use std::cmp::{max, min};
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
    const DET_LIMIT_SIDE: u32 = 960;
    const DET_THRESHOLD: f32 = 0.30;
    const REC_HEIGHT: u32 = 48;
    const REC_MAX_WIDTH: u32 = 320;

    struct OcrEngine {
        det: Session,
        rec: Session,
        vocab: Vec<String>,
    }

    static ENGINE: OnceLock<Result<Mutex<OcrEngine>, String>> = OnceLock::new();

    pub fn recognize_data_url(data_url: &str, resource_dir: &Path) -> Result<OcrResult, OcrError> {
        let started = Instant::now();
        let bytes = decode_data_url(data_url)?;
        let image = image::load_from_memory(&bytes)
            .map_err(|error| OcrError::InvalidImage(error.to_string()))?;
        let engine = ENGINE.get_or_init(|| {
            OcrEngine::new(resource_dir)
                .map(Mutex::new)
                .map_err(|error| error.to_string())
        });
        let engine = engine
            .as_ref()
            .map_err(|error| OcrError::Resources(error.clone()))?;
        let mut engine = engine
            .lock()
            .map_err(|_| OcrError::Inference("OCR engine lock is poisoned".into()))?;
        let vocab = engine.vocab.clone();
        let boxes = detect_boxes(&mut engine.det, &image)?;
        let mut lines = Vec::new();
        for text_box in boxes {
            let crop = image.crop_imm(text_box.x, text_box.y, text_box.width, text_box.height);
            let (text, confidence) = recognize_crop(&mut engine.rec, &vocab, &crop)?;
            if !text.is_empty() && confidence >= 0.25 {
                lines.push(text);
            }
        }
        if lines.is_empty() {
            let (text, confidence) = recognize_crop(&mut engine.rec, &vocab, &image)?;
            if !text.is_empty() && confidence >= 0.25 {
                lines.push(text);
            }
        }
        Ok(OcrResult {
            line_count: lines.len(),
            text: lines.join("\n"),
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }

    impl OcrEngine {
        fn new(resource_dir: &Path) -> Result<Self, OcrError> {
            let runtime = resource_dir.join("onnxruntime.dll");
            let detection = resource_dir.join("ppocrv5_mobile_det.onnx");
            let recognition = resource_dir.join("ppocrv5_mobile_rec.onnx");
            let vocabulary = resource_dir.join("ppocrv5_mobile_vocab.txt");
            for path in [&runtime, &detection, &recognition, &vocabulary] {
                if !path.is_file() {
                    return Err(OcrError::Resources(path.display().to_string()));
                }
            }
            ort::init_from(runtime)
                .map_err(|error| OcrError::Resources(error.to_string()))?
                .commit();
            let det = Session::builder()
                .map_err(|error| OcrError::Inference(error.to_string()))?
                .commit_from_file(detection)
                .map_err(|error| OcrError::Inference(error.to_string()))?;
            let rec = Session::builder()
                .map_err(|error| OcrError::Inference(error.to_string()))?
                .commit_from_file(recognition)
                .map_err(|error| OcrError::Inference(error.to_string()))?;
            let vocab = std::fs::read_to_string(vocabulary)
                .map_err(|error| OcrError::Resources(error.to_string()))?
                .lines()
                .map(ToOwned::to_owned)
                .collect();
            Ok(Self { det, rec, vocab })
        }
    }

    fn decode_data_url(data_url: &str) -> Result<Vec<u8>, OcrError> {
        let (header, encoded) = data_url
            .split_once(',')
            .ok_or_else(|| OcrError::InvalidImage("image must be a base64 data URL".into()))?;
        let mime = header
            .strip_prefix("data:")
            .and_then(|value| value.strip_suffix(";base64"))
            .ok_or_else(|| OcrError::InvalidImage("image data URL must use base64".into()))?;
        if !matches!(
            mime,
            "image/png"
                | "image/jpeg"
                | "image/jpg"
                | "image/gif"
                | "image/webp"
                | "image/bmp"
                | "image/x-icon"
        ) {
            return Err(OcrError::InvalidImage(format!(
                "unsupported image type {mime}"
            )));
        }
        if encoded.len() > MAX_IMAGE_BYTES.saturating_mul(4).div_ceil(3) + 16 {
            return Err(OcrError::InvalidImage(
                "image exceeds the 4 MiB OCR limit".into(),
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| OcrError::InvalidImage(error.to_string()))?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(OcrError::InvalidImage(
                "image exceeds the 4 MiB OCR limit".into(),
            ));
        }
        Ok(bytes)
    }

    #[derive(Clone, Copy)]
    struct TextBox {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    }

    fn detect_boxes(session: &mut Session, image: &DynamicImage) -> Result<Vec<TextBox>, OcrError> {
        let (original_width, original_height) = image.dimensions();
        let scale = (DET_LIMIT_SIDE as f32 / max(original_width, original_height) as f32).min(1.0);
        let scaled_width = max(32, ((original_width as f32 * scale) as u32 / 32) * 32);
        let scaled_height = max(32, ((original_height as f32 * scale) as u32 / 32) * 32);
        let resized = image
            .resize_exact(scaled_width, scaled_height, FilterType::Triangle)
            .to_rgb8();
        let tensor = image_tensor(
            &resized,
            scaled_width as usize,
            scaled_height as usize,
            true,
        );
        let outputs = session
            .run(ort::inputs![
                TensorRef::from_array_view(&tensor)
                    .map_err(|error| OcrError::Inference(error.to_string()))?
            ])
            .map_err(|error| OcrError::Inference(error.to_string()))?;
        let (_, output) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|error| OcrError::Inference(error.to_string()))?;
        let shape = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|error| OcrError::Inference(error.to_string()))?
            .0;
        if shape.len() != 4 || shape[0] != 1 || shape[1] != 1 {
            return Err(OcrError::Inference(format!(
                "unexpected detector output shape {shape:?}"
            )));
        }
        let map_height = usize::try_from(shape[2]).unwrap_or(0);
        let map_width = usize::try_from(shape[3]).unwrap_or(0);
        if map_height == 0 || map_width == 0 || output.len() < map_height * map_width {
            return Ok(Vec::new());
        }
        let mut probabilities = output[..map_height * map_width].to_vec();
        if probabilities.iter().copied().fold(0.0_f32, f32::max) > 1.0 {
            for value in &mut probabilities {
                *value = 1.0 / (1.0 + (-*value).exp());
            }
        }
        let mut visited = vec![false; probabilities.len()];
        let mut boxes = Vec::new();
        for start in 0..probabilities.len() {
            if visited[start] || probabilities[start] < DET_THRESHOLD {
                continue;
            }
            let mut queue = VecDeque::from([start]);
            visited[start] = true;
            let mut count = 0usize;
            let mut min_x = map_width;
            let mut max_x = 0usize;
            let mut min_y = map_height;
            let mut max_y = 0usize;
            let mut score = 0.0_f32;
            while let Some(index) = queue.pop_front() {
                let x = index % map_width;
                let y = index / map_width;
                count += 1;
                score += probabilities[index];
                min_x = min(min_x, x);
                max_x = max(max_x, x);
                min_y = min(min_y, y);
                max_y = max(max_y, y);
                for (next_x, next_y) in neighbors(x, y, map_width, map_height) {
                    let next = next_y * map_width + next_x;
                    if !visited[next] && probabilities[next] >= DET_THRESHOLD {
                        visited[next] = true;
                        queue.push_back(next);
                    }
                }
            }
            if count < 3 || score / (count as f32) < DET_THRESHOLD {
                continue;
            }
            let x0 = (min_x as f32 * scaled_width as f32 / map_width as f32) as u32;
            let y0 = (min_y as f32 * scaled_height as f32 / map_height as f32) as u32;
            let x1 = ((max_x + 1) as f32 * scaled_width as f32 / map_width as f32) as u32;
            let y1 = ((max_y + 1) as f32 * scaled_height as f32 / map_height as f32) as u32;
            let x0 = (x0.saturating_sub(2) as f32 / scale) as u32;
            let y0 = (y0.saturating_sub(2) as f32 / scale) as u32;
            let x1 = min(original_width, ((x1 + 2) as f32 / scale) as u32);
            let y1 = min(original_height, ((y1 + 2) as f32 / scale) as u32);
            if x1 > x0 && y1 > y0 {
                boxes.push(TextBox {
                    x: x0,
                    y: y0,
                    width: x1 - x0,
                    height: y1 - y0,
                });
            }
        }
        boxes.sort_by_key(|text_box| (text_box.y, text_box.x));
        merge_boxes(boxes)
    }

    fn neighbors(x: usize, y: usize, width: usize, height: usize) -> [(usize, usize); 8] {
        [
            (x.saturating_sub(1), y.saturating_sub(1)),
            (x, y.saturating_sub(1)),
            (min(width.saturating_sub(1), x + 1), y.saturating_sub(1)),
            (x.saturating_sub(1), y),
            (min(width.saturating_sub(1), x + 1), y),
            (x.saturating_sub(1), min(height.saturating_sub(1), y + 1)),
            (x, min(height.saturating_sub(1), y + 1)),
            (
                min(width.saturating_sub(1), x + 1),
                min(height.saturating_sub(1), y + 1),
            ),
        ]
    }

    fn merge_boxes(mut boxes: Vec<TextBox>) -> Result<Vec<TextBox>, OcrError> {
        let mut merged = Vec::new();
        for current in boxes.drain(..) {
            if let Some(previous) = merged.iter_mut().find(|previous: &&mut TextBox| {
                let vertical_overlap =
                    min(previous.y + previous.height, current.y + current.height)
                        .saturating_sub(max(previous.y, current.y));
                let gap = current.x.saturating_sub(previous.x + previous.width);
                vertical_overlap as f32 >= min(previous.height, current.height) as f32 * 0.35
                    && gap <= max(previous.height, current.height) * 2
            }) {
                let x0 = min(previous.x, current.x);
                let y0 = min(previous.y, current.y);
                let x1 = max(previous.x + previous.width, current.x + current.width);
                let y1 = max(previous.y + previous.height, current.y + current.height);
                *previous = TextBox {
                    x: x0,
                    y: y0,
                    width: x1 - x0,
                    height: y1 - y0,
                };
            } else {
                merged.push(current);
            }
        }
        Ok(merged)
    }

    fn recognize_crop(
        session: &mut Session,
        vocab: &[String],
        image: &DynamicImage,
    ) -> Result<(String, f32), OcrError> {
        let rgb = image.to_rgb8();
        let (width, height) = rgb.dimensions();
        if width == 0 || height == 0 {
            return Ok((String::new(), 0.0));
        }
        let target_width = ((width as f32 / height as f32) * REC_HEIGHT as f32)
            .ceil()
            .clamp(8.0, REC_MAX_WIDTH as f32) as u32;
        let resized = DynamicImage::ImageRgb8(rgb)
            .resize_exact(target_width, REC_HEIGHT, FilterType::Triangle)
            .to_rgb8();
        let tensor = image_tensor(&resized, target_width as usize, REC_HEIGHT as usize, false);
        let outputs = session
            .run(ort::inputs![
                TensorRef::from_array_view(&tensor)
                    .map_err(|error| OcrError::Inference(error.to_string()))?
            ])
            .map_err(|error| OcrError::Inference(error.to_string()))?;
        let (shape, output) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|error| OcrError::Inference(error.to_string()))?;
        if shape.len() != 3 || shape[0] != 1 {
            return Err(OcrError::Inference(format!(
                "unexpected recognizer output shape {shape:?}"
            )));
        }
        let time = usize::try_from(shape[1]).unwrap_or(0);
        let classes = usize::try_from(shape[2]).unwrap_or(0);
        if time == 0 || classes == 0 || output.len() < time * classes {
            return Ok((String::new(), 0.0));
        }
        let blank = classes.saturating_sub(1);
        let mut previous = usize::MAX;
        let mut text = String::new();
        let mut confidence = 0.0_f32;
        for step in 0..time {
            let row = &output[step * classes..(step + 1) * classes];
            let (index, score) = row
                .iter()
                .copied()
                .enumerate()
                .max_by(|left, right| left.1.total_cmp(&right.1))
                .unwrap_or((blank, 0.0));
            if index != blank && index != previous && index < vocab.len() {
                text.push_str(&vocab[index]);
            }
            if index != blank {
                confidence += score;
            }
            previous = index;
        }
        let non_blank = text.chars().count().max(1) as f32;
        Ok((text.trim().to_string(), confidence / non_blank))
    }

    fn image_tensor(
        image: &image::RgbImage,
        width: usize,
        height: usize,
        detector: bool,
    ) -> Array4<f32> {
        let mut tensor = Array4::<f32>::zeros((1, 3, height, width));
        for y in 0..height {
            for x in 0..width {
                let pixel = image.get_pixel(x as u32, y as u32).0;
                for channel in 0..3 {
                    let value = pixel[channel] as f32 / 255.0;
                    tensor[[0, channel, y, x]] = if detector {
                        (value - [0.485, 0.456, 0.406][channel]) / [0.229, 0.224, 0.225][channel]
                    } else {
                        value * 2.0 - 1.0
                    };
                }
            }
        }
        tensor
    }

    #[cfg(test)]
    mod tests {
        use super::decode_data_url;

        #[test]
        fn rejects_non_image_and_non_base64_payloads() {
            assert!(decode_data_url("data:text/plain;base64,SGk=").is_err());
            assert!(decode_data_url("data:image/png,not-base64").is_err());
        }

        #[test]
        fn rejects_oversized_image_payloads_before_decoding() {
            let encoded = "A".repeat(5_600_000);
            let result = decode_data_url(&format!("data:image/png;base64,{encoded}"));
            assert!(result.is_err());
        }
    }
}

#[cfg(windows)]
pub use windows_backend::recognize_data_url;
