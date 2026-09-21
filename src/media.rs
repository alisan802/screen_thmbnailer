use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub file_name: String,
    pub duration_seconds: f64,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub frame_rate: Option<f64>,
    pub video_codec: String,
    pub audio_codec: Option<String>,
}

impl MediaInfo {
    pub fn duration_label(&self) -> String {
        format_timestamp(self.duration_seconds)
    }
}

#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("ファイルが存在しないか、通常のファイルではありません")]
    InvalidPath,
    #[error(
        "ffprobeを起動できません。FFmpegがインストールされ、PATHに設定されているか確認してください: {0}"
    )]
    MissingFfprobe(#[source] std::io::Error),
    #[error("動画情報を取得できません: {0}")]
    ProbeFailed(String),
    #[error("ffprobeの応答を解析できません: {0}")]
    InvalidResponse(#[from] serde_json::Error),
    #[error("映像ストリームが見つかりません")]
    NoVideo,
    #[error("動画の長さを取得できません")]
    NoDuration,
}

#[derive(Deserialize)]
struct ProbeResponse {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    duration: Option<String>,
    avg_frame_rate: Option<String>,
    #[serde(default)]
    tags: std::collections::HashMap<String, String>,
    #[serde(default)]
    side_data_list: Vec<SideData>,
}

#[derive(Deserialize)]
struct SideData {
    rotation: Option<i32>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
    size: Option<String>,
}

pub fn probe(path: &Path) -> Result<MediaInfo, ProbeError> {
    if !path.is_file() {
        return Err(ProbeError::InvalidPath);
    }

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration,size:stream=codec_type,codec_name,width,height,duration,avg_frame_rate:stream_tags=rotate:stream_side_data=rotation",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(ProbeError::MissingFfprobe)?;

    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(ProbeError::ProbeFailed(if reason.is_empty() {
            "未対応形式または破損したファイルです".into()
        } else {
            reason
        }));
    }

    let response: ProbeResponse = serde_json::from_slice(&output.stdout)?;
    let video = response
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"))
        .ok_or(ProbeError::NoVideo)?;
    let audio = response
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("audio"));
    let format = response.format.as_ref();
    let duration_seconds = parse_number(format.and_then(|f| f.duration.as_deref()))
        .or_else(|| parse_number(video.duration.as_deref()))
        .filter(|n| n.is_finite() && *n > 0.0)
        .ok_or(ProbeError::NoDuration)?;
    let mut width = video.width.unwrap_or(0);
    let mut height = video.height.unwrap_or(0);
    if width == 0 || height == 0 {
        return Err(ProbeError::NoVideo);
    }
    let rotation = video
        .side_data_list
        .iter()
        .find_map(|data| data.rotation)
        .or_else(|| video.tags.get("rotate").and_then(|v| v.parse().ok()))
        .unwrap_or(0);
    if rotation.rem_euclid(180) == 90 {
        std::mem::swap(&mut width, &mut height);
    }

    Ok(MediaInfo {
        path: path.to_owned(),
        file_name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        duration_seconds,
        width,
        height,
        file_size: format
            .and_then(|f| f.size.as_deref())
            .and_then(|v| v.parse().ok())
            .or_else(|| path.metadata().ok().map(|m| m.len()))
            .unwrap_or(0),
        frame_rate: parse_rate(video.avg_frame_rate.as_deref()),
        video_codec: video.codec_name.clone().unwrap_or_else(|| "不明".into()),
        audio_codec: audio.and_then(|stream| stream.codec_name.clone()),
    })
}

fn parse_number(value: Option<&str>) -> Option<f64> {
    value?.parse().ok()
}

fn parse_rate(value: Option<&str>) -> Option<f64> {
    let value = value?;
    let (numerator, denominator) = value.split_once('/')?;
    let numerator: f64 = numerator.parse().ok()?;
    let denominator: f64 = denominator.parse().ok()?;
    (denominator != 0.0).then_some(numerator / denominator)
}

pub fn format_timestamp(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_has_expected_shape() {
        assert_eq!(format_timestamp(65.2), "01:05");
        assert_eq!(format_timestamp(3661.0), "01:01:01");
    }

    #[test]
    fn parses_fractional_frame_rate() {
        assert!((parse_rate(Some("30000/1001")).unwrap() - 29.970).abs() < 0.001);
    }
}
