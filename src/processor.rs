use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use ab_glyph::{FontArc, PxScale};
use image::{DynamicImage, ImageFormat as EncoderFormat, Rgb, RgbImage};
use imageproc::drawing::{draw_filled_rect_mut, draw_hollow_rect_mut, draw_text_mut, text_size};
use imageproc::rect::Rect;
use tempfile::Builder;

use crate::{
    config::{CaptureMode, ConflictPolicy, ImageFormat, JobSettings},
    media::{MediaInfo, format_timestamp},
};

#[derive(Debug, Clone)]
pub struct ProcessRequest {
    pub media: MediaInfo,
    pub settings: JobSettings,
}

#[derive(Debug)]
pub enum ProcessEvent {
    Progress { fraction: f32, message: String },
    Finished(Result<ProcessSummary, String>),
}

#[derive(Debug)]
pub struct ProcessSummary {
    pub output_directory: PathBuf,
    pub files: Vec<PathBuf>,
    pub warnings: Vec<String>,
    pub elapsed: Duration,
}

pub struct ProcessHandle {
    pub receiver: Receiver<ProcessEvent>,
    cancel: Arc<AtomicBool>,
}

impl ProcessHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn start(request: ProcessRequest) -> ProcessHandle {
    let (sender, receiver) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    thread::spawn(move || {
        let result = run(request, &sender, &worker_cancel);
        let _ = sender.send(ProcessEvent::Finished(result));
    });
    ProcessHandle { receiver, cancel }
}

fn run(
    request: ProcessRequest,
    sender: &Sender<ProcessEvent>,
    cancel: &AtomicBool,
) -> Result<ProcessSummary, String> {
    let started = Instant::now();
    let settings = &request.settings;
    settings.validate(request.media.duration_seconds)?;
    validate_estimated_output(&request.media, settings)?;
    let output_directory = output_directory(&request)?;
    fs::create_dir_all(&output_directory)
        .map_err(|e| format!("出力フォルダーを作成できません: {e}"))?;
    let temp = tempfile::tempdir().map_err(|e| format!("一時フォルダーを作成できません: {e}"))?;
    let times = capture_times(settings, request.media.duration_seconds)?;
    let mut warnings = Vec::new();
    if times.len() < settings.thumbnail_count as usize {
        warnings.push(format!(
            "指定条件で抽出可能な画像は{}枚でした（指定: {}枚）。",
            times.len(),
            settings.thumbnail_count
        ));
    }

    let font = load_font();
    if (settings.show_timestamp || settings.show_header) && font.is_none() {
        return Err("文字描画用フォントが見つかりません。Noto Sans、Yu Gothic、Meiryo、DejaVu Sansのいずれかをインストールしてください。".into());
    }

    let mut frames = Vec::with_capacity(times.len());
    let mut hashes: HashMap<u64, usize> = HashMap::new();
    for (index, timestamp) in times.iter().copied().enumerate() {
        ensure_not_cancelled(cancel)?;
        send_progress(
            sender,
            index as f32 / (times.len() as f32 + 2.0),
            format!("サムネイルを抽出中… {}/{}", index + 1, times.len()),
        );
        let path = temp.path().join(format!("frame_{index:04}.png"));
        extract_frame(
            &request.media.path,
            timestamp,
            settings.thumbnail_width,
            &path,
            cancel,
        )?;
        let mut image = image::open(&path)
            .map_err(|e| format!("抽出画像を読み込めません（{}枚目）: {e}", index + 1))?
            .to_rgb8();
        let mut hasher = DefaultHasher::new();
        image.as_raw().hash(&mut hasher);
        let hash = hasher.finish();
        if let Some(previous) = hashes.insert(hash, index) {
            warnings.push(format!(
                "{}枚目と{}枚目は同じフレームの可能性があります。",
                previous + 1,
                index + 1
            ));
        }
        if settings.show_timestamp {
            draw_timestamp(&mut image, timestamp, font.as_ref().expect("checked above"));
        }
        frames.push(image);
    }

    ensure_not_cancelled(cancel)?;
    send_progress(sender, 0.85, "コンタクトシートを作成中…".into());
    let sheet = compose_sheet(&frames, &request.media, settings, font.as_ref())?;
    let base_name = output_base_name(&request.media.path, settings);
    let mut files = Vec::new();
    let sheet_name = format!("{base_name}_contact.{}", settings.image_format.extension());
    if let Some(path) = save_image(&sheet, &output_directory.join(sheet_name), settings)? {
        files.push(path);
    }

    if settings.output_individual {
        for (index, frame) in frames.iter().enumerate() {
            ensure_not_cancelled(cancel)?;
            send_progress(
                sender,
                0.88 + 0.1 * ((index + 1) as f32 / frames.len() as f32),
                format!("個別画像を保存中… {}/{}", index + 1, frames.len()),
            );
            let name = format!(
                "{base_name}_{:03}.{}",
                index + 1,
                settings.image_format.extension()
            );
            if let Some(path) = save_image(
                &DynamicImage::ImageRgb8(frame.clone()),
                &output_directory.join(name),
                settings,
            )? {
                files.push(path);
            }
        }
    }

    ensure_not_cancelled(cancel)?;
    send_progress(sender, 1.0, "完了".into());
    Ok(ProcessSummary {
        output_directory,
        files,
        warnings,
        elapsed: started.elapsed(),
    })
}

