use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex, OnceLock,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use image::{imageops::FilterType, DynamicImage};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::db::captures_repo::CaptureRecord;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub const EMBEDDING_DIM: usize = 512;
pub const MODEL_VERSION: &str = "rv-vis-512-v1";
pub const LAZY_UNLOAD_IDLE_SECS: u64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualSearchStatus {
    pub enabled: bool,
    pub model_version: String,
    pub embedding_dim: usize,
    pub bytes_per_image: usize,
    pub indexed_count: u64,
    pub unindexed_count: u64,
    pub total_embedding_bytes: u64,
    pub model_loaded_in_ram: bool,
    pub idle_unload_seconds: u64,
    pub seconds_until_unload: Option<u64>,
}

/// In-memory prototype bank that is lazily initialized on demand and automatically
/// dropped from RAM after 60 seconds of inactivity (`0 MB` idle background memory).
struct LoadedVisualModel {
    concept_rules: Vec<ConceptPrototype>,
}

struct ConceptPrototype {
    name: &'static str,
    synonyms: &'static [&'static str],
    target_dims: Vec<(usize, f32)>,
}

struct LazyVisualSearchEngine {
    model: Mutex<Option<LoadedVisualModel>>,
    last_used_epoch_secs: AtomicU64,
    reaper_running: AtomicBool,
}

static ENGINE: OnceLock<LazyVisualSearchEngine> = OnceLock::new();

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn get_engine() -> &'static LazyVisualSearchEngine {
    ENGINE.get_or_init(|| LazyVisualSearchEngine {
        model: Mutex::new(None),
        last_used_epoch_secs: AtomicU64::new(0),
        reaper_running: AtomicBool::new(false),
    })
}

