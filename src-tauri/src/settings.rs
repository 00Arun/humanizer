use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

pub const DEFAULT_MODEL: &str = "claude-fable-5-1";

/// Models the rewrite engine may run on. All three accept `fallbacks: "default"`.
pub const ALLOWED_MODELS: &[&str] = &["claude-fable-5-1", "claude-opus-5-5", "claude-sonnet-5-5"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Anthropic API key used by the rewrite engine and the AI Judge detector.
    pub anthropic_api_key: String,
    pub model: String,
    /// `output_config.effort`: low | medium | high | xhigh | max
    pub effort: String,
    pub default_style: String,
    pub default_intensity: String,
    /// Run the model-based "AI Judge" alongside the five local detectors.
    pub ai_judge_enabled: bool,
    /// auto | api | local | rules. Auto = API key if set, else the offline
    /// model if downloaded, else rules.
    pub engine_mode: String,
    /// Custom GGUF path; empty means the default download location.
    pub local_model_path: String,
    /// Fetch the offline model by itself on first launch (no click needed).
    pub auto_download_model: bool,
    /// auto | best | balanced | light. Auto = strongest installed tier, or
    /// the strongest the machine's RAM allows when nothing is installed yet.
    pub local_model_tier: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            anthropic_api_key: String::new(),
            model: DEFAULT_MODEL.to_string(),
            effort: "high".to_string(),
            default_style: "natural".to_string(),
            default_intensity: "balanced".to_string(),
            ai_judge_enabled: true,
            engine_mode: "auto".to_string(),
            local_model_path: String::new(),
            auto_download_model: true,
            local_model_tier: "auto".to_string(),
        }
    }
}

impl Settings {
    fn path(app: &AppHandle) -> Option<PathBuf> {
        app.path().app_config_dir().ok().map(|d| d.join("settings.json"))
    }

    pub fn load(app: &AppHandle) -> Self {
        let Some(path) = Self::path(app) else {
            return Self::default();
        };
        let mut s: Settings = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        s.normalize();
        s
    }

    pub fn save(&self, app: &AppHandle) -> Result<(), String> {
        let path = Self::path(app).ok_or("no config directory available")?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let raw = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, raw).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn normalize(&mut self) {
        self.anthropic_api_key = self.anthropic_api_key.trim().to_string();
        if !ALLOWED_MODELS.contains(&self.model.as_str()) {
            self.model = DEFAULT_MODEL.to_string();
        }
        if !["low", "medium", "high", "xhigh", "max"].contains(&self.effort.as_str()) {
            self.effort = "high".to_string();
        }
        if !["auto", "api", "local", "rules"].contains(&self.engine_mode.as_str()) {
            self.engine_mode = "auto".to_string();
        }
        self.local_model_path = self.local_model_path.trim().to_string();
        if !["auto", "best", "balanced", "light"].contains(&self.local_model_tier.as_str()) {
            self.local_model_tier = "auto".to_string();
        }
    }

    pub fn has_api_key(&self) -> bool {
        !self.anthropic_api_key.is_empty()
    }
}