fn output_directory(request: &ProcessRequest) -> Result<PathBuf, String> {
    if request.settings.use_input_directory {
        request
            .media
            .path
            .parent()
            .map(Path::to_owned)
            .ok_or_else(|| "入力動画のフォルダーを取得できません。".into())
    } else {
        Ok(request.settings.output_directory.clone())
    }
}

fn validate_estimated_output(media: &MediaInfo, settings: &JobSettings) -> Result<(), String> {
    let columns = settings.columns.min(settings.thumbnail_count).max(1) as u64;
    let rows = (settings.thumbnail_count as u64).div_ceil(columns);
    let cell_width = settings.thumbnail_width as u64;
    let cell_height =
        ((cell_width as f64 * media.height as f64 / media.width as f64).ceil() as u64).max(1);
    let estimated_width = settings.outer_margin as u64 * 2
        + cell_width * columns
        + settings.gap as u64 * columns.saturating_sub(1);
    let estimated_height = settings.outer_margin as u64 * 2
        + cell_height * rows
        + settings.gap as u64 * rows.saturating_sub(1)
        + if settings.show_header { 300 } else { 0 };
    if estimated_width > 32_768
        || estimated_height > 32_768
        || estimated_width.saturating_mul(estimated_height) > 60_000_000
    {
        return Err(
            "出力画像が大きすぎます。枚数、列数、またはサムネイル幅を小さくしてください。".into(),
        );
    }
    Ok(())
}

pub(crate) fn capture_times(settings: &JobSettings, duration: f64) -> Result<Vec<f64>, String> {
    let start = settings.trim_start_seconds.max(0.0);
    let end = duration - settings.trim_end_seconds.max(0.0);
    if end <= start {
        return Err("抽出可能な時間範囲がありません。".into());
    }
    let count = settings.thumbnail_count as usize;
    let times = match settings.capture_mode {
        CaptureMode::EvenlySpaced => {
            let step = (end - start) / count as f64;
            (0..count)
                .map(|index| start + step * (index as f64 + 0.5))
                .collect()
        }
        CaptureMode::FixedInterval => {
            let mut result = Vec::with_capacity(count);
            let mut timestamp = start;
            while timestamp < end && result.len() < count {
                result.push(timestamp);
                timestamp += settings.interval_seconds;
            }
            result
        }
    };
    Ok(times)
}

fn extract_frame(
    input: &Path,
    timestamp: f64,
    width: u32,
    output: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    // stderr is redirected to a file instead of a pipe. This prevents FFmpeg from
    // blocking when an unusually large diagnostic fills the OS pipe buffer.
    let mut error_log =
        tempfile::tempfile().map_err(|e| format!("ffmpeg用の一時ログを作成できません: {e}"))?;
    let error_output = error_log
        .try_clone()
        .map_err(|e| format!("ffmpeg用の一時ログを準備できません: {e}"))?;
    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-ss"])
        .arg(format!("{timestamp:.6}"))
        .arg("-i")
        .arg(input)
        .args(["-map", "0:v:0", "-frames:v", "1", "-an", "-vf"])
        .arg(format!("scale={width}:-2:flags=lanczos"))
        .args(["-y"])
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(error_output))
        .spawn()
        .map_err(|e| {
            format!("ffmpegを起動できません。FFmpegがPATHに設定されているか確認してください: {e}")
        })?;

    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("処理をキャンセルしました。不完全な出力は保存していません。".into());
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {
                let mut reason = String::new();
                let _ = error_log.seek(SeekFrom::Start(0));
                let _ = error_log.read_to_string(&mut reason);
                let reason = reason.trim();
                return Err(if reason.is_empty() {
                    format!("{timestamp:.2}秒のフレームを抽出できません。")
                } else {
                    format!("{timestamp:.2}秒のフレームを抽出できません: {reason}")
                });
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("ffmpegの状態を確認できません: {e}")),
        }
    }
}

