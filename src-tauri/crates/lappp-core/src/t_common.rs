/**
 * Common constants and shared types
 */

// Image support
pub const NORMAL_IMGS: &[&str] = &[
    "jpg", "jpeg", "jfif", "png", "gif", "bmp", "tif", "tiff", "webp", "avif", "heic", "heif",
    "hif", "jxl",
];

pub const AFFINITY_IMGS: &[&str] = &["af", "afphoto", "afdesign", "afpub"];

// Image formats decoded through the bundled FFmpeg sidecar.
pub const FFMPEG_BACKED_IMGS: &[&str] = &[
    "psd", // Photoshop native format; preview only (merged composite), no edit support
    "exr", // HDR industry standard (VFX/rendering/Resolve output)
    "hdr", "rgbe", // Radiance RGBE format; HDR panoramas and tonemapped output
    "tga",  // Legacy format; some cameras/scanners/game asset pipelines
    "dds",  // DirectDraw Surface; game texture format; relevant only for game-dev users
    "qoi",  // Fast lossless format (2021); ecosystem still immature
    "jp2", "j2k", "j2c", "jpc", "jpf", "jpx", // JPEG 2000 family; medical/satellite use only
    "dpx", // Digital cinema intermediate format; niche film/grading pipeline only
];

// RAW support
pub const RAW_IMGS: &[&str] = &[
    "cr2", "cr3", "crw", // Canon
    "nef", "nrw", // Nikon
    "arw", "srf", "sr2", // Sony
    "raf", // Fujifilm
    "rw2", // Panasonic
    "orf", // Olympus / OM System
    "pef", // Pentax
    "dng", // Adobe / generic RAW
    "srw", // Samsung
    "rwl", // Leica
    "mrw", // Minolta / Konica Minolta
    "3fr", // Hasselblad
    "mos", "iiq", // Leaf / Phase One
    // "x3f", // Sigma / Foveon - temporarily disabled: current LibRaw path reports FileUnsupported for sampled X3F files, so indexing fails at RAW dimensions.
    "dcr", "kdc", // Kodak
    "erf", // Epson
    "mef", // Mamiya
    "raw", // Generic vendor RAW extension
    "mdc", // Legacy RAW variant in sample set
];

// Video support
pub const VIDEOS: &[&str] = &[
    "mpg", "mpeg", "vob", "mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "3gp", "m4v", "hevc", "asf",
    "mts", "m2ts", "mod", "tod", "ts",
];
// pub const AUDIOS: &[&str] = &[
//     "mp3", "wav", "flac", "aac", "m4a", "ogg", "wma", "mp2", "mp1", "ape", "alac", "wavpack",
// ];

