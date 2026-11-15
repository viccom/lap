/**
 * Face Recognition module
 * Handles face detection (RetinaFace) and embedding (MobileFaceNet) using ONNX Runtime.
 */
use crate::{t_cluster, t_common, t_sqlite};
use image::DynamicImage;
use ndarray::Array;
use ort::{
    inputs,
    session::{Session, builder::GraphOptimizationLevel},
    value::Value,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

// cancellation token for face indexing
#[derive(Clone)]
pub struct FaceIndexCancellation(pub Arc<Mutex<bool>>);

// detailed status for face indexing
#[derive(Clone)]
pub struct FaceIndexingStatus(pub Arc<Mutex<bool>>);

// face indexing progress
#[derive(Clone, serde::Serialize)]
pub struct FaceIndexProgress {
    pub current: usize,
    pub total: usize,
    pub faces_found: usize,
    pub phase: String,
}

#[derive(Clone)]
pub struct FaceIndexProgressState(pub Arc<Mutex<FaceIndexProgress>>);

// face stats
#[derive(Clone, serde::Serialize)]
pub struct FaceStats {
    pub total: usize,
    pub processed: usize,
    pub unprocessed: usize,
    pub faces: usize,
}

/// Detected face bounding box and landmarks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub confidence: f32,
    pub landmarks: Option<Vec<(f32, f32)>>, // 5 facial landmarks
}

/// Face with embedding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceData {
    pub bbox: FaceBox,
    pub embedding: Vec<f32>,
}

struct Anchor {
    cx: f32,
    cy: f32,
}

pub struct FaceEngine {
    detection_model: Option<Session>, // RetinaFace
    embedding_model: Option<Session>, // MobileFaceNet
}

impl FaceEngine {
    pub fn new() -> Self {
        Self {
            detection_model: None,
            embedding_model: None,
        }
    }

    pub fn load_models(&mut self, app: &AppHandle) -> Result<(), String> {
        if self.detection_model.is_some() {
            return Ok(());
        }

        // Resolve paths
        let resource_dir = app
            .path()
            .resolve("models", tauri::path::BaseDirectory::Resource)
            .map_err(|e| format!("Failed to resolve resource path: {}", e))?;

        self.load_models_from_dir(&resource_dir)
    }

    /// Load both sessions from a models directory. Split from `load_models`
    /// so tests (and later a headless server) can load the models without an
    /// `AppHandle`.
    pub fn load_models_from_dir(&mut self, models_dir: &std::path::Path) -> Result<(), String> {
        if self.detection_model.is_some() {
            return Ok(());
        }

        let detection_model_path = models_dir.join(t_common::DETECTION_MODEL);
        let embedding_model_path = models_dir.join(t_common::EMBEDDING_MODEL);

        // Check if models exist
        if !detection_model_path.exists() {
            return Err(format!(
                "Detection model not found at {:?}",
                detection_model_path
            ));
        }
        if !embedding_model_path.exists() {
            return Err(format!(
                "Embedding model not found at {:?}",
                embedding_model_path
            ));
        }

        // Load Detection Model (RetinaFace)
        let detection_model = Session::builder()
            .map_err(|e| e.to_string())?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| e.to_string())?
            .with_intra_threads(4)
            .map_err(|e| e.to_string())?
            .commit_from_file(&detection_model_path)
            .map_err(|e| format!("Failed to load detection model: {}", e))?;

        self.detection_model = Some(detection_model);