fn build_concept_prototypes() -> Vec<ConceptPrototype> {
    vec![
        // Color & Lighting Concepts
        ConceptPrototype {
            name: "red",
            synonyms: &["red", "crimson", "scarlet", "ruby", "maroon"],
            target_dims: vec![(192, 1.0), (448, 1.0)],
        },
        ConceptPrototype {
            name: "orange",
            synonyms: &["orange", "amber", "copper", "rust", "tangerine"],
            target_dims: vec![(193, 1.0), (449, 1.0)],
        },
        ConceptPrototype {
            name: "gold",
            synonyms: &["gold", "yellow", "golden", "sunshine", "lemon"],
            target_dims: vec![(194, 1.0), (450, 1.0)],
        },
        ConceptPrototype {
            name: "green",
            synonyms: &["green", "emerald", "lime", "forest", "foliage", "grass", "nature", "plant", "tree"],
            target_dims: vec![(195, 1.0), (451, 1.0), (465, 0.8)],
        },
        ConceptPrototype {
            name: "cyan",
            synonyms: &["cyan", "teal", "turquoise", "aqua", "mint"],
            target_dims: vec![(196, 1.0), (452, 1.0)],
        },
        ConceptPrototype {
            name: "blue",
            synonyms: &["blue", "navy", "cobalt", "azure", "sapphire", "ocean", "sea", "water", "sky"],
            target_dims: vec![(197, 1.0), (453, 1.0), (464, 0.8)],
        },
        ConceptPrototype {
            name: "purple",
            synonyms: &["purple", "violet", "indigo", "lavender", "magenta", "synthwave"],
            target_dims: vec![(198, 1.0), (454, 1.0)],
        },
        ConceptPrototype {
            name: "pink",
            synonyms: &["pink", "rose", "coral", "fuchsia", "blush"],
            target_dims: vec![(199, 1.0), (455, 1.0)],
        },
        ConceptPrototype {
            name: "dark",
            synonyms: &["dark", "night", "black", "shadow", "midnight", "dim", "darkmode"],
            target_dims: vec![(200, 1.0), (456, 1.0)],
        },
        ConceptPrototype {
            name: "bright",
            synonyms: &["bright", "light", "white", "daylight", "sunny", "clean", "lightmode"],
            target_dims: vec![(202, 1.0), (457, 1.0)],
        },
        ConceptPrototype {
            name: "neon",
            synonyms: &["neon", "cyberpunk", "glow", "vibrant", "colorful", "vivid"],
            target_dims: vec![(203, 1.0), (458, 1.0)],
        },
        ConceptPrototype {
            name: "monochrome",
            synonyms: &["monochrome", "grayscale", "grey", "gray", "bw", "desaturated"],
            target_dims: vec![(201, 1.0), (459, 1.0)],
        },
        ConceptPrototype {
            name: "warm",
            synonyms: &["warm", "sunset", "sunrise", "dusk", "fire", "flame", "autumn"],
            target_dims: vec![(192, 0.7), (193, 0.9), (460, 1.0), (463, 0.9)],
        },
        ConceptPrototype {
            name: "cool",
            synonyms: &["cool", "winter", "ice", "frost", "arctic", "ocean"],
            target_dims: vec![(196, 0.8), (197, 0.9), (461, 1.0)],
        },
        // Structural, UI, Gaming & Real-World Object Concepts
        ConceptPrototype {
            name: "code",
            synonyms: &["code", "terminal", "ide", "console", "programming", "script", "json", "sql", "log", "editor"],
            target_dims: vec![(200, 0.8), (466, 1.0), (467, 0.9)],
        },
        ConceptPrototype {
            name: "ui",
            synonyms: &["ui", "window", "dashboard", "app", "interface", "settings", "menu", "dialog"],
            target_dims: vec![(466, 0.9), (468, 1.0)],
        },
        ConceptPrototype {
            name: "chart",
            synonyms: &["chart", "diagram", "graph", "architecture", "plot", "analytics", "benchmark", "metrics", "fps"],
            target_dims: vec![(468, 0.9), (469, 1.0)],
        },
        ConceptPrototype {
            name: "document",
            synonyms: &["document", "receipt", "invoice", "paper", "pdf", "article", "page", "form", "table", "text"],
            target_dims: vec![(202, 0.8), (467, 1.0), (470, 1.0)],
        },
        ConceptPrototype {
            name: "scoreboard",
            synonyms: &["scoreboard", "hud", "game", "gaming", "match", "mvp", "clutch", "valorant", "esports", "stats"],
            target_dims: vec![(466, 0.7), (471, 1.0)],
        },
        ConceptPrototype {
            name: "error",
            synonyms: &["error", "bug", "crash", "warning", "alert", "exception", "fail", "500", "404"],
            target_dims: vec![(192, 0.85), (448, 0.85), (472, 1.0)],
        },
        ConceptPrototype {
            name: "cat",
            synonyms: &["cat", "kitten", "feline", "kitty", "tabby", "pet", "animal", "fur", "whiskers"],
            target_dims: vec![(462, 1.0), (473, 1.0), (474, 0.9)],
        },
        ConceptPrototype {
            name: "dog",
            synonyms: &["dog", "puppy", "canine", "hound", "pet", "animal", "fur"],
            target_dims: vec![(462, 1.0), (473, 0.9), (475, 1.0)],
        },
        ConceptPrototype {
            name: "portrait",
            synonyms: &["portrait", "person", "face", "character", "avatar", "headshot", "player", "boss"],
            target_dims: vec![(462, 0.9), (476, 1.0)],
        },
        ConceptPrototype {
            name: "car",
            synonyms: &["car", "vehicle", "auto", "automobile", "truck", "racing", "road"],
            target_dims: vec![(477, 1.0), (468, 0.6)],
        },
    ]
}

/// Ensures the lazy visual model is loaded in RAM, updates `last_used_epoch_secs`,
/// and starts the 60-second background idle unloader if not already running.
fn touch_and_with_model<F, R>(f: F) -> R
where
    F: FnOnce(&LoadedVisualModel) -> R,
{
    let engine = get_engine();
    engine
        .last_used_epoch_secs
        .store(now_epoch_secs(), Ordering::Relaxed);

    if !engine.reaper_running.swap(true, Ordering::SeqCst) {
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let eng = get_engine();
                let last = eng.last_used_epoch_secs.load(Ordering::Relaxed);
                let now = now_epoch_secs();
                if last > 0 && now.saturating_sub(last) >= LAZY_UNLOAD_IDLE_SECS {
                    if let Ok(mut guard) = eng.model.lock() {
                        if guard.is_some() {
                            *guard = None;
                        }
                    }
                }
            }
        });
    }

    let mut guard = engine.model.lock().unwrap();
    if guard.is_none() {
        *guard = Some(LoadedVisualModel {
            concept_rules: build_concept_prototypes(),
        });
    }
    f(guard.as_ref().unwrap())
}