/// Human-readable description of a file format extension.
pub(crate) fn extension_description(ext: &str) -> &'static str {
    match ext {
        "3fr" => "Hasselblad RAW",
        "3gp" => "3GPP Multimedia",
        "arw" => "Sony Alpha RAW",
        "asf" => "Advanced Systems Format",
        "avi" => "Audio Video Interleave",
        "avif" => "AV1 Image File Format",
        "bmp" => "Bitmap Image File",
        "cr2" => "Canon RAW 2",
        "cr3" => "Canon RAW 3",
        "crw" => "Canon RAW",
        "dcr" => "Kodak RAW",
        "dds" => "DirectDraw Surface",
        "dng" => "Digital Negative",
        "dpx" => "Digital Picture Exchange",
        "erf" => "Epson RAW",
        "exr" => "OpenEXR High Dynamic Range Image",
        "flv" => "Flash Video",
        "gif" => "Graphics Interchange Format",
        "hdr" => "Radiance High Dynamic Range Image",
        "heic" => "High Efficiency Image Container",
        "heif" => "High Efficiency Image File",
        "hevc" => "High Efficiency Video Coding",
        "hif" => "HEIF Image File",
        "j2c" => "JPEG 2000 Codestream",
        "j2k" => "JPEG 2000 Codestream",
        "jp2" => "JPEG 2000 Image",
        "jpc" => "JPEG 2000 Codestream",
        "jpeg" => "Joint Photographic Experts Group",
        "jfif" => "JPEG File Interchange Format",
        "jpf" => "JPEG 2000 Image",
        "jpg" => "Joint Photographic Experts Group",
        "jpx" => "JPEG 2000 Extended Image",
        "jxl" => "JPEG XL",
        "kdc" => "Kodak Digital Camera RAW",
        "m2ts" => "Blu-ray BDAV Video",
        "m4v" => "MPEG-4 Video",
        "mdc" => "Minolta RAW",
        "mef" => "Mamiya RAW",
        "mkv" => "Matroska Video",
        "mod" => "Camcorder Video",
        "mos" => "Leaf RAW",
        "mov" => "QuickTime Movie",
        "mp4" => "MPEG-4 Video",
        "mpeg" => "Moving Picture Experts Group Video",
        "mpg" => "MPEG Video",
        "mrw" => "Minolta RAW",
        "mts" => "AVCHD Video",
        "nef" => "Nikon Electronic Format",
        "nrw" => "Nikon RAW",
        "orf" => "Olympus RAW Format",
        "pef" => "Pentax Electronic File",
        "png" => "Portable Network Graphics",
        "af" | "afphoto" | "afdesign" | "afpub" => "Affinity Document",
        "psd" => "Photoshop Document",
        "qoi" => "Quite OK Image",
        "raf" => "Fujifilm RAW",
        "raw" => "Generic Camera RAW",
        "rgbe" => "Radiance RGBE Image",
        "rw2" => "Panasonic RAW 2",
        "rwl" => "Leica RAW",
        "sr2" => "Sony RAW 2",
        "srf" => "Sony RAW Format",
        "srw" => "Samsung RAW",
        "tga" => "Truevision Targa Image",
        "tif" => "Tagged Image File",
        "tiff" => "Tagged Image File Format",
        "tod" => "JVC HD Camcorder Video",
        "ts" => "MPEG Transport Stream",
        "vob" => "DVD Video Object",
        "webm" => "WebM Video",
        "webp" => "Web Picture Format",
        "wmv" => "Windows Media Video",
        _ => "File Format",
    }
}

// AI search
pub const AI_TEXT_MODEL: &str = "text_model.onnx";
pub const AI_VISION_MODEL: &str = "vision_model.onnx";
pub const AI_TOKENIZER: &str = "tokenizer.json";

// Face Recognition Constants

// models
pub const DETECTION_MODEL: &str = "det_500m.onnx"; // RetinaFace
pub const EMBEDDING_MODEL: &str = "w600k_mbf.onnx"; // MobileFaceNet

// Quality thresholds
/// Face confidence floor, store-side. Calibrated 2026-11-15 on this library:
/// real back-row faces in a scanned class photo measure 0.61-0.63 and distant
/// phone-photo faces 0.60-0.63, so the previous 0.65 rejected genuine faces.
/// 0.60 equals the detector's own NMS threshold, so quality control now rests
/// on the source-pixel size gate plus the aligned embedding, not on score.
pub const MIN_CONFIDENCE: f32 = 0.60;
// pub const MIN_FACE_RATIO: f32 = 0.0; // Disabled. Rely on absolute pixel size instead (better for high-res photos).
/// Minimum face size in SOURCE-image pixels (the image detection ran on —
/// thumbnail or original). The alignment warp samples that image, so embedding
/// quality is bound by the real face size. Faces below this embed noise and
/// used to become singleton garbage persons.
pub const MIN_FACE_SIZE: f32 = 90.0; // 80-112px is minimum for good recognition. 90.0 is a safe high-quality baseline.
/// Anti-mush floor for the Laplacian-variance blur score (crop measured at a
/// 256px long side, never upscaled — see `calculate_blur_score`). This is NOT
/// a quality filter anymore: on a scan-heavy library the clearest front-row
/// faces measure 45-76 while pre-fix garbage sat at 15-25, and the bands
/// overlap, so the score cannot separate real from garbage. The floor only
/// rejects literal motion mush; alignment-fixed embeddings keep the rest
/// honest. (The earlier 100 was calibrated on sharp digital close-ups and
/// rejected nearly every scanned-print face.)
pub const MIN_BLUR_SCORE: f32 = 20.0;