fn draw_timestamp(image: &mut RgbImage, timestamp: f64, font: &FontArc) {
    let text = format_timestamp(timestamp);
    let size = (image.width() as f32 / 22.0).clamp(14.0, 42.0);
    let scale = PxScale::from(size);
    let (text_width, text_height) = text_size(scale, font, &text);
    let padding = (size / 4.0).max(4.0) as u32;
    let x = image.width().saturating_sub(text_width + padding * 2);
    let y = image.height().saturating_sub(text_height + padding * 2);
    draw_filled_rect_mut(
        image,
        Rect::at(x as i32, y as i32).of_size(text_width + padding * 2, text_height + padding * 2),
        Rgb([0, 0, 0]),
    );
    draw_text_mut(
        image,
        Rgb([255, 255, 255]),
        (x + padding) as i32,
        (y + padding) as i32,
        scale,
        font,
        &text,
    );
}

fn compose_sheet(
    frames: &[RgbImage],
    media: &MediaInfo,
    settings: &JobSettings,
    font: Option<&FontArc>,
) -> Result<DynamicImage, String> {
    let first = frames
        .first()
        .ok_or_else(|| "抽出画像がありません。".to_owned())?;
    let columns = settings.columns.min(frames.len() as u32).max(1);
    let rows = (frames.len() as u32).div_ceil(columns);
    let cell_width = frames
        .iter()
        .map(RgbImage::width)
        .max()
        .unwrap_or(first.width());
    let cell_height = frames
        .iter()
        .map(RgbImage::height)
        .max()
        .unwrap_or(first.height());
    let header_height = if settings.show_header {
        (settings.font_size.max(12.0) * 3.4) as u32 + settings.gap
    } else {
        0
    };
    let width = checked_dimension(
        settings.outer_margin.saturating_mul(2) as u64
            + cell_width as u64 * columns as u64
            + settings.gap as u64 * columns.saturating_sub(1) as u64,
    )?;
    let height = checked_dimension(
        settings.outer_margin.saturating_mul(2) as u64
            + header_height as u64
            + cell_height as u64 * rows as u64
            + settings.gap as u64 * rows.saturating_sub(1) as u64,
    )?;
    if width as u64 * height as u64 > 60_000_000 {
        return Err("出力画像の総画素数が大きすぎます。設定を小さくしてください。".into());
    }
    let mut canvas = RgbImage::from_pixel(width, height, Rgb(settings.background));

    if settings.show_header {
        let font = font.expect("font checked by caller");
        let title = if settings.custom_title.trim().is_empty() {
            media.file_name.as_str()
        } else {
            settings.custom_title.trim()
        };
        let details = format!(
            "長さ: {}    映像: {}×{}    {} 枚",
            media.duration_label(),
            media.width,
            media.height,
            frames.len()
        );
        let title_scale = PxScale::from(settings.font_size.max(12.0));
        let detail_scale = PxScale::from((settings.font_size * 0.78).max(10.0));
        let x = settings.outer_margin as i32;
        let y = settings.outer_margin as i32;
        draw_text_mut(
            &mut canvas,
            Rgb(settings.text_color),
            x,
            y,
            title_scale,
            font,
            title,
        );
        draw_text_mut(
            &mut canvas,
            Rgb(settings.text_color),
            x,
            y + (settings.font_size * 1.45) as i32,
            detail_scale,
            font,
            &details,
        );
    }

    for (index, frame) in frames.iter().enumerate() {
        let column = index as u32 % columns;
        let row = index as u32 / columns;
        let x = settings.outer_margin + column * (cell_width + settings.gap);
        let y = settings.outer_margin + header_height + row * (cell_height + settings.gap);
        image::imageops::overlay(&mut canvas, frame, x as i64, y as i64);
        if settings.border_width > 0 {
            for inset in 0..settings
                .border_width
                .min(frame.width() / 2)
                .min(frame.height() / 2)
            {
                let rect = Rect::at((x + inset) as i32, (y + inset) as i32).of_size(
                    frame.width().saturating_sub(inset * 2),
                    frame.height().saturating_sub(inset * 2),
                );
                draw_hollow_rect_mut(&mut canvas, rect, Rgb(settings.border_color));
            }
        }
    }
    Ok(DynamicImage::ImageRgb8(canvas))
}

fn checked_dimension(value: u64) -> Result<u32, String> {
    let value: u32 = value
        .try_into()
        .map_err(|_| "出力画像のサイズが大きすぎます。".to_owned())?;
    if value > 32_768 {
        return Err("出力画像の一辺が32,768 pxを超えます。設定を小さくしてください。".into());
    }
    Ok(value)
}