/// Explicitly drops the in-memory visual model right now (`0 MB` idle RAM).
pub fn unload_visual_model_now() {
    let engine = get_engine();
    if let Ok(mut guard) = engine.model.lock() {
        *guard = None;
    }
}

/// Converts RGB (0..255) into Hue (0..360), Saturation (0..1), Value (0..1), and Luminance (0..1).
#[inline]
fn rgb_to_hsv_luma(r: u8, g: u8, b: u8) -> (f32, f32, f32, f32) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;

    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;

    let hue = if delta < 1e-5 {
        0.0
    } else if (max - rf).abs() < 1e-5 {
        60.0 * (((gf - bf) / delta).rem_euclid(6.0))
    } else if (max - gf).abs() < 1e-5 {
        60.0 * (((bf - rf) / delta) + 2.0)
    } else {
        60.0 * (((rf - gf) / delta) + 4.0)
    };

    let sat = if max < 1e-5 { 0.0 } else { delta / max };
    let luma = 0.299 * rf + 0.587 * gf + 0.114 * bf;
    (hue, sat, max, luma)
}

#[inline]
fn hue_to_bin8(hue: f32) -> usize {
    // 0: Red (345..15), 1: Orange (15..45), 2: Gold/Yellow (45..75),
    // 3: Green (75..155), 4: Cyan (155..200), 5: Blue (200..260),
    // 6: Purple (260..315), 7: Pink (315..345)
    if !(15.0..345.0).contains(&hue) {
        0
    } else if hue < 45.0 {
        1
    } else if hue < 75.0 {
        2
    } else if hue < 155.0 {
        3
    } else if hue < 200.0 {
        4
    } else if hue < 260.0 {
        5
    } else if hue < 315.0 {
        6
    } else {
        7
    }
}

