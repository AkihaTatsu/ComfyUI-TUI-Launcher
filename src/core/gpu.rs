//! System-level GPU detection without Python dependencies.
//!
//! Enumerates GPUs via platform tools (`nvidia-smi`, `lspci`, PowerShell)
//! and labels each with its most likely compute backend. Results are cached
//! for the process lifetime.

use std::collections::HashSet;
use std::process::Command;
use std::sync::Mutex;

/// One detected GPU device.
#[derive(Debug, Clone)]
pub struct GpuInfo {
    /// Backend identifier: `"cuda"`, `"rocm"`, `"dml"`, or `"cpu"`.
    pub backend: String,
    /// Device index within the backend (0-based).
    pub id: i32,
    /// Human-readable device name, e.g. `NVIDIA GeForce RTX 4090`.
    pub name: String,
    /// Total VRAM in GiB (0 when unavailable).
    pub vram_gb: u64,
}

impl GpuInfo {
    /// Returns the config value stored for this device.
    ///
    /// Format: `cuda:0`, `rocm:0`, `dml:0`, or `cpu`.
    pub fn config_value(&self) -> String {
        if self.backend == "cpu" {
            "cpu".to_string()
        } else {
            format!("{}:{}", self.backend, self.id)
        }
    }

    /// Returns a display label including backend, name, and VRAM.
    pub fn display_label(&self) -> String {
        if self.backend == "cpu" {
            return "CPU".to_string();
        }
        let backend_upper = self.backend.to_uppercase();
        if self.vram_gb > 0 {
            format!(
                "{} GPU {}: {} ({} GB)",
                backend_upper, self.id, self.name, self.vram_gb
            )
        } else {
            format!("{} GPU {}: {}", backend_upper, self.id, self.name)
        }
    }
}

static CACHE: Mutex<Option<Vec<GpuInfo>>> = Mutex::new(None);

/// Detects available GPUs using platform tools.
///
/// Results are cached so repeated calls are free. A `CPU` entry is always
/// appended as the last option.
pub fn detect() -> Vec<GpuInfo> {
    {
        let cache = CACHE.lock().unwrap();
        if let Some(cached) = cache.as_ref() {
            return cached.clone();
        }
    }
    let mut gpus = Vec::new();
    let mut seen = HashSet::new();

    detect_nvidia_smi(&mut gpus, &mut seen);

    #[cfg(target_os = "linux")]
    detect_lspci(&mut gpus, &mut seen);

    #[cfg(target_os = "windows")]
    detect_windows(&mut gpus, &mut seen);

    #[cfg(target_os = "macos")]
    detect_macos(&mut gpus, &mut seen);

    gpus.push(cpu_entry());

    let mut cache = CACHE.lock().unwrap();
    *cache = Some(gpus.clone());
    gpus
}

/// Returns the most recently cached detection results, or an empty list
/// if no detection has run yet.
pub fn last_detected() -> Vec<GpuInfo> {
    let cache = CACHE.lock().unwrap();
    match cache.as_ref() {
        Some(gpus) => gpus.clone(),
        None => Vec::new(),
    }
}

/// Returns a label for the "Default" GPU option that shows what ComfyUI
/// would actually use, e.g. `"Default - CUDA GPU 0: NVIDIA GeForce RTX 5090 (32 GB)"`.
///
/// ComfyUI's default is the first CUDA/ROCm device, or MPS on macOS, or
/// CPU if nothing else is available.
pub fn default_label(default_text: &str) -> String {
    let gpus = detect();
    let first_gpu = gpus.iter().find(|g| g.backend != "cpu");
    match first_gpu {
        Some(g) => format!("{} - {}", default_text, g.display_label()),
        None => default_text.to_string(),
    }
}

/// Clears the cached detection results so the next `detect` call re-runs
/// system enumeration.
#[allow(dead_code)]
pub fn invalidate_cache() {
    let mut cache = CACHE.lock().unwrap();
    *cache = None;
}

fn cpu_entry() -> GpuInfo {
    GpuInfo {
        backend: "cpu".to_string(),
        id: 0,
        name: "CPU".to_string(),
        vram_gb: 0,
    }
}

// ── nvidia-smi (Linux + Windows) ──────────────────────────────────────────

fn detect_nvidia_smi(gpus: &mut Vec<GpuInfo>, seen: &mut HashSet<String>) {
    let out = match Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let parts: Vec<&str> = line.splitn(3, ',').map(|s| s.trim()).collect();
        if parts.len() < 3 {
            continue;
        }
        let id: i32 = parts[0].parse().unwrap_or(0);
        let name = parts[1].to_string();
        let vram_gb = parts[2]
            .parse::<f64>()
            .map(|m| (m / 1024.0).round() as u64)
            .unwrap_or(0);
        let key = name.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);
        gpus.push(GpuInfo {
            backend: "cuda".to_string(),
            id,
            name,
            vram_gb,
        });
    }
}