        // Load Embedding Model (MobileFaceNet)
        let embedding_model = Session::builder()
            .map_err(|e| e.to_string())?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| e.to_string())?
            .with_intra_threads(4)
            .map_err(|e| e.to_string())?
            .commit_from_file(&embedding_model_path)
            .map_err(|e| format!("Failed to load embedding model: {}", e))?;

        self.embedding_model = Some(embedding_model);

        Ok(())
    }

    pub fn is_loaded(&self) -> bool {
        self.detection_model.is_some() && self.embedding_model.is_some()
    }

    /// Detect faces implementation (from DynamicImage)
    fn detect_faces(&mut self, img: &DynamicImage) -> Result<Vec<FaceBox>, String> {
        let original_width = img.width() as f32;
        let original_height = img.height() as f32;

        // RetinaFace typically expects 640x640 input, but works with any size divisible by 32 (stride 32).
        // Optimization: For small images (like thumbnails ~512px), use their native size slightly rounded up.
        // For large images, downscale to 640px max dimension.
        let max_dim = original_width.max(original_height);
        let target_size = if max_dim < 640.0 {
            // Round up to nearest multiple of 32
            ((max_dim as u32 + 31) / 32) * 32
        } else {
            640
        };
        // Resize preserving aspect ratio (Letterbox)
        // Use max dimension to fit within target
        let scale = (target_size as f32) / original_width.max(original_height);
        // Use round() to minimize truncation error
        let new_w = (original_width * scale).round() as u32;
        let new_h = (original_height * scale).round() as u32;

        let rgb_buf; // Owned buffer if needed
        let rgb_img = if new_w == img.width() && new_h == img.height() {
            // Optimization: Skip resize if unnecessary
            if let Some(buf) = img.as_rgb8() {
                buf
            } else {
                rgb_buf = img.to_rgb8();
                &rgb_buf
            }
        } else {
            rgb_buf = img
                .resize_exact(new_w, new_h, image::imageops::FilterType::Triangle)
                .into_rgb8();
            &rgb_buf
        };

        // Standard InsightFace/RetinaFace preprocessing aligns to Top-Left (0,0)

        // Normalize: (pixel - 127.5) / 128.0
        // Initialize with zeros (padding)
        let mut array = Array::zeros((1, 3, target_size as usize, target_size as usize));

        if let Some(slice) = array.as_slice_mut() {
            let area = (target_size as usize) * (target_size as usize);
            let offset_b = 0;
            let offset_g = area;
            let offset_r = area * 2;
            let target_w = target_size as usize;

            for (x, y, pixel) in rgb_img.enumerate_pixels() {
                let r = (pixel[0] as f32 - 127.5) / 128.0;
                let g = (pixel[1] as f32 - 127.5) / 128.0;
                let b = (pixel[2] as f32 - 127.5) / 128.0;

                let idx = (y as usize) * target_w + (x as usize);

                slice[offset_b + idx] = b;
                slice[offset_g + idx] = g;
                slice[offset_r + idx] = r;
            }
        } else {
            // Fallback if array is not contiguous (should not happen with default init)
            for (x, y, pixel) in rgb_img.enumerate_pixels() {
                let r = (pixel[0] as f32 - 127.5) / 128.0;
                let g = (pixel[1] as f32 - 127.5) / 128.0;
                let b = (pixel[2] as f32 - 127.5) / 128.0;

                array[[0, 0, y as usize, x as usize]] = b; // Blue
                array[[0, 1, y as usize, x as usize]] = g; // Green
                array[[0, 2, y as usize, x as usize]] = r; // Red
            }
        }

        let input_value = Value::from_array(array).map_err(|e| e.to_string())?;

        // Use block scope to ensure outputs is dropped before calling nms
        let mut faces = {
            let outputs = self
                .detection_model
                .as_mut()
                .unwrap()
                .run(inputs!["input.1" => input_value])
                .map_err(|e| format!("Detection inference error: {}", e))?;

            let mut all_detections = Vec::new();
            let strides = [8, 16, 32];
            let min_sizes = [[16, 32], [64, 128], [256, 512]]; // Standard RetinaFace config

            // Map output indices based on observation
            // Scores, Boxes, Landmarks indices per stride
            let indices = [
                (0, 3, 6), // Stride 8
                (1, 4, 7), // Stride 16
                (2, 5, 8), // Stride 32
            ];

            let confidence_threshold = 0.6;

            for (i, &stride) in strides.iter().enumerate() {
                let (score_idx, box_idx, kps_idx) = indices[i];

                let scores_tensor = &outputs[score_idx];
                let boxes_tensor = &outputs[box_idx];
                let kps_tensor = &outputs[kps_idx];

                let (_, scores_data) = scores_tensor
                    .try_extract_tensor::<f32>()
                    .map_err(|e| format!("Failed stride {} scores: {}", stride, e))?;
                let (_, boxes_data) = boxes_tensor
                    .try_extract_tensor::<f32>()
                    .map_err(|e| format!("Failed stride {} boxes: {}", stride, e))?;
                let (_, kps_data) = kps_tensor
                    .try_extract_tensor::<f32>()
                    .map_err(|e| format!("Failed stride {} kps: {}", stride, e))?;

                let feature_map_w = target_size / stride;
                let feature_map_h = target_size / stride;
                let anchors =
                    Self::generate_anchors(stride, &min_sizes[i], feature_map_w, feature_map_h);

                for (j, anchor) in anchors.iter().enumerate() {
                    let score = scores_data[j];
                    if score < confidence_threshold {
                        continue;
                    }

                    // Decode box: [l, t, r, b] (distances from center, normalized by stride)
                    // This assumes SCRFD model (det_10g.onnx) which outputs distances
                    let l = boxes_data[j * 4];
                    let t = boxes_data[j * 4 + 1];
                    let r = boxes_data[j * 4 + 2];
                    let b = boxes_data[j * 4 + 3];

                    // SCRFD uses stride-scaled distances
                    // x1 = cx - l * stride
                    // y1 = cy - t * stride
                    // x2 = cx + r * stride
                    // y2 = cy + b * stride

                    let x1 = anchor.cx - l * stride as f32;
                    let y1 = anchor.cy - t * stride as f32;
                    let x2 = anchor.cx + r * stride as f32;
                    let y2 = anchor.cy + b * stride as f32;

                    // Clamp to the letterboxed input before mapping back.
                    let limit = target_size as f32;
                    let x1 = x1.clamp(0.0, limit);
                    let y1 = y1.clamp(0.0, limit);
                    let x2 = x2.clamp(0.0, limit);
                    let y2 = y2.clamp(0.0, limit);

                    // Scale back to original image
                    // Use effective scale factors derived from actual resized dimensions
                    let inv_scale_x = original_width / new_w as f32;
                    let inv_scale_y = original_height / new_h as f32;

                    // Scale directly (no padding offset), clamped to the image bounds.
                    let original_x1 = (x1 * inv_scale_x).clamp(0.0, original_width);
                    let original_y1 = (y1 * inv_scale_y).clamp(0.0, original_height);
                    let original_x2 = (x2 * inv_scale_x).clamp(0.0, original_width);
                    let original_y2 = (y2 * inv_scale_y).clamp(0.0, original_height);

                    // Decode the 5 landmarks (SCRFD kps output: two stride-scaled
                    // offsets per point from the anchor center). They feed the
                    // 112x112 alignment warp before embedding.
                    let mut landmarks = Vec::with_capacity(5);
                    for point in 0..5 {
                        let dx = kps_data[j * 10 + point * 2];
                        let dy = kps_data[j * 10 + point * 2 + 1];
                        let px = ((anchor.cx + dx * stride as f32) * inv_scale_x)
                            .clamp(0.0, original_width);
                        let py = ((anchor.cy + dy * stride as f32) * inv_scale_y)
                            .clamp(0.0, original_height);
                        landmarks.push((px, py));
                    }

                    all_detections.push(FaceBox {
                        x: original_x1,
                        y: original_y1,
                        width: original_x2 - original_x1,
                        height: original_y2 - original_y1,
                        confidence: score,
                        landmarks: Some(landmarks),
                    });
                }
            }

            all_detections
        };

        // Non-maximum suppression
        faces = self.nms(faces, 0.4);

        if faces.is_empty() {
            // No faces found after NMS
        }

        Ok(faces)
    }

    /// Generate anchors for a specific stride
    fn generate_anchors(
        stride: u32,
        min_sizes: &[u32],
        feature_w: u32,
        feature_h: u32,
    ) -> Vec<Anchor> {
        let mut anchors =
            Vec::with_capacity((feature_w * feature_h * min_sizes.len() as u32) as usize);

        for y in 0..feature_h {
            for x in 0..feature_w {
                for &_min_size in min_sizes {
                    // Dense anchor centers
                    // Adjusted to 0.0 (top-left) from 0.5 (center) to fix systematic bottom-right shift
                    let cx = (x as f32) * stride as f32;
                    let cy = (y as f32) * stride as f32;

                    anchors.push(Anchor { cx, cy });
                }
            }
        }
        anchors
    }

    /// Get face embedding implementation (from DynamicImage).
    ///
    /// Prefers the 5-point aligned crop (the InsightFace standard the
    /// w600k_mbf model was trained with); falls back to the padded bbox crop
    /// when landmarks are missing.
    fn get_face_embedding(
        &mut self,
        img: &DynamicImage,
        bbox: &FaceBox,
    ) -> Result<Vec<f32>, String> {
        const TARGET: u32 = 112;

        let rgb_face: image::RgbImage = match bbox.landmarks.as_ref().filter(|k| k.len() == 5) {
            Some(kps) => {
                let src = [kps[0], kps[1], kps[2], kps[3], kps[4]];
                aligned_face_crop(img, &src).ok_or("Failed to align face crop")?
            }
            None => {
                // Fallback: crop face region with some padding, resize to 112x112.
                let padding = 0.2;
                let x = (bbox.x - bbox.width * padding).max(0.0) as u32;
                let y = (bbox.y - bbox.height * padding).max(0.0) as u32;
                let w = (bbox.width * (1.0 + 2.0 * padding)) as u32;
                let h = (bbox.height * (1.0 + 2.0 * padding)) as u32;

                let max_x = (x + w).min(img.width());
                let max_y = (y + h).min(img.height());

                img.crop_imm(x, y, max_x - x, max_y - y)
                    .resize_exact(TARGET, TARGET, image::imageops::FilterType::Triangle)
                    .into_rgb8()
            }
        };

        // The embedding model is BGR-trained (same preprocessing as the
        // detection model); it used to be fed RGB here, which systematically
        // degraded embeddings and split same-person clusters.
        let array = embedding_input_bgr(&rgb_face);
        let input_value = Value::from_array(array).map_err(|e| e.to_string())?;

        let outputs = self
            .embedding_model
            .as_mut()
            .unwrap()
            .run(inputs!["input.1" => input_value])
            .map_err(|e| format!("Embedding inference error: {}", e))?;

        let embedding = &outputs[0];
        let (_, embedding_data) = embedding
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("Failed to extract embedding: {}", e))?;

        // Normalize embedding to unit vector
        let emb_vec = embedding_data.to_vec();
        let norm: f32 = emb_vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if !norm.is_finite() || norm <= f32::EPSILON {
            return Err("Invalid face embedding norm".to_string());
        }
        let normalized: Vec<f32> = emb_vec.iter().map(|x| x / norm).collect();

        Ok(normalized)
    }

    /// Compute cosine similarity between two embeddings
    #[allow(dead_code)]
    pub fn compare_faces(emb1: &[f32], emb2: &[f32]) -> f32 {
        if emb1.len() != emb2.len() {
            return 0.0;
        }
        // Embeddings are already normalized, so dot product = cosine similarity
        emb1.iter().zip(emb2.iter()).map(|(a, b)| a * b).sum()
    }

    /// Process image: detect all faces and get embeddings
    /// Filters out low-quality faces (low confidence, small size, blurry)
    pub fn process_image(
        &mut self,
        image_path: &str,
    ) -> Result<(Vec<FaceData>, (u32, u32)), String> {
        let img = image::open(image_path).map_err(|e| format!("Failed to open image: {}", e))?;
        // `image::open` does NOT apply EXIF orientation. Rotate to display
        // space so bboxes land in the same space as the thumbnail path and
        // every consumer (stored dims are display dims).
        let orientation = crate::t_image::get_image_orientation(image_path);
        let img = crate::t_image::apply_orientation(img, orientation);
        self.process_dynamic_image(&img)
    }

    pub fn process_image_from_bytes(
        &mut self,
        image_bytes: &[u8],
    ) -> Result<(Vec<FaceData>, (u32, u32)), String> {
        let img = image::load_from_memory(image_bytes)
            .map_err(|e| format!("Failed to load image from memory: {}", e))?;
        self.process_dynamic_image(&img)
    }

    fn process_dynamic_image(
        &mut self,
        img: &DynamicImage,
    ) -> Result<(Vec<FaceData>, (u32, u32)), String> {
        let faces = self.detect_faces(img)?;

        let mut results = Vec::new();
        for face in faces {
            // Filter 1: Skip low confidence faces
            if face.confidence < t_common::MIN_CONFIDENCE {
                continue;
            }

            // Filter 3: Skip faces smaller than the minimum size, measured in
            // SOURCE-image pixels — the alignment warp samples the source
            // image, so embedding quality is bound by the real face size, not
            // the 640px detection input. (An input-space gate here wrongly
            // rejected faces on large originals that downscale to ~50px in the
            // 640 input.) Tiny thumbnail faces used to embed noise and became
            // singleton garbage persons.
            if face.width < t_common::MIN_FACE_SIZE || face.height < t_common::MIN_FACE_SIZE {
                continue;
            }

            // Filter 4: Skip blurry faces
            let blur_score = self.calculate_blur_score(img, &face);
            if blur_score < t_common::MIN_BLUR_SCORE {
                continue;
            }

            // Get embedding for quality face
            let embedding = self.get_face_embedding(img, &face)?;
            results.push(FaceData {
                bbox: face,
                embedding,
            });
        }

        Ok((results, (img.width(), img.height())))
    }

    /// Calculate blur score using Variance of Laplacian
    /// Optimized: Uses Welford's online algorithm to avoid allocating a large vector
    fn calculate_blur_score(&self, img: &DynamicImage, bbox: &FaceBox) -> f32 {
        let x = bbox.x.max(0.0) as u32;
        let y = bbox.y.max(0.0) as u32;
        // Check bounds to ensure we don't crash on cropping
        let w = bbox.width.min(img.width() as f32 - bbox.x) as u32;
        let h = bbox.height.min(img.height() as f32 - bbox.y) as u32;

        if w < 3 || h < 3 {
            return 0.0;
        }

        let crop = img.crop_imm(x, y, w, h).to_luma8();
        // Normalize the crop scale before scoring. An absolute Laplacian
        // variance is resolution-dependent: the same face scored ~25 on a
        // 4272px original but passed on its 512px thumbnail, so the gate used
        // to silently reject everything detected via the original path (and
        // will also reject RAW faces). Measuring at a fixed long-side scale
        // gives thumbnail and original paths the same ruler.
        let crop = {
            const BLUR_MEASURE_LONG_SIDE: u32 = 256;
            let long = crop.width().max(crop.height());
            if long > BLUR_MEASURE_LONG_SIDE {
                let scale = BLUR_MEASURE_LONG_SIDE as f32 / long as f32;
                image::imageops::resize(
                    &crop,
                    ((crop.width() as f32) * scale).round().max(1.0) as u32,
                    ((crop.height() as f32) * scale).round().max(1.0) as u32,
                    image::imageops::FilterType::Triangle,
                )
            } else {
                crop
            }
        };
        let (width, height) = crop.dimensions();

        // Online variance calculation (Welford's algorithm)
        let mut count = 0usize;
        let mut m2 = 0.0;
        let mut mean = 0.0;

        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let p = crop.get_pixel(x, y).0[0] as i16;
                let top = crop.get_pixel(x, y - 1).0[0] as i16;
                let bottom = crop.get_pixel(x, y + 1).0[0] as i16;
                let left = crop.get_pixel(x - 1, y).0[0] as i16;
                let right = crop.get_pixel(x + 1, y).0[0] as i16;

                let sum = top + bottom + left + right - 4 * p;
                let val = sum as f32;

                count += 1;
                let delta = val - mean;
                mean += delta / count as f32;
                let delta2 = val - mean;
                m2 += delta * delta2;
            }
        }

        if count < 2 {
            return 0.0;
        }

        // Variance
        m2 / (count as f32)
    }

    /// Non-maximum suppression
    fn nms(&self, mut boxes: Vec<FaceBox>, iou_threshold: f32) -> Vec<FaceBox> {
        boxes.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));

        let mut keep = Vec::new();
        let mut suppressed = vec![false; boxes.len()];

        for i in 0..boxes.len() {
            if suppressed[i] {
                continue;
            }
            keep.push(boxes[i].clone());

            for j in (i + 1)..boxes.len() {
                if suppressed[j] {
                    continue;
                }
                if self.iou(&boxes[i], &boxes[j]) > iou_threshold {
                    suppressed[j] = true;
                }
            }
        }

        keep
    }

    /// Intersection over Union
    /// Optimized: Simplified redundant max(0.0) for valid boxes
    fn iou(&self, a: &FaceBox, b: &FaceBox) -> f32 {
        let x1 = a.x.max(b.x);
        let y1 = a.y.max(b.y);
        let x2 = (a.x + a.width).min(b.x + b.width);
        let y2 = (a.y + a.height).min(b.y + b.height);

        if x2 <= x1 || y2 <= y1 {
            return 0.0;
        }

        let inter_area = (x2 - x1) * (y2 - y1);
        let a_area = a.width * a.height;
        let b_area = b.width * b.height;

        inter_area / (a_area + b_area - inter_area)
    }
}