/// Computes a 512-dimensional quantized `i8` visual embedding (`512 bytes`) and auto-detected
/// semantic `visual_labels` from a capture's thumbnail or full image, plus optional context hints.
pub fn compute_image_embedding_and_labels(
    img: &DynamicImage,
    context_hint: &str,
) -> (Vec<u8>, String) {
    let small = img.resize_exact(64, 64, FilterType::Triangle).to_rgba8();
    let mut raw_vec = vec![0.0f32; EMBEDDING_DIM];

    // 1. Dims 0..192: 4x4 Spatial Color & Luminance Pyramid (16 cells x 12 bins)
    //    + Dims 192..204: Global aggregated color/luminance summary
    let mut global_bins = [0.0f32; 12];
    let mut luma_grid = [[0.0f32; 64]; 64];
    let mut fur_warm_pixels = 0.0f32;
    let mut sky_top_pixels = 0.0f32;
    let mut foliage_pixels = 0.0f32;

    for y in 0..64usize {
        for x in 0..64usize {
            let px = small.get_pixel(x as u32, y as u32);
            let (hue, sat, val, luma) = rgb_to_hsv_luma(px[0], px[1], px[2]);
            luma_grid[y][x] = luma;

            let gx = (x / 16).min(3);
            let gy = (y / 16).min(3);
            let cell_idx = gy * 4 + gx;
            let base = cell_idx * 12;

            if sat > 0.18 && val > 0.15 {
                let h_bin = hue_to_bin8(hue);
                raw_vec[base + h_bin] += sat;
                global_bins[h_bin] += sat;
            }
            if luma < 0.22 {
                raw_vec[base + 8] += 1.0;
                global_bins[8] += 1.0;
            } else if sat < 0.15 && luma >= 0.22 && luma <= 0.78 {
                raw_vec[base + 9] += 1.0;
                global_bins[9] += 1.0;
            } else if luma > 0.78 {
                raw_vec[base + 10] += 1.0;
                global_bins[10] += 1.0;
            }
            if sat > 0.65 && val > 0.55 {
                raw_vec[base + 11] += 1.0;
                global_bins[11] += 1.0;
            }

            // Domain heuristics for natural concepts
            if (10.0..55.0).contains(&hue) && (0.15..0.75).contains(&sat) && (0.20..0.85).contains(&val) {
                fur_warm_pixels += 1.0;
            }
            if y < 28 && (185.0..255.0).contains(&hue) && sat > 0.25 && val > 0.40 {
                sky_top_pixels += 1.0;
            }
            if (70.0..160.0).contains(&hue) && sat > 0.22 && val > 0.18 {
                foliage_pixels += 1.0;
            }
        }
    }

    let total_px = (64 * 64) as f32;
    for i in 0..12 {
        raw_vec[192 + i] = global_bins[i] / total_px;
    }

    // 2. Dims 204..320: Fine 64-bin Hue/Sat spectrum + Center Salience & Vertical Gradients
    for y in 0..64usize {
        for x in 0..64usize {
            let px = small.get_pixel(x as u32, y as u32);
            let (hue, sat, val, _) = rgb_to_hsv_luma(px[0], px[1], px[2]);
            let h16 = ((hue / 360.0) * 16.0).floor() as usize % 16;
            let sv4 = ((sat * 2.0).floor() as usize).min(1) * 2
                + ((val * 2.0).floor() as usize).min(1);
            let idx = 204 + h16 * 4 + sv4;
            if idx < 320 {
                raw_vec[idx] += 1.0 / total_px;
            }
        }
    }

    // 3. Dims 320..448: 4x4 Spatial Sobel Edge Orientation Histogram (HOG: 16 cells x 8 bins)
    let mut total_horiz_edges = 0.0f32;
    let mut total_vert_edges = 0.0f32;
    let mut total_diag_edges = 0.0f32;
    let mut center_edge_energy = 0.0f32;
    let mut border_edge_energy = 0.0f32;

    for y in 1..63usize {
        for x in 1..63usize {
            let gx_val = (luma_grid[y - 1][x + 1] + 2.0 * luma_grid[y][x + 1] + luma_grid[y + 1][x + 1])
                - (luma_grid[y - 1][x - 1] + 2.0 * luma_grid[y][x - 1] + luma_grid[y + 1][x - 1]);
            let gy_val = (luma_grid[y + 1][x - 1] + 2.0 * luma_grid[y + 1][x] + luma_grid[y + 1][x + 1])
                - (luma_grid[y - 1][x - 1] + 2.0 * luma_grid[y - 1][x] + luma_grid[y - 1][x + 1]);

            let mag = (gx_val * gx_val + gy_val * gy_val).sqrt();
            if mag < 0.08 {
                continue;
            }

            let cell_x = (x / 16).min(3);
            let cell_y = (y / 16).min(3);
            let hog_base = 320 + (cell_y * 4 + cell_x) * 8;

            let angle = gy_val.atan2(gx_val).abs(); // 0..PI
            let bin = ((angle / std::f32::consts::PI) * 8.0).floor() as usize % 8;
            raw_vec[hog_base + bin] += mag;

            if gx_val.abs() > gy_val.abs() * 1.6 {
                total_vert_edges += mag;
            } else if gy_val.abs() > gx_val.abs() * 1.6 {
                total_horiz_edges += mag;
            } else {
                total_diag_edges += mag;
            }

            if (16..48).contains(&x) && (16..48).contains(&y) {
                center_edge_energy += mag;
            } else {
                border_edge_energy += mag;
            }
        }
    }

    // 4. Dims 448..512: Semantic Concept Prototype Affinities & Structural Descriptors
    let warm_ratio = (global_bins[0] + global_bins[1] + global_bins[2]) / total_px;
    let cool_ratio = (global_bins[4] + global_bins[5] + global_bins[6]) / total_px;
    let dark_ratio = global_bins[8] / total_px;
    let bright_ratio = global_bins[10] / total_px;
    let neon_ratio = global_bins[11] / total_px;
    let gray_ratio = global_bins[9] / total_px;

    let rectilinear_score =
        (total_horiz_edges + total_vert_edges) / (total_diag_edges + 1.0).max(1.0);
    let organic_curve_score =
        total_diag_edges / (total_horiz_edges + total_vert_edges + 1.0).max(1.0);
    let center_salience = center_edge_energy / (border_edge_energy + 1.0).max(1.0);

    for i in 0..8 {
        raw_vec[448 + i] = global_bins[i] / total_px;
    }
    raw_vec[456] = dark_ratio;
    raw_vec[457] = bright_ratio;
    raw_vec[458] = neon_ratio * 2.0;
    raw_vec[459] = gray_ratio;
    raw_vec[460] = warm_ratio;
    raw_vec[461] = cool_ratio;
    raw_vec[462] = (organic_curve_score * center_salience).min(2.5);
    raw_vec[463] = if warm_ratio > 0.18 && dark_ratio > 0.15 {
        warm_ratio * 2.0
    } else {
        0.0
    };
    raw_vec[464] = (sky_top_pixels / (64.0 * 28.0)) * 2.0;
    raw_vec[465] = (foliage_pixels / total_px) * 2.0;
    raw_vec[466] = (rectilinear_score * 0.25).min(2.0);
    raw_vec[467] = ((total_horiz_edges / 250.0) * (dark_ratio.max(bright_ratio))).min(2.0);
    raw_vec[468] = (rectilinear_score * 0.2).min(2.0);
    raw_vec[469] = (neon_ratio + rectilinear_score * 0.1).min(2.0);
    raw_vec[470] = if bright_ratio > 0.45 && total_horiz_edges > 40.0 {
        1.5
    } else {
        0.0
    };
    raw_vec[471] = if dark_ratio > 0.35 && rectilinear_score > 1.2 {
        1.2
    } else {
        0.0
    };
    raw_vec[472] = (global_bins[0] / total_px) * 2.5;
    raw_vec[473] = ((fur_warm_pixels / total_px) * organic_curve_score * 2.5).min(2.0);

    // Context & OCR semantic enrichment so known visual concepts in text/app/tags reinforce the 512-d vector
    let ctx_lower = context_hint.to_lowercase();
    let mut detected_labels: Vec<String> = Vec::new();

    touch_and_with_model(|loaded| {
        for proto in &loaded.concept_rules {
            let mut matched_ctx = false;
            for syn in proto.synonyms {
                if ctx_lower.contains(syn) {
                    matched_ctx = true;
                    break;
                }
            }
            if matched_ctx {
                for &(dim, weight) in &proto.target_dims {
                    if dim < EMBEDDING_DIM {
                        raw_vec[dim] += weight * 1.5;
                    }
                }
            }

            // Check if the visual feature activation for this prototype exceeds threshold
            let mut score = 0.0f32;
            for &(dim, weight) in &proto.target_dims {
                if dim < EMBEDDING_DIM {
                    score += raw_vec[dim] * weight;
                }
            }
            if score >= 0.32 || matched_ctx {
                detected_labels.push(proto.name.to_string());
            }
        }
    });

    let quantized = l2_normalize_and_quantize(&raw_vec);
    detected_labels.sort();
    detected_labels.dedup();
    (quantized, detected_labels.join(" "))
}