// ── lspci (Linux) ─────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn detect_lspci(gpus: &mut Vec<GpuInfo>, seen: &mut HashSet<String>) {
    let out = match Command::new("lspci").output() {
        Ok(o) if o.status.success() => o,
        _ => return,
    };
    let text = String::from_utf8_lossy(&out.stdout);

    let mut rocm_idx: i32 = 0;
    let mut other_idx: i32 = 0;

    for line in text.lines() {
        let upper = line.to_uppercase();
        if !upper.contains("VGA") && !upper.contains("3D") && !upper.contains("DISPLAY") {
            continue;
        }
        // NVIDIA GPUs are handled by nvidia-smi with accurate VRAM.
        if upper.contains("NVIDIA") {
            continue;
        }

        // Extract the device name from between `]` and optional `(rev XX)`.
        // Example: "0c:00.0 VGA compatible controller: Advanced Micro Devices, Inc. [AMD/ATI] Granite Ridge [Radeon Graphics] (rev c9)"
        // We want "Granite Ridge [Radeon Graphics]" — take everything after
        // the LAST `] ` that follows the vendor bracket, up to `(rev`.
        let name = extract_lspci_name(line);
        if name.is_empty() {
            continue;
        }
        let key = name.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);

        let (backend, id) = if upper.contains("AMD") || upper.contains("ATI") {
            let id = rocm_idx;
            rocm_idx += 1;
            ("rocm", id)
        } else {
            let id = other_idx;
            other_idx += 1;
            ("other", id)
        };

        gpus.push(GpuInfo {
            backend: backend.to_string(),
            id,
            name,
            vram_gb: 0,
        });
    }

    // Try to fill in VRAM for AMD GPUs via /sys or rocm-smi.
    for gpu in gpus.iter_mut() {
        if gpu.backend == "rocm" && gpu.vram_gb == 0 {
            gpu.vram_gb = amd_vram_from_sys(gpu.id);
        }
    }
}

/// Extracts a readable device name from an lspci output line.
///
/// Handles formats like:
///   `... [AMD/ATI] Granite Ridge [Radeon Graphics] (rev c9)`
///   `... Intel Corporation UHD Graphics 770`
#[cfg(target_os = "linux")]
fn extract_lspci_name(line: &str) -> String {
    // Find the part after the device class description (after the `: `).
    let after_colon = match line.find(": ") {
        Some(i) => &line[i + 2..],
        None => return String::new(),
    };
    // Strip trailing `(rev XX)`.
    let trimmed = match after_colon.rfind("(rev ") {
        Some(i) => after_colon[..i].trim(),
        None => after_colon.trim(),
    };
    // If there's a vendor bracket like `[AMD/ATI]`, take everything after it.
    // Look for `] ` pattern — the name follows the last vendor bracket.
    if let Some(i) = trimmed.find("] ") {
        let after_bracket = trimmed[i + 2..].trim();
        if !after_bracket.is_empty() {
            return after_bracket.to_string();
        }
    }
    trimmed.to_string()
}

/// Reads AMD GPU VRAM from sysfs.
#[cfg(target_os = "linux")]
fn amd_vram_from_sys(device_idx: i32) -> u64 {
    // Try rocm-smi first.
    if let Some(vram) = amd_vram_rocm_smi(device_idx) {
        return vram;
    }
    // Walk /sys/class/drm/cardN/device/mem_info_vram_total for amdgpu.
    for entry in std::fs::read_dir("/sys/class/drm").into_iter().flatten() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("card") || name_str.contains('-') {
            continue;
        }
        let vram_path = entry.path().join("device/mem_info_vram_total");
        if let Ok(contents) = std::fs::read_to_string(&vram_path) {
            if let Ok(bytes) = contents.trim().parse::<u64>() {
                let gb = bytes / (1024 * 1024 * 1024);
                if gb > 0 {
                    return gb;
                }
            }
        }
    }
    0
}