fn save_image(
    image: &DynamicImage,
    requested_path: &Path,
    settings: &JobSettings,
) -> Result<Option<PathBuf>, String> {
    let Some(destination) = resolve_destination(requested_path, settings.conflict_policy) else {
        return Ok(None);
    };
    let parent = destination
        .parent()
        .ok_or_else(|| "出力先が不正です。".to_owned())?;
    let mut temp = Builder::new()
        .prefix(".screen-thumbnailer-")
        .suffix(".part")
        .tempfile_in(parent)
        .map_err(|e| format!("出力用一時ファイルを作成できません: {e}"))?;
    match settings.image_format {
        ImageFormat::Jpeg => {
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                temp.as_file_mut(),
                settings.jpeg_quality,
            );
            encoder
                .encode_image(image)
                .map_err(|e| format!("JPEGを書き込めません: {e}"))?;
        }
        ImageFormat::Png => {
            image
                .write_to(temp.as_file_mut(), EncoderFormat::Png)
                .map_err(|e| format!("PNGを書き込めません: {e}"))?;
        }
    }
    temp.as_file_mut()
        .flush()
        .map_err(|e| format!("画像を保存できません: {e}"))?;
    temp.as_file_mut().seek(SeekFrom::Start(0)).ok();
    if settings.conflict_policy == ConflictPolicy::Overwrite && destination.exists() {
        fs::remove_file(&destination).map_err(|e| {
            format!(
                "既存ファイルを上書きできません（{}）: {e}",
                destination.display()
            )
        })?;
    }
    temp.persist(&destination).map_err(|e| {
        format!(
            "画像を確定できません（{}）: {}",
            destination.display(),
            e.error
        )
    })?;
    Ok(Some(destination))
}

pub(crate) fn resolve_destination(path: &Path, policy: ConflictPolicy) -> Option<PathBuf> {
    if !path.exists() || policy == ConflictPolicy::Overwrite {
        return Some(path.to_owned());
    }
    if policy == ConflictPolicy::Skip {
        return None;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path.extension().unwrap_or_default().to_string_lossy();
    for number in 2..=9999 {
        let candidate = if extension.is_empty() {
            parent.join(format!("{stem} ({number})"))
        } else {
            parent.join(format!("{stem} ({number}).{extension}"))
        };
        if !candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn output_base_name(input: &Path, settings: &JobSettings) -> String {
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    format!(
        "{}{stem}{}",
        settings.filename_prefix, settings.filename_suffix
    )
}

fn ensure_not_cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("処理をキャンセルしました。不完全な出力は保存していません。".into())
    } else {
        Ok(())
    }
}

fn send_progress(sender: &Sender<ProcessEvent>, fraction: f32, message: String) {
    let _ = sender.send(ProcessEvent::Progress { fraction, message });
}

fn load_font() -> Option<FontArc> {
    #[cfg(target_os = "windows")]
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\arial.ttf",
    ];
    #[cfg(not(target_os = "windows"))]
    const CANDIDATES: &[&str] = &[
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
    ];
    CANDIDATES.iter().find_map(|path| {
        fs::read(path)
            .ok()
            .and_then(|bytes| FontArc::try_from_vec(bytes).ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evenly_spaced_times_use_bin_centres() {
        let settings = JobSettings {
            thumbnail_count: 4,
            ..Default::default()
        };
        assert_eq!(
            capture_times(&settings, 40.0).unwrap(),
            vec![5.0, 15.0, 25.0, 35.0]
        );
    }

    #[test]
    fn fixed_interval_stops_at_end() {
        let settings = JobSettings {
            capture_mode: CaptureMode::FixedInterval,
            thumbnail_count: 10,
            interval_seconds: 3.0,
            trim_start_seconds: 1.0,
            trim_end_seconds: 1.0,
            ..Default::default()
        };
        assert_eq!(capture_times(&settings, 10.0).unwrap(), vec![1.0, 4.0, 7.0]);
    }

    #[test]
    fn end_to_end_with_ffmpeg_when_available() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let video = directory.path().join("日本語 sample.mp4");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=duration=2:size=160x90:rate=10",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&video)
            .status()
            .unwrap();
        if !status.success() {
            // A minimal FFmpeg build may not contain a suitable MP4 encoder.
            return;
        }
        let media = crate::media::probe(&video).unwrap();
        let settings = JobSettings {
            thumbnail_count: 3,
            columns: 2,
            thumbnail_width: 160,
            show_timestamp: false,
            show_header: false,
            output_individual: true,
            image_format: ImageFormat::Png,
            ..Default::default()
        };
        let (sender, _receiver) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let result = run(ProcessRequest { media, settings }, &sender, &cancel).unwrap();
        assert_eq!(result.files.len(), 4);
        assert!(result.files.iter().all(|path| path.is_file()));
        let sheet = image::open(&result.files[0]).unwrap();
        assert!(sheet.width() >= 320);
        assert!(sheet.height() >= 180);
    }
}