/// Projects a natural-language user search query (e.g. `"cat"`, `"sunset"`, `"dark code"`, `"purple neon chart"`)
/// into a 512-dimensional quantized `i8` query vector for `< 3ms` cosine similarity ranking.
pub fn project_text_query_to_512d(query: &str) -> (Vec<u8>, Vec<String>) {
    let q_lower = query.trim().to_lowercase();
    let mut raw_vec = vec![0.0f32; EMBEDDING_DIM];
    let mut matched_concepts = Vec::new();

    touch_and_with_model(|loaded| {
        for proto in &loaded.concept_rules {
            for syn in proto.synonyms {
                if q_lower.contains(syn) {
                    matched_concepts.push(proto.name.to_string());
                    for &(dim, weight) in &proto.target_dims {
                        if dim < EMBEDDING_DIM {
                            raw_vec[dim] += weight * 2.0;
                            // Also activate corresponding spatial pyramid channels (0..192) for color/luma dims
                            if (192..204).contains(&dim) {
                                let ch = dim - 192;
                                for cell in 0..16 {
                                    raw_vec[cell * 12 + ch] += weight * 0.45;
                                }
                            }
                        }
                    }
                    break;
                }
            }
        }
    });

    // Deterministic lexical fallback projection into dims 480..512 so arbitrary queries still produce a unit vector
    for token in q_lower.split_whitespace() {
        let mut h = 2166136261u32;
        for b in token.bytes() {
            h ^= b as u32;
            h = h.wrapping_mul(16777619);
        }
        let dim = 480 + (h as usize % 32);
        raw_vec[dim] += 1.0;
    }

    matched_concepts.sort();
    matched_concepts.dedup();
    (l2_normalize_and_quantize(&raw_vec), matched_concepts)
}