/// Standard InsightFace 112x112 five-point alignment template
/// (left eye, right eye, nose tip, left mouth corner, right mouth corner).
const ALIGN_TEMPLATE_112: [(f32, f32); 5] = [
    (38.2946, 51.6963),
    (73.5318, 51.5014),
    (56.0252, 71.7366),
    (41.5493, 92.3655),
    (70.7299, 92.2041),
];

/// Build the (1, 3, H, W) planar input the InsightFace models expect:
/// channel 0 is BLUE, values are (pixel - 127.5) / 128. The detection path
/// always wrote BGR; the embedding path used to write RGB, feeding the
/// BGR-trained w600k_mbf swapped channels and collapsing same-person
/// similarity to ~0.33.
fn embedding_input_bgr(rgb: &image::RgbImage) -> Array<f32, ndarray::Ix4> {
    let (width, height) = rgb.dimensions();
    let mut array = Array::zeros((1, 3, height as usize, width as usize));
    let area = (width as usize) * (height as usize);
    if let Some(slice) = array.as_slice_mut() {
        for (x, y, pixel) in rgb.enumerate_pixels() {
            let idx = (y as usize) * width as usize + x as usize;
            slice[idx] = (pixel[2] as f32 - 127.5) / 128.0; // Blue
            slice[area + idx] = (pixel[1] as f32 - 127.5) / 128.0; // Green
            slice[area * 2 + idx] = (pixel[0] as f32 - 127.5) / 128.0; // Red
        }
    }
    array
}