/// Queries `rocm-smi` for VRAM of a specific device.
#[cfg(target_os = "linux")]
fn amd_vram_rocm_smi(device_idx: i32) -> Option<u64> {
    let out = Command::new("rocm-smi")
        .args(["--showmeminfo", "vram", "--json"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // rocm-smi JSON: {"card0": {"VRAM Total Memory (B)": "..."}, ...}
    let obj: serde_json::Value = serde_json::from_str(&text).ok()?;
    let key = format!("card{device_idx}");
    let card = obj.get(&key)?;
    let total = card
        .get("VRAM Total Memory (B)")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())?;
    Some(total / (1024 * 1024 * 1024))
}

// ── Windows (PowerShell) ──────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn detect_windows(gpus: &mut Vec<GpuInfo>, seen: &mut HashSet<String>) {
    let out = match Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_VideoController | Select-Object Name,AdapterRAM | ConvertTo-Json -Compress",
        ])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let data: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let arr = match &data {
        serde_json::Value::Array(a) => a.clone(),
        obj @ serde_json::Value::Object(_) => vec![obj.clone()],
        _ => return,
    };

    let mut cuda_idx: i32 = 0;
    let mut dml_idx: i32 = 0;

    for item in &arr {
        let name = item
            .get("Name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let key = name.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);

        let ram = item
            .get("AdapterRAM")
            .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
            .unwrap_or(0);
        let vram_gb = ram / (1024 * 1024 * 1024);

        let upper = name.to_uppercase();
        if upper.contains("NVIDIA") {
            // NVIDIA GPU: emit as both CUDA and DML so the user can pick.
            gpus.push(GpuInfo {
                backend: "cuda".to_string(),
                id: cuda_idx,
                name: name.clone(),
                vram_gb,
            });
            cuda_idx += 1;
            gpus.push(GpuInfo {
                backend: "dml".to_string(),
                id: dml_idx,
                name,
                vram_gb,
            });
            dml_idx += 1;
        } else {
            // AMD / Intel / other: DML is the available backend on Windows.
            gpus.push(GpuInfo {
                backend: "dml".to_string(),
                id: dml_idx,
                name,
                vram_gb,
            });
            dml_idx += 1;
        }
    }
}

// ── macOS (system_profiler) ───────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn detect_macos(gpus: &mut Vec<GpuInfo>, seen: &mut HashSet<String>) {
    let out = match Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let root: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let displays = match root.get("SPDisplaysDataType").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return,
    };
    for (i, entry) in displays.iter().enumerate() {
        // "sppci_model" is the GPU name on both Intel Macs and Apple Silicon.
        // Apple Silicon entries also carry "sppci_bus" = "sppci_gpu_builtin".
        let name = entry
            .get("sppci_model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let key = name.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);

        // Discrete GPUs report VRAM via "spdisplays_vram" (e.g. "8 GB").
        // Apple Silicon uses unified memory — try "spdisplays_vram" first,
        // then fall back to reading total system RAM as a proxy.
        let vram_gb: u64 = entry
            .get("spdisplays_vram")
            .and_then(|v| v.as_str())
            .and_then(|s| {
                let num = s.split_whitespace().next()?;
                let gb: u64 = num.parse().ok()?;
                // "spdisplays_vram" may report in MB for older entries.
                if s.contains("MB") {
                    Some(gb / 1024)
                } else {
                    Some(gb)
                }
            })
            .unwrap_or_else(macos_unified_memory_gb);

        gpus.push(GpuInfo {
            backend: "mps".to_string(),
            id: i as i32,
            name,
            vram_gb,
        });
    }
}

/// Returns total system RAM in GiB on macOS (used as a proxy for Apple
/// Silicon unified memory when per-GPU VRAM is not reported).
#[cfg(target_os = "macos")]
fn macos_unified_memory_gb() -> u64 {
    let out = match Command::new("sysctl").args(["-n", "hw.memsize"]).output() {
        Ok(o) if o.status.success() => o,
        _ => return 0,
    };
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .map(|b| b / (1024 * 1024 * 1024))
        .unwrap_or(0)
}

/// Converts a stored config value (e.g. `"cuda:1"`) into CLI arguments for
/// ComfyUI.
///
/// - `cuda:N` → `--cuda-device N`
/// - `rocm:N` → `--cuda-device N` (ROCm uses the CUDA API in PyTorch)
/// - `dml:N`  → `--directml N`
/// - `mps:N`  → nothing (PyTorch uses MPS automatically on macOS)
/// - `cpu`    → `--cpu`
/// - `""`     → nothing (auto)
pub fn config_value_to_cli_args(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    if value == "cpu" {
        return vec!["--cpu".to_string()];
    }
    if let Some(rest) = value.strip_prefix("cuda:") {
        return vec!["--cuda-device".to_string(), rest.to_string()];
    }
    if let Some(rest) = value.strip_prefix("rocm:") {
        return vec!["--cuda-device".to_string(), rest.to_string()];
    }
    if value.starts_with("mps:") {
        return Vec::new();
    }
    if let Some(rest) = value.strip_prefix("dml:") {
        if let Ok(id) = rest.parse::<i32>() {
            if id < 0 {
                return vec!["--directml".to_string()];
            }
            return vec!["--directml".to_string(), id.to_string()];
        }
        return vec!["--directml".to_string()];
    }
    Vec::new()
}