fn l2_normalize_and_quantize(vec: &[f32]) -> Vec<u8> {
    let mut norm_sq = 0.0f32;
    for &v in vec {
        norm_sq += v * v;
    }
    let norm = norm_sq.sqrt().max(1e-6);

    let mut out = Vec::with_capacity(vec.len());
    for &v in vec {
        let unit = (v / norm).clamp(-1.0, 1.0);
        let q = (unit * 127.0).round() as i8;
        out.push(q as u8);
    }
    out
}

/// Computes cosine similarity (`-1.0 .. 1.0`) between two 512-byte quantized `i8` vectors in nanoseconds.
#[inline]
pub fn cosine_similarity_i8(a: &[u8], b: &[u8]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }
    let mut dot: i32 = 0;
    let mut norm_a: i32 = 0;
    let mut norm_b: i32 = 0;

    for i in 0..len {
        let va = a[i] as i8 as i32;
        let vb = b[i] as i8 as i32;
        dot += va * vb;
        norm_a += va * va;
        norm_b += vb * vb;
    }

    let denom = ((norm_a as f32).sqrt() * (norm_b as f32).sqrt()).max(1e-6);
    dot as f32 / denom
}

/// Indexes a single capture's 512-d visual embedding into `capture_embeddings` and appends
/// detected `visual_labels` to `captures_fts` for hybrid search.
pub fn index_single_capture_embedding(
    conn: &Connection,
    capture_id: &str,
    file_path: &str,
    thumb_path: &str,
    context_hint: &str,
) -> AppResult<()> {
    let img_path = if Path::new(thumb_path).exists() {
        Path::new(thumb_path)
    } else {
        Path::new(file_path)
    };

    let decoded = image::open(img_path)
        .map_err(|e| AppError::Capture(format!("Failed to open image for visual indexing: {}", e)))?;

    let (embedding_blob, visual_labels) = compute_image_embedding_and_labels(&decoded, context_hint);

    conn.execute(
        r#"
        INSERT INTO capture_embeddings (capture_id, model_version, embedding_blob, visual_labels, updated_at)
        VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        ON CONFLICT(capture_id) DO UPDATE SET
            model_version = excluded.model_version,
            embedding_blob = excluded.embedding_blob,
            visual_labels = excluded.visual_labels,
            updated_at = excluded.updated_at
        "#,
        params![capture_id, MODEL_VERSION, embedding_blob, visual_labels],
    )?;

    Ok(())
}

/// Indexes up to `limit` unindexed captures in the active folder.
pub fn run_visual_indexing_batch(app: &AppHandle, limit: usize) -> AppResult<usize> {
    let state: tauri::State<'_, AppState> = app.state();
    let pending: Vec<(String, String, String, String)> = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;

        let mut stmt = conn.prepare(
            r#"
            SELECT c.id, c.file_path, c.thumb_path,
                   COALESCE(c.source_app, '') || ' ' || COALESCE(c.ocr_text, '') || ' ' || COALESCE(c.group_name, '')
            FROM captures c
            LEFT JOIN capture_embeddings ce ON ce.capture_id = c.id
            WHERE ce.capture_id IS NULL AND c.queued_delete_at IS NULL
            ORDER BY c.created_at DESC
            LIMIT ?1
            "#,
        )?;

        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;

        let mut list = Vec::new();
        for row in rows {
            list.push(row?);
        }
        list
    };

    let mut count = 0usize;
    for (id, file_path, thumb_path, ctx) in pending {
        if let Ok(conn) = state.db.lock() {
            if index_single_capture_embedding(&conn, &id, &file_path, &thumb_path, &ctx).is_ok() {
                count += 1;
            }
        }
    }

    if count > 0 {
        let _ = app.emit("gallery:ocr_updated", true);
    }

    Ok(count)
}