/// Least-squares similarity transform (uniform scale + rotation + translation)
/// mapping `src` onto `dst`. Returns (a, b, tx, ty) with
/// dst = (a*x - b*y + tx, b*x + a*y + ty); None for degenerate input
/// (all src points identical).
fn estimate_similarity_transform(
    src: &[(f32, f32); 5],
    dst: &[(f32, f32); 5],
) -> Option<(f32, f32, f32, f32)> {
    let n = src.len() as f64;
    let src_mean = (
        src.iter().map(|p| p.0 as f64).sum::<f64>() / n,
        src.iter().map(|p| p.1 as f64).sum::<f64>() / n,
    );
    let dst_mean = (
        dst.iter().map(|p| p.0 as f64).sum::<f64>() / n,
        dst.iter().map(|p| p.1 as f64).sum::<f64>() / n,
    );

    let mut denom = 0.0f64;
    let mut num_a = 0.0f64;
    let mut num_b = 0.0f64;
    for ((x, y), (u, v)) in src.iter().zip(dst.iter()) {
        let x = *x as f64 - src_mean.0;
        let y = *y as f64 - src_mean.1;
        let u = *u as f64 - dst_mean.0;
        let v = *v as f64 - dst_mean.1;
        denom += x * x + y * y;
        num_a += x * u + y * v;
        num_b += x * v - y * u;
    }
    if denom <= f64::EPSILON {
        return None;
    }
    let a = num_a / denom;
    let b = num_b / denom;
    let tx = dst_mean.0 - (a * src_mean.0 - b * src_mean.1);
    let ty = dst_mean.1 - (b * src_mean.0 + a * src_mean.1);
    Some((a as f32, b as f32, tx as f32, ty as f32))
}