// Clustering Constants
pub const K_NEIGHBORS: usize = 80; // Prune edges to Top-K (K-NN)
pub const MIN_SAMPLES: usize = 1; // Minimum samples per cluster

/// Face bboxes are stored in DISPLAY (EXIF-oriented) pixel space. JPG-style
/// rows already store display dims in `afiles.width/height`, but RAW and TIFF
/// store decoder (sensor) dims there — for those the display dims are the
/// swap when orientation > 4.
pub fn display_dimensions(
    width: i64,
    height: i64,
    orientation: i32,
    sensor_dims: bool,
) -> (f32, f32) {
    if sensor_dims && orientation > 4 {
        (height as f32, width as f32)
    } else {
        (width as f32, height as f32)
    }
}

// Application identity.
//
// The core crate is shared by the desktop shell and the headless server, so
// `env!("CARGO_PKG_NAME")` here would resolve to "lappp-core" — an internal
// crate name, not the product. Anything user-visible (Welcome heading, About
// page, update dialogs) reads these constants instead.
pub const APP_NAME: &str = "lappp";
pub const APP_DESCRIPTION: &str = "An offline-first photo manager for large local libraries.";
pub const APP_REPOSITORY: &str = "https://github.com/viccom/lappp";
pub const APP_HOMEPAGE: &str = "https://github.com/viccom/lappp";
pub const APP_LICENSE: &str = "GPL-3.0-or-later";
/// Upstream authorship is kept for attribution; the fork adds no separate list.
pub const APP_AUTHORS: &[&str] = &["julyx10 <tiangle@gmail.com>"];

// Image understanding (vision-language) gateway
pub const VL_GATEWAY_URL_ENV: &str = "LAP_VL_GATEWAY_URL";
pub const VL_API_KEY_ENV: &str = "LAP_VL_API_KEY";
pub const VL_DEFAULT_MODEL: &str = "auto/deepseek-v4.1-flash";
/// The gateway is Anthropic Messages compatible.
pub const VL_ANTHROPIC_VERSION: &str = "2023-06-01";
/// Reasoning models spend part of this budget on thinking.
pub const VL_MAX_TOKENS: usize = 1500;
/// The gateway may take a long time before the first byte.
pub const VL_REQUEST_TIMEOUT_SECS: u64 = 180;

// ------------------------------------------------------------------
// Media type sniffing (moved from the desktop t_protocol in phase 3 so the
// core can classify bytes without the shell).

pub fn detect_image_mime(data: &[u8]) -> &'static str {
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if data.starts_with(&[0x47, 0x49, 0x46, 0x38]) {
        "image/gif"
    } else if data.starts_with(&[0x49, 0x49, 0x2A, 0x00])
        || data.starts_with(&[0x4D, 0x4D, 0x00, 0x2A])
        || data.starts_with(&[0x4D, 0x4D, 0x00, 0x2B])
        || data.starts_with(&[0x49, 0x49, 0x2B, 0x00])
    {
        "image/tiff"
    } else if data.starts_with(b"BM") {
        "image/bmp"
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        detect_isobmff_mime(data)
    }
}

fn detect_isobmff_mime(data: &[u8]) -> &'static str {
    if data.len() < 16 || &data[4..8] != b"ftyp" {
        return "application/octet-stream";
    }

    let box_size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if box_size < 16 || box_size > data.len() {
        return "application/octet-stream";
    }

    match &data[8..12] {
        b"avif" | b"avis" => "image/avif",
        b"heic" | b"heix" | b"hevc" | b"hevx" => "image/heic",
        b"mif1" | b"heif" => "image/heif",
        _ => {
            for brand in data[16..box_size].chunks(4) {
                if brand.len() < 4 {
                    break;
                }
                match brand {
                    b"avif" | b"avis" => return "image/avif",
                    b"heic" | b"heix" | b"hevc" | b"hevx" => {
                        return "image/heic";
                    }
                    b"mif1" | b"heif" => return "image/heif",
                    _ => {}
                }
            }
            "application/octet-stream"
        }
    }
}