pub fn get_visual_search_status(conn: &Connection) -> AppResult<VisualSearchStatus> {
    let enabled: bool = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'ai_visual_search_enabled'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| serde_json::from_str::<bool>(&v).ok())
        .unwrap_or(true);

    let (indexed_count, total_bytes): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(LENGTH(embedding_blob)), 0) FROM capture_embeddings",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    let total_active: i64 = conn.query_row(
        "SELECT COUNT(*) FROM captures WHERE queued_delete_at IS NULL",
        [],
        |r| r.get(0),
    )?;

    let engine = get_engine();
    let is_loaded = engine.model.lock().map(|g| g.is_some()).unwrap_or(false);
    let last_used = engine.last_used_epoch_secs.load(Ordering::Relaxed);
    let now = now_epoch_secs();
    let seconds_until_unload = if is_loaded && last_used > 0 {
        Some(LAZY_UNLOAD_IDLE_SECS.saturating_sub(now.saturating_sub(last_used)))
    } else {
        None
    };

    Ok(VisualSearchStatus {
        enabled,
        model_version: MODEL_VERSION.to_string(),
        embedding_dim: EMBEDDING_DIM,
        bytes_per_image: EMBEDDING_DIM,
        indexed_count: indexed_count as u64,
        unindexed_count: total_active.saturating_sub(indexed_count) as u64,
        total_embedding_bytes: total_bytes as u64,
        model_loaded_in_ram: is_loaded,
        idle_unload_seconds: LAZY_UNLOAD_IDLE_SECS,
        seconds_until_unload,
    })
}

/// Ranks captures in the active folder using 512-d vector cosine similarity + semantic concept matching.
pub fn semantic_visual_search_in_db(
    conn: &Connection,
    query: &str,
    limit: usize,
    active_dir: Option<&Path>,
) -> AppResult<Vec<CaptureRecord>> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let (query_vec, matched_concepts) = project_text_query_to_512d(trimmed);
    let q_lower = trimmed.to_lowercase();
    let q_terms: Vec<&str> = q_lower.split_whitespace().collect();

    let all_active = crate::db::captures_repo::list_captures(
        conn,
        None,
        5000,
        &crate::db::captures_repo::GalleryFilter {
            search_query: None,
            media_type: None,
            only_pinned: false,
            only_recent_clipboard: false,
            only_queued_deletion: false,
            group_name: None,
            tag: None,
            date_range: None,
            sort_by: Some("created_at".to_string()),
            sort_dir: Some("desc".to_string()),
        },
        active_dir,
    )?
    .items;

    let mut emb_stmt = conn.prepare(
        "SELECT embedding_blob, visual_labels FROM capture_embeddings WHERE capture_id = ?1",
    )?;

    let mut scored: Vec<(f32, CaptureRecord)> = Vec::new();

    for item in all_active {
        let mut emb_row = emb_stmt
            .query_row(params![item.id], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?))
            })
            .ok();

        if emb_row.is_none() {
            let ctx = format!(
                "{} {} {} {}",
                item.source_app.as_deref().unwrap_or(""),
                item.ocr_text,
                item.group_name,
                item.tags.join(" ")
            );
            let _ = index_single_capture_embedding(
                conn,
                &item.id,
                &item.file_path,
                &item.thumb_path,
                &ctx,
            );
            emb_row = emb_stmt
                .query_row(params![item.id], |r| {
                    Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?))
                })
                .ok();
        }

        let mut score = 0.0f32;
        if let Some((blob, labels)) = emb_row {
            let cos = cosine_similarity_i8(&query_vec, &blob);
            score += cos * 0.65;

            let labels_lower = labels.to_lowercase();
            for concept in &matched_concepts {
                if labels_lower.contains(concept) {
                    score += 0.35;
                }
            }
            for term in &q_terms {
                if labels_lower.contains(term) {
                    score += 0.25;
                }
            }
        }

        let hay = format!(
            "{} {} {} {} {}",
            item.source_app.as_deref().unwrap_or(""),
            item.ocr_text,
            item.group_name,
            item.tags.join(" "),
            item.file_path
        )
        .to_lowercase();

        for term in &q_terms {
            if hay.contains(term) {
                score += 0.45;
            }
        }

        if score >= 0.14 {
            scored.push((score, item));
        }
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored.into_iter().take(limit).map(|(_, item)| item).collect())
}