/// Bilinear sample with black outside the image (alignment background).
fn sample_bilinear_rgb(img: &image::RgbImage, x: f32, y: f32) -> image::Rgb<u8> {
    let (w, h) = img.dimensions();
    if x < 0.0 || y < 0.0 || x > (w - 1) as f32 || y > (h - 1) as f32 {
        return image::Rgb([0, 0, 0]);
    }
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let mut out = [0u8; 3];
    for c in 0..3 {
        let p00 = img.get_pixel(x0, y0).0[c] as f32;
        let p10 = img.get_pixel(x1, y0).0[c] as f32;
        let p01 = img.get_pixel(x0, y1).0[c] as f32;
        let p11 = img.get_pixel(x1, y1).0[c] as f32;
        out[c] = (p00 * (1.0 - fx) * (1.0 - fy)
            + p10 * fx * (1.0 - fy)
            + p01 * (1.0 - fx) * fy
            + p11 * fx * fy) as u8;
    }
    image::Rgb(out)
}

/// Warp the image so the 5 landmarks land on the standard 112x112 InsightFace
/// template — the alignment w600k_mbf was trained with. Unaligned bbox crops
/// used to inflate intra-person variance and split one person into clusters.
fn aligned_face_crop(img: &DynamicImage, kps: &[(f32, f32); 5]) -> Option<image::RgbImage> {
    const TARGET: u32 = 112;
    // src -> template
    let (a, b, tx, ty) = estimate_similarity_transform(kps, &ALIGN_TEMPLATE_112)?;
    let denom = a * a + b * b;
    if denom < f32::EPSILON {
        return None;
    }
    let base = img.as_rgb8().cloned().unwrap_or_else(|| img.to_rgb8());
    let mut out = image::RgbImage::new(TARGET, TARGET);
    for oy in 0..TARGET {
        for ox in 0..TARGET {
            // inverse mapping: template -> source
            let dx = ox as f32 - tx;
            let dy = oy as f32 - ty;
            let u = (a * dx + b * dy) / denom;
            let v = (-b * dx + a * dy) / denom;
            out.put_pixel(ox, oy, sample_bilinear_rgb(&base, u, v));
        }
    }
    Some(out)
}

#[derive(Clone)]
pub struct FaceState(pub std::sync::Arc<Mutex<FaceEngine>>);

