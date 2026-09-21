use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureMode {
    EvenlySpaced,
    FixedInterval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFormat {
    Jpeg,
    Png,
}

impl ImageFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictPolicy {
    Rename,
    Overwrite,
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct JobSettings {
    pub capture_mode: CaptureMode,
    pub thumbnail_count: u32,
    pub interval_seconds: f64,
    pub trim_start_seconds: f64,
    pub trim_end_seconds: f64,
    pub columns: u32,
    pub thumbnail_width: u32,
    pub gap: u32,
    pub outer_margin: u32,
    pub background: [u8; 3],
    pub border_width: u32,
    pub border_color: [u8; 3],
    pub show_timestamp: bool,
    pub show_header: bool,
    pub custom_title: String,
    pub font_size: f32,
    pub text_color: [u8; 3],
    pub output_individual: bool,
    pub image_format: ImageFormat,
    pub jpeg_quality: u8,
    pub use_input_directory: bool,
    pub output_directory: PathBuf,
    pub filename_prefix: String,
    pub filename_suffix: String,
    pub conflict_policy: ConflictPolicy,
}

impl Default for JobSettings {
    fn default() -> Self {
        Self {
            capture_mode: CaptureMode::EvenlySpaced,
            thumbnail_count: 12,
            interval_seconds: 60.0,
            trim_start_seconds: 0.0,
            trim_end_seconds: 0.0,
            columns: 4,
            thumbnail_width: 320,
            gap: 8,
            outer_margin: 16,
            background: [28, 30, 34],
            border_width: 1,
            border_color: [100, 104, 112],
            show_timestamp: true,
            show_header: true,
            custom_title: String::new(),
            font_size: 20.0,
            text_color: [240, 240, 240],
            output_individual: false,
            image_format: ImageFormat::Jpeg,
            jpeg_quality: 90,
            use_input_directory: true,
            output_directory: PathBuf::new(),
            filename_prefix: String::new(),
            filename_suffix: String::new(),
            conflict_policy: ConflictPolicy::Rename,
        }
    }
}

impl JobSettings {
    pub fn validate(&self, duration: f64) -> Result<(), String> {
        if !(1..=100).contains(&self.thumbnail_count) {
            return Err("サムネイル枚数は1〜100枚にしてください。".into());
        }
        if !(1..=20).contains(&self.columns) {
            return Err("列数は1〜20列にしてください。".into());
        }
        if !(80..=1920).contains(&self.thumbnail_width) {
            return Err("サムネイル幅は80〜1920 pxにしてください。".into());
        }
        if self.gap > 100 || self.outer_margin > 200 || self.border_width > 20 {
            return Err("余白または枠線の値が設定可能範囲を超えています。".into());
        }
        if !(10.0..=72.0).contains(&self.font_size) {
            return Err("文字サイズは10〜72 pxにしてください。".into());
        }
        if self.trim_start_seconds < 0.0 || self.trim_end_seconds < 0.0 {
            return Err("除外時間は0秒以上にしてください。".into());
        }
        if self.capture_mode == CaptureMode::FixedInterval && self.interval_seconds < 0.1 {
            return Err("抽出間隔は0.1秒以上にしてください。".into());
        }
        if self.trim_start_seconds + self.trim_end_seconds >= duration {
            return Err("先頭・末尾の除外時間が動画の長さ以上です。".into());
        }
        if self.image_format == ImageFormat::Jpeg && !(1..=100).contains(&self.jpeg_quality) {
            return Err("JPEG画質は1〜100にしてください。".into());
        }
        if !self.use_input_directory && self.output_directory.as_os_str().is_empty() {
            return Err("出力フォルダーを選択してください。".into());
        }
        if [&self.filename_prefix, &self.filename_suffix]
            .into_iter()
            .any(|part| {
                part.chars()
                    .any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
            })
        {
            return Err(
                "接頭辞・接尾辞にファイル名として使用できない文字が含まれています。".into(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StoredConfig {
    pub settings: JobSettings,
    pub presets: BTreeMap<String, JobSettings>,
}

impl StoredConfig {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        let path =
            config_path().ok_or_else(|| io::Error::other("設定フォルダーを取得できません"))?;
        save_json_atomic(&path, self)
    }

    pub fn export_to(&self, path: &Path) -> io::Result<()> {
        save_json_atomic(path, self)
    }

    pub fn import_from(path: &Path) -> Result<Self, String> {
        let bytes = fs::read(path).map_err(|e| format!("設定ファイルを読めません: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("設定ファイルの形式が不正です: {e}"))
    }
}

fn config_path() -> Option<PathBuf> {
    ProjectDirs::from("jp", "screen-thumbnailer", "screen-thumbnailer")
        .map(|dirs| dirs.config_dir().join("settings.json"))
}

fn save_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, data)?;
    replace_file(&temp, path)
}

fn replace_file(temp: &Path, destination: &Path) -> io::Result<()> {
    // Unix rename replaces atomically. Windows rename does not replace an
    // existing file, so keep the previous valid settings as a recoverable
    // backup until the new file is in place.
    #[cfg(not(target_os = "windows"))]
    {
        fs::rename(temp, destination)
    }
    #[cfg(target_os = "windows")]
    {
        if !destination.exists() {
            return fs::rename(temp, destination);
        }
        let backup = destination.with_extension("json.bak");
        if backup.exists() {
            fs::remove_file(&backup)?;
        }
        fs::rename(destination, &backup)?;
        match fs::rename(temp, destination) {
            Ok(()) => {
                let _ = fs::remove_file(backup);
                Ok(())
            }
            Err(error) => {
                let _ = fs::rename(backup, destination);
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert!(JobSettings::default().validate(120.0).is_ok());
    }

    #[test]
    fn rejects_trim_larger_than_video() {
        let settings = JobSettings {
            trim_start_seconds: 6.0,
            trim_end_seconds: 4.0,
            ..Default::default()
        };
        assert!(settings.validate(10.0).is_err());
    }
}