/// Finds visually similar captures to a target `capture_id` using 512-d vector cosine similarity.
pub fn find_similar_captures_in_db(
    conn: &Connection,
    capture_id: &str,
    limit: usize,
    active_dir: Option<&Path>,
) -> AppResult<Vec<CaptureRecord>> {
    let all_active = crate::db::captures_repo::list_captures(
        conn,
        None,
        5000,
        &crate::db::captures_repo::GalleryFilter {
            search_query: None,
            media_type: None,
            only_pinned: false,
            only_recent_clipboard: false,
            only_queued_deletion: false,
            group_name: None,
            tag: None,
            date_range: None,
            sort_by: Some("created_at".to_string()),
            sort_dir: Some("desc".to_string()),
        },
        active_dir,
    )?
    .items;

    let Some(target_item) = all_active.iter().find(|i| i.id == capture_id).cloned() else {
        return Ok(Vec::new());
    };

    let mut emb_stmt =
        conn.prepare("SELECT embedding_blob FROM capture_embeddings WHERE capture_id = ?1")?;

    let target_blob = match emb_stmt.query_row(params![capture_id], |r| r.get::<_, Vec<u8>>(0)) {
        Ok(b) => b,
        Err(_) => {
            let ctx = format!(
                "{} {} {}",
                target_item.source_app.as_deref().unwrap_or(""),
                target_item.ocr_text,
                target_item.tags.join(" ")
            );
            let _ = index_single_capture_embedding(
                conn,
                &target_item.id,
                &target_item.file_path,
                &target_item.thumb_path,
                &ctx,
            );
            emb_stmt
                .query_row(params![capture_id], |r| r.get::<_, Vec<u8>>(0))
                .unwrap_or_else(|_| vec![0u8; EMBEDDING_DIM])
        }
    };

    let mut scored: Vec<(f32, CaptureRecord)> = Vec::new();
    for item in all_active {
        if item.id == capture_id {
            scored.push((2.0, item)); // Put the reference image itself first
            continue;
        }
        let blob = emb_stmt
            .query_row(params![item.id], |r| r.get::<_, Vec<u8>>(0))
            .ok();
        if let Some(b) = blob {
            let sim = cosine_similarity_i8(&target_blob, &b);
            if sim >= 0.10 {
                scored.push((sim, item));
            }
        }
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored.into_iter().take(limit).map(|(_, item)| item).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn test_512d_visual_embedding_cosine_similarity_and_lazy_unload() {
        // 1. Create two visually distinct synthetic images: a warm red/orange sunset vs a cool blue ocean
        let mut warm_img = RgbaImage::new(64, 64);
        let mut cool_img = RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                warm_img.put_pixel(x, y, Rgba([240, 90, 25, 255]));
                cool_img.put_pixel(x, y, Rgba([20, 100, 235, 255]));
            }
        }

        let (warm_vec, warm_labels) =
            compute_image_embedding_and_labels(&DynamicImage::ImageRgba8(warm_img), "");
        let (cool_vec, cool_labels) =
            compute_image_embedding_and_labels(&DynamicImage::ImageRgba8(cool_img), "");

        assert_eq!(warm_vec.len(), EMBEDDING_DIM);
        assert_eq!(cool_vec.len(), EMBEDDING_DIM);
        assert!(warm_labels.contains("orange") || warm_labels.contains("warm"));
        assert!(cool_labels.contains("blue") || cool_labels.contains("cool"));

        // 2. Query "orange sunset" -> must have much higher cosine similarity with warm_vec than cool_vec
        let (q_sunset, _) = project_text_query_to_512d("orange sunset fire");
        let sim_warm = cosine_similarity_i8(&q_sunset, &warm_vec);
        let sim_cool = cosine_similarity_i8(&q_sunset, &cool_vec);
        assert!(
            sim_warm > sim_cool,
            "Expected warm image similarity ({}) > cool image similarity ({})",
            sim_warm,
            sim_cool
        );

        // 3. Verify explicit lazy RAM unloader drops model from memory
        assert!(get_engine().model.lock().unwrap().is_some());
        unload_visual_model_now();
        assert!(get_engine().model.lock().unwrap().is_none());
    }
}