pub fn run_face_indexing(
    app_handle: AppHandle,
    face_state: FaceState,
    cancel_token_struct: FaceIndexCancellation,
    status_token_struct: FaceIndexingStatus,
    progress_token_struct: FaceIndexProgressState,
    cluster_epsilon: Option<f32>,
) -> Result<(), String> {
    let cancel_token = cancel_token_struct.0.clone();
    let status_token = status_token_struct.0.clone();
    let progress_token = progress_token_struct.0.clone();
    // Use provided epsilon or default to 0.42
    let epsilon = cluster_epsilon.unwrap_or(0.42);

    // Check if already running
    {
        let mut running = status_token.lock().unwrap();
        if *running {
            return Err("Face indexing is already running".to_string());
        }
        *running = true;
    }

    // Reset cancellation flag
    *cancel_token.lock().unwrap() = false;

    // Reset progress
    {
        let mut progress = progress_token.lock().unwrap();
        progress.current = 0;
        progress.total = 0;
        progress.faces_found = 0;
        progress.phase = "indexing".to_string();
    }

    tauri::async_runtime::spawn(async move {
        // 1. Initialization
        let reset_status = || {
            if let Ok(mut running) = status_token.lock() {
                *running = false;
            }
        };

        // Load models if not already loaded
        {
            let mut engine = face_state.0.lock().unwrap();
            if !engine.is_loaded() {
                if let Err(e) = engine.load_models(&app_handle) {
                    eprintln!("Failed to load face models: {}", e);
                    let _ = app_handle.emit(
                        "face_index_finished",
                        serde_json::json!({
                            "total_faces": 0,
                            "total_persons": 0,
                            "cancelled": false,
                            "error": e.to_string()
                        }),
                    );
                    reset_status();
                    return;
                }
            }
        }

        // 2. Preparation (Get files and stats)
        let (processed_count, existing_faces_count) = match t_sqlite::Face::get_stats() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to get stats: {}", e);
                (0, 0)
            }
        };

        let files = match t_sqlite::Face::get_unprocessed_image_files() {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Failed to get unprocessed files: {}", e);
                let _ = app_handle.emit(
                    "face_index_finished",
                    serde_json::json!({
                        "total_faces": 0,
                        "total_persons": 0,
                        "cancelled": false,
                        "error": e
                    }),
                );
                reset_status();
                return;
            }
        };

        let total_files = processed_count + files.len();
        let mut total_faces = existing_faces_count;
        let mut current = processed_count;

        // Init progress
        {
            let mut progress = progress_token.lock().unwrap();
            progress.total = total_files;
            progress.current = current;
            progress.faces_found = total_faces;
            progress.phase = "indexing".to_string();
        }

        let _ = app_handle.emit(
            "face_index_progress",
            serde_json::json!({
                "current": current,
                "total": total_files,
                "faces_found": total_faces,
                "phase": "indexing"
            }),
        );

        // 3. Image Processing Loop
        let mut cancelled = false;
        let db_conn = match t_sqlite::open_conn() {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("Failed to open DB connection for face indexing: {}", e);
                let _ = app_handle.emit(
                    "face_index_finished",
                    serde_json::json!({
                        "total_faces": 0,
                        "total_persons": 0,
                        "cancelled": false,
                        "error": e
                    }),
                );
                reset_status();
                return;
            }
        };

        for (file_id, file_path, width, height, orientation, file_type) in files {
            if *cancel_token.lock().unwrap() {
                cancelled = true;
                break;
            }

            current += 1;

            // Hydrated thumbnail (blob or cache-file bytes via AThumb::fetch);
            // only used as the LAST detection source — see the priority note
            // below.
            let thumb = t_sqlite::AThumb::fetch(file_id);
            // Reading the online-only original would download it; retry once it is local.
            if !matches!(&thumb, Ok(Some(t)) if t.thumb_data.is_some())
                && crate::t_utils::is_cloud_only_path(&file_path)
            {
                continue;
            }
            let mut engine = face_state.0.lock().unwrap();
            let load_original = |engine: &mut FaceEngine| {
                if !crate::t_utils::content_readable(&file_path) {
                    return Err("Skipped: the original is not local".to_string());
                }
                engine.process_image(&file_path)
            };
            // RAW fallback: `image::open` cannot decode CR2/NEF/ARW/..., so
            // without this every RAW failed in load_original and was marked
            // has_faces=2 forever. LibRaw's preview (embedded JPEG first,
            // display-oriented) is the detection source instead.
            let load_raw_preview = |engine: &mut FaceEngine| -> Result<
                (Vec<FaceData>, (u32, u32)),
                String,
            > {
                if !crate::t_utils::content_readable(&file_path) {
                    return Err("Skipped: the original is not local".to_string());
                }
                let preview = crate::t_libraw::get_raw_preview_image(
                    &file_path,
                    crate::t_raw_display::RawDisplayOptions::default(),
                )?
                .ok_or_else(|| "No RAW preview available".to_string())?;
                engine.process_image_from_bytes(&preview)
            };
            // Source priority: original → RAW preview → thumbnail. A thumbnail
            // is a 512px letterbox, so a 300px face on a 4272px original shrinks
            // to ~36px there and falls under the 90px source gate: indexing from
            // thumbnails (which became the de-facto behavior for every file once
            // AThumb::fetch started hydrating cache files into thumb_data)
            // silently dropped nearly the whole library. Detect at full
            // resolution first; the thumbnail stays as the last resort for rows
            // the decoders cannot read (e.g. HEIC) or cloud-only originals that
            // must not be downloaded.
            let mut process_result = load_original(&mut engine);
            if process_result.is_err() && file_type == 3 {
                process_result = load_raw_preview(&mut engine);
            }
            if process_result.is_err() {
                if let Ok(Some(thumb)) = thumb.as_ref() {
                    if let Some(thumb_bytes) = thumb.thumb_data.as_ref() {
                        process_result = engine.process_image_from_bytes(thumb_bytes);
                    }
                }
            }

            match process_result {
                Ok((mut faces, (proc_w, proc_h))) => {
                    // Both detection paths yield display-space pixels now.
                    // Scale bboxes (and landmarks) from the processed image
                    // into the file's DISPLAY dimensions. RAW/TIFF rows store
                    // sensor dims in afiles, so compute display dims explicitly.
                    let sensor_dims =
                        file_type == 3 || crate::t_libraw::is_tiff_path(&file_path);
                    let (disp_w, disp_h) =
                        t_common::display_dimensions(width, height, orientation, sensor_dims);
                    let scale_x = disp_w / proc_w as f32;
                    let scale_y = disp_h / proc_h as f32;

                    for face in &mut faces {
                        face.bbox.x *= scale_x;
                        face.bbox.y *= scale_y;
                        face.bbox.width *= scale_x;
                        face.bbox.height *= scale_y;
                        if let Some(kps) = face.bbox.landmarks.as_mut() {
                            for (kx, ky) in kps.iter_mut() {
                                *kx *= scale_x;
                                *ky *= scale_y;
                            }
                        }
                    }

                    let has_faces = !faces.is_empty();
                    let status = if has_faces { 1 } else { 2 };

                    if let Err(e) =
                        t_sqlite::Face::mark_scanned_with_conn(&db_conn, file_id, status)
                    {
                        eprintln!("Failed to mark file {} as scanned: {}", file_id, e);
                    }

                    if has_faces {
                        for face_data in &faces {
                            let bbox_json = serde_json::json!({
                                "x": face_data.bbox.x,
                                "y": face_data.bbox.y,
                                "width": face_data.bbox.width,
                                "height": face_data.bbox.height,
                                "confidence": face_data.bbox.confidence,
                            })
                            .to_string();

                            match t_sqlite::Face::add_with_conn(
                                &db_conn,
                                file_id,
                                &bbox_json,
                                &face_data.embedding,
                            ) {
                                Ok(_) => total_faces += 1,
                                Err(e) => eprintln!("Failed to store face: {}", e),
                            }
                        }
                    }
                }
                Err(e) => {
                    if !crate::t_utils::content_readable(&file_path) {
                        continue;
                    }
                    eprintln!("Failed to process image {}: {}", file_path, e);
                    // Do not retry files the decoder cannot read on every resume.
                    // Status 2 means the image was processed without a face result.
                    if let Err(mark_error) =
                        t_sqlite::Face::mark_scanned_with_conn(&db_conn, file_id, 2)
                    {
                        eprintln!(
                            "Failed to mark unreadable file {} as skipped: {}",
                            file_id, mark_error
                        );
                    }
                }
            }

            // Periodic progress update (every 10 files or at end)
            if current % 10 == 0 || current == total_files {
                {
                    let mut progress = progress_token.lock().unwrap();
                    progress.current = current;
                    progress.faces_found = total_faces;
                }

                let _ = app_handle.emit(
                    "face_index_progress",
                    serde_json::json!({
                        "current": current,
                        "total": total_files,
                        "faces_found": total_faces,
                        "phase": "indexing"
                    }),
                );
            }
        }

        if cancelled {
            let _ = app_handle.emit(
                "face_index_finished",
                serde_json::json!({
                    "total_faces": total_faces,
                    "total_persons": 0,
                    "cancelled": true
                }),
            );
            reset_status();
            return;
        }

        // 4. Clustering
        {
            let mut progress = progress_token.lock().unwrap();
            progress.phase = "clustering".to_string();
        }

        let _ = app_handle.emit(
            "face_index_progress",
            serde_json::json!({
                "current": total_files,
                "total": total_files,
                "faces_found": total_faces,
                "phase": "clustering"
            }),
        );

        let cancel_token_cluster = cancel_token.clone();
        let total_persons = match t_cluster::cluster_faces(
            epsilon,
            |progress| {
                let _ = app_handle.emit(
                    "cluster_progress",
                    serde_json::json!({
                        "phase": progress.phase,
                        "current": progress.current,
                        "total": progress.total,
                    }),
                );
            },
            || {
                // Check if user has cancelled
                *cancel_token_cluster.lock().unwrap()
            },
        ) {
            Ok(count) => count,
            Err(e) => {
                eprintln!("Clustering failed: {}", e);
                0
            }
        };
        let cancelled_during_cluster = *cancel_token.lock().unwrap();

        // 5. Finished
        let _ = app_handle.emit(
            "face_index_finished",
            serde_json::json!({
                "total_faces": total_faces,
                "total_persons": total_persons,
                "cancelled": cancelled_during_cluster
            }),
        );
        reset_status();
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_input_is_bgr_planar() {
        // 2x1 image: red pixel, then blue pixel. Channel 0 must be BLUE.
        let mut img = image::RgbImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        img.put_pixel(1, 0, image::Rgb([0, 0, 255]));
        let arr = embedding_input_bgr(&img);
        assert_eq!(arr.shape(), &[1, 3, 1, 2]);
        let expect = |v: u8| (v as f32 - 127.5) / 128.0;
        // red pixel: B channel reads 0, R channel reads 255
        assert!((arr[[0, 0, 0, 0]] - expect(0)).abs() < 1e-6);
        assert!((arr[[0, 2, 0, 0]] - expect(255)).abs() < 1e-6);
        // blue pixel: B channel reads 255, R channel reads 0
        assert!((arr[[0, 0, 0, 1]] - expect(255)).abs() < 1e-6);
        assert!((arr[[0, 2, 0, 1]] - expect(0)).abs() < 1e-6);
        assert!((arr[[0, 1, 0, 0]] - expect(0)).abs() < 1e-6);
    }

    #[test]
    fn similarity_transform_identity() {
        let pts = ALIGN_TEMPLATE_112;
        let (a, b, tx, ty) = estimate_similarity_transform(&pts, &pts).unwrap();
        assert!((a - 1.0).abs() < 1e-4 && b.abs() < 1e-4);
        assert!(tx.abs() < 1e-3 && ty.abs() < 1e-3);
    }

    #[test]
    fn similarity_transform_rotation_scale_translation() {
        // dst = rotate90ccw * 2 + (3, -4): X = -2y + 3, Y = 2x - 4
        let src = [
            (10.0f32, 20.0f32),
            (60.0, 15.0),
            (35.0, 80.0),
            (5.0, 55.0),
            (70.0, 60.0),
        ];
        let dst = src.map(|(x, y)| (-2.0 * y + 3.0, 2.0 * x - 4.0));
        let (a, b, tx, ty) = estimate_similarity_transform(&src, &dst).unwrap();
        assert!((a - 0.0).abs() < 1e-3, "a={a}");
        assert!((b - 2.0).abs() < 1e-3, "b={b}");
        assert!((tx - 3.0).abs() < 1e-3 && (ty + 4.0).abs() < 1e-3);
    }

    #[test]
    fn similarity_transform_degenerate_src_returns_none() {
        let src = [(5.0f32, 5.0f32); 5];
        let dst = ALIGN_TEMPLATE_112;
        assert!(estimate_similarity_transform(&src, &dst).is_none());
    }

    #[test]
    fn aligned_crop_with_identity_landmarks_returns_source() {
        // Landmarks already on the template -> identity transform -> the warp
        // must reproduce the source pixels exactly (integer sampling).
        let mut img = image::RgbImage::new(112, 112);
        for y in 0..112 {
            for x in 0..112 {
                img.put_pixel(x, y, image::Rgb([(x * 2 % 256) as u8, (y * 2 % 256) as u8, 77]));
            }
        }
        let kps = ALIGN_TEMPLATE_112;
        let out = aligned_face_crop(&DynamicImage::ImageRgb8(img.clone()), &kps).unwrap();
        assert_eq!(out.dimensions(), (112, 112));
        for y in 0..112 {
            for x in 0..112 {
                assert_eq!(out.get_pixel(x, y), img.get_pixel(x, y), "at {x},{y}");
            }
        }
    }

    #[test]
    fn display_dimensions_only_swaps_sensor_space_rows() {
        // JPG-style rows (display dims): never swapped again.
        assert_eq!(t_common::display_dimensions(4000, 3000, 6, false), (4000.0, 3000.0));
        // RAW/TIFF rows (sensor dims): swapped for orientation > 4.
        assert_eq!(t_common::display_dimensions(4000, 3000, 6, true), (3000.0, 4000.0));
        assert_eq!(t_common::display_dimensions(4000, 3000, 1, true), (4000.0, 3000.0));
    }

    /// End-to-end check on real photos with the real ONNX models. Skipped
    /// unless LAP_FACE_E2E_PAIRS is set to `pathA|pathB;pathC|pathD` where
    /// each pair shows the same person. Asserts every pair's best face-pair
    /// cosine reaches the cluster merge boundary (0.45); before the BGR +
    /// alignment fixes the same measurement averaged ~0.33.
    #[test]
    fn face_pipeline_same_person_similarity_local_e2e() {
        let Ok(spec) = std::env::var("LAP_FACE_E2E_PAIRS") else {
            eprintln!("LAP_FACE_E2E_PAIRS not set; skipping");
            return;
        };
        let models_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/models");
        let mut engine = FaceEngine::new();
        engine.load_models_from_dir(&models_dir).expect("models load");

        let mut worst = f32::INFINITY;
        // Same RAW fallback as the indexer, so CR2/NEF pairs can be tested.
        let process_any = |engine: &mut FaceEngine, path: &str| {
            engine
                .process_image(path)
                .or_else(|_| -> Result<(Vec<FaceData>, (u32, u32)), String> {
                    let preview = crate::t_libraw::get_raw_preview_image(
                        path,
                        crate::t_raw_display::RawDisplayOptions::default(),
                    )?
                    .ok_or_else(|| "No RAW preview available".to_string())?;
                    engine.process_image_from_bytes(&preview)
                })
        };
        for pair in spec.split(';').filter(|s| !s.trim().is_empty()) {
            let (pa, pb) = pair.split_once('|').expect("pair format: a|b");
            let fa = process_any(&mut engine, pa).unwrap_or_else(|e| panic!("{pa}: {e}")).0;
            let fb = process_any(&mut engine, pb).unwrap_or_else(|e| panic!("{pb}: {e}")).0;
            assert!(!fa.is_empty() && !fb.is_empty(), "no faces in {pa}/{pb}");
            let mut best = f32::NEG_INFINITY;
            for x in &fa {
                for y in &fb {
                    let dot: f32 =
                        x.embedding.iter().zip(&y.embedding).map(|(p, q)| p * q).sum();
                    best = best.max(dot);
                }
            }
            eprintln!("{pa} vs {pb}: faces=({},{}) best-pair cosine={best:.3}", fa.len(), fb.len());
            worst = worst.min(best);
        }
        assert!(
            worst >= 0.45,
            "same-person best-pair cosine {worst:.3} below the cluster merge boundary 0.45"
        );
    }

    /// Diagnostic: dump per-face pre-gate stats (confidence, source size, blur
    /// score) for gate calibration. Runs only with LAP_FACE_GATE_PROBE set to
    /// `path1;path2`; faces are NOT filtered, so the output shows exactly what
    /// each gate would have rejected.
    #[test]
    fn face_gate_probe_local() {
        let Ok(spec) = std::env::var("LAP_FACE_GATE_PROBE") else {
            eprintln!("LAP_FACE_GATE_PROBE not set; skipping");
            return;
        };
        let models_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/models");
        let mut engine = FaceEngine::new();
        engine.load_models_from_dir(&models_dir).expect("models load");

        for path in spec.split(';').filter(|s| !s.trim().is_empty()) {
            let img = match image::open(path) {
                Ok(i) => {
                    let orientation = crate::t_image::get_image_orientation(path);
                    crate::t_image::apply_orientation(i, orientation)
                }
                Err(open_err) => {
                    // Same RAW fallback the indexer uses, so CR2/NEF can be probed.
                    match crate::t_libraw::get_raw_preview_image(
                        path,
                        crate::t_raw_display::RawDisplayOptions::default(),
                    ) {
                        Ok(Some(bytes)) => match image::load_from_memory(&bytes) {
                            Ok(i) => i,
                            Err(e) => {
                                eprintln!("== {path}: RAW preview decode failed: {e}");
                                continue;
                            }
                        },
                        _ => {
                            eprintln!("== {path}: open failed: {open_err}; no RAW preview");
                            continue;
                        }
                    }
                }
            };
            let faces = match engine.detect_faces(&img) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("== {path}: detect failed: {e}");
                    continue;
                }
            };
            // Mirror the indexer's exact call so Err-vs-empty is visible.
            match engine.process_image(path) {
                Ok((kept, _)) => eprintln!("   process_image kept={}", kept.len()),
                Err(e) => eprintln!("   process_image ERR: {e}"),
            }
            eprintln!(
                "== {} ({}x{}) {} faces pre-gate",
                path,
                img.width(),
                img.height(),
                faces.len()
            );
            for f in &faces {
                let blur = engine.calculate_blur_score(&img, f);
                let pass = f.confidence >= t_common::MIN_CONFIDENCE
                    && f.width >= t_common::MIN_FACE_SIZE
                    && f.height >= t_common::MIN_FACE_SIZE
                    && blur >= t_common::MIN_BLUR_SCORE;
                eprintln!(
                    "   conf={:.3} box={:.0}x{:.0} blur={:.1} pass={}",
                    f.confidence, f.width, f.height, blur, pass
                );
            }
        }
    }
}
