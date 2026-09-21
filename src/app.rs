use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
};

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, RichText};

use crate::{
    config::{CaptureMode, ConflictPolicy, ImageFormat, StoredConfig},
    media::{self, MediaInfo},
    processor::{self, ProcessEvent, ProcessHandle, ProcessRequest, ProcessSummary},
};

pub struct ThumbnailApp {
    config: StoredConfig,
    media: Option<MediaInfo>,
    probing: Option<Receiver<Result<MediaInfo, String>>>,
    selected_path: Option<PathBuf>,
    process: Option<ProcessHandle>,
    progress: f32,
    progress_message: String,
    result: Option<Result<ProcessSummary, String>>,
    preset_name: String,
    selected_preset: String,
    notice: Option<String>,
    confirm_overwrite: bool,
}

impl ThumbnailApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_japanese_ui_font(&cc.egui_ctx);
        Self {
            config: StoredConfig::load(),
            media: None,
            probing: None,
            selected_path: None,
            process: None,
            progress: 0.0,
            progress_message: String::new(),
            result: None,
            preset_name: String::new(),
            selected_preset: String::new(),
            notice: None,
            confirm_overwrite: false,
        }
    }

    fn select_video(&mut self, path: PathBuf) {
        self.selected_path = Some(path.clone());
        self.media = None;
        self.result = None;
        let (sender, receiver) = mpsc::channel();
        self.probing = Some(receiver);
        std::thread::spawn(move || {
            let result = media::probe(&path).map_err(|e| e.to_string());
            let _ = sender.send(result);
        });
    }

    fn poll_background_work(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.probing {
            match receiver.try_recv() {
                Ok(Ok(info)) => {
                    self.media = Some(info);
                    self.probing = None;
                }
                Ok(Err(error)) => {
                    self.notice = Some(format!(
                        "動画を読み込めません: {error}\n入力ファイルへの変更・削除は行っていません。"
                    ));
                    self.probing = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.notice = Some("動画情報の取得処理が予期せず終了しました。".into());
                    self.probing = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100))
                }
            }
        }

        let mut finished = None;
        if let Some(handle) = &self.process {
            loop {
                match handle.receiver.try_recv() {
                    Ok(ProcessEvent::Progress { fraction, message }) => {
                        self.progress = fraction;
                        self.progress_message = message;
                    }
                    Ok(ProcessEvent::Finished(result)) => {
                        finished = Some(result);
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        ctx.request_repaint_after(std::time::Duration::from_millis(50));
                        break;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        finished = Some(Err(
                            "生成処理が予期せず終了しました。入力動画は変更していません。".into(),
                        ));
                        break;
                    }
                }
            }
        }
        if let Some(result) = finished {
            self.progress = 1.0;
            self.progress_message = if result.is_ok() { "完了" } else { "失敗" }.into();
            self.result = Some(result);
            self.process = None;
        }
    }

    fn begin_generation(&mut self) {
        let Some(media) = self.media.clone() else {
            self.notice = Some("先に動画を選択してください。".into());
            return;
        };
        if let Err(error) = self.config.settings.validate(media.duration_seconds) {
            self.notice = Some(error);
            return;
        }
        if let Err(error) = self.config.save() {
            self.notice = Some(format!("設定を保存できません: {error}"));
        }
        self.progress = 0.0;
        self.progress_message = "準備中…".into();
        self.result = None;
        self.process = Some(processor::start(ProcessRequest {
            media,
            settings: self.config.settings.clone(),
        }));
    }

    fn output_directory(&self) -> Option<PathBuf> {
        if self.config.settings.use_input_directory {
            self.media.as_ref()?.path.parent().map(Path::to_owned)
        } else if self.config.settings.output_directory.as_os_str().is_empty() {
            None
        } else {
            Some(self.config.settings.output_directory.clone())
        }
    }

    fn input_panel(&mut self, ui: &mut egui::Ui, enabled: bool) {
        ui.heading("1. 入力動画");
        ui.horizontal(|ui| {
            if ui
                .add_enabled(enabled, egui::Button::new("動画を選択…"))
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter(
                        "動画",
                        &[
                            "mp4", "mkv", "mov", "avi", "webm", "m4v", "mpg", "mpeg", "ts",
                        ],
                    )
                    .pick_file()
            {
                self.select_video(path);
            }
            if ui
                .add_enabled(
                    enabled && self.selected_path.is_some(),
                    egui::Button::new("解除"),
                )
                .clicked()
            {
                self.selected_path = None;
                self.media = None;
                self.probing = None;
                self.result = None;
            }
            ui.label("（ファイルを画面へドロップすることもできます）");
        });
        if self.probing.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("動画情報を取得中…");
            });
        } else if let Some(info) = &self.media {
            egui::Grid::new("media_info")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    ui.label("ファイル");
                    ui.label(&info.file_name);
                    ui.end_row();
                    ui.label("場所");
                    ui.label(info.path.display().to_string());
                    ui.end_row();
                    ui.label("長さ");
                    ui.label(info.duration_label());
                    ui.end_row();
                    ui.label("映像サイズ");
                    ui.label(format!("{} × {}", info.width, info.height));
                    ui.end_row();
                    ui.label("容量");
                    ui.label(format_file_size(info.file_size));
                    ui.end_row();
                    ui.label("映像 / 音声");
                    ui.label(format!(
                        "{} / {}",
                        info.video_codec,
                        info.audio_codec.as_deref().unwrap_or("なし")
                    ));
                    ui.end_row();
                    ui.label("フレームレート");
                    ui.label(
                        info.frame_rate
                            .map(|v| format!("{v:.3} fps"))
                            .unwrap_or_else(|| "不明".into()),
                    );
                    ui.end_row();
                });
        } else {
            ui.label(
                RichText::new("動画が選択されていません")
                    .italics()
                    .color(Color32::GRAY),
            );
        }
    }

    fn settings_panel(&mut self, ui: &mut egui::Ui, enabled: bool) {
        let settings = &mut self.config.settings;
        ui.add_enabled_ui(enabled, |ui| {
            ui.heading("2. 抽出とレイアウト");
            egui::Grid::new("basic_settings")
                .num_columns(2)
                .spacing([24.0, 8.0])
                .show(ui, |ui| {
                    ui.label("抽出方法");
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut settings.capture_mode,
                            CaptureMode::EvenlySpaced,
                            "均等",
                        );
                        ui.selectable_value(
                            &mut settings.capture_mode,
                            CaptureMode::FixedInterval,
                            "一定間隔",
                        );
                    });
                    ui.end_row();
                    ui.label("最大枚数");
                    ui.add(
                        egui::DragValue::new(&mut settings.thumbnail_count)
                            .range(1..=100)
                            .suffix(" 枚"),
                    );
                    ui.end_row();
                    if settings.capture_mode == CaptureMode::FixedInterval {
                        ui.label("抽出間隔");
                        ui.add(
                            egui::DragValue::new(&mut settings.interval_seconds)
                                .range(0.1..=86400.0)
                                .speed(1.0)
                                .suffix(" 秒"),
                        );
                        ui.end_row();
                    }
                    ui.label("列数");
                    ui.add(
                        egui::DragValue::new(&mut settings.columns)
                            .range(1..=20)
                            .suffix(" 列"),
                    );
                    ui.end_row();
                    ui.label("サムネイル幅");
                    ui.add(
                        egui::DragValue::new(&mut settings.thumbnail_width)
                            .range(80..=1920)
                            .suffix(" px"),
                    );
                    ui.end_row();
                    ui.label("表示情報");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut settings.show_timestamp, "タイムスタンプ");
                        ui.checkbox(&mut settings.show_header, "ヘッダー");
                    });
                    ui.end_row();
                });

            egui::CollapsingHeader::new("詳細設定").show(ui, |ui| {
                egui::Grid::new("advanced_settings")
                    .num_columns(2)
                    .spacing([24.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("先頭 / 末尾を除外");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut settings.trim_start_seconds)
                                    .range(0.0..=86400.0)
                                    .suffix(" 秒"),
                            );
                            ui.label("/");
                            ui.add(
                                egui::DragValue::new(&mut settings.trim_end_seconds)
                                    .range(0.0..=86400.0)
                                    .suffix(" 秒"),
                            );
                        });
                        ui.end_row();
                        ui.label("画像間 / 外側の余白");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut settings.gap)
                                    .range(0..=100)
                                    .suffix(" px"),
                            );
                            ui.label("/");
                            ui.add(
                                egui::DragValue::new(&mut settings.outer_margin)
                                    .range(0..=200)
                                    .suffix(" px"),
                            );
                        });
                        ui.end_row();
                        ui.label("背景色");
                        egui::color_picker::color_edit_button_srgb(ui, &mut settings.background);
                        ui.end_row();
                        ui.label("枠線");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut settings.border_width)
                                    .range(0..=20)
                                    .suffix(" px"),
                            );
                            egui::color_picker::color_edit_button_srgb(
                                ui,
                                &mut settings.border_color,
                            );
                        });
                        ui.end_row();
                        ui.label("ヘッダータイトル");
                        ui.text_edit_singleline(&mut settings.custom_title);
                        ui.end_row();
                        ui.label("文字");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut settings.font_size)
                                    .range(10.0..=72.0)
                                    .suffix(" px"),
                            );
                            egui::color_picker::color_edit_button_srgb(
                                ui,
                                &mut settings.text_color,
                            );
                        });
                        ui.end_row();
                    });
            });

            ui.add_space(8.0);
            preview(
                ui,
                settings.columns,
                settings.thumbnail_count,
                settings.background,
                settings.gap,
            );
        });
    }

    fn output_panel(&mut self, ui: &mut egui::Ui, enabled: bool) {
        let settings = &mut self.config.settings;
        ui.add_enabled_ui(enabled, |ui| {
            ui.heading("3. 出力");
            egui::Grid::new("output_settings")
                .num_columns(2)
                .spacing([24.0, 8.0])
                .show(ui, |ui| {
                    ui.label("画像形式");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut settings.image_format, ImageFormat::Jpeg, "JPEG");
                        ui.selectable_value(&mut settings.image_format, ImageFormat::Png, "PNG");
                        if settings.image_format == ImageFormat::Jpeg {
                            ui.label("画質");
                            ui.add(egui::DragValue::new(&mut settings.jpeg_quality).range(1..=100));
                        }
                    });
                    ui.end_row();
                    ui.label("個別画像");
                    ui.checkbox(
                        &mut settings.output_individual,
                        "コンタクトシートと一緒に保存",
                    );
                    ui.end_row();
                    ui.label("重複時");
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut settings.conflict_policy,
                            ConflictPolicy::Rename,
                            "別名",
                        );
                        ui.selectable_value(
                            &mut settings.conflict_policy,
                            ConflictPolicy::Overwrite,
                            "上書き",
                        );
                        ui.selectable_value(
                            &mut settings.conflict_policy,
                            ConflictPolicy::Skip,
                            "スキップ",
                        );
                    });
                    ui.end_row();
                    ui.label("ファイル名");
                    ui.horizontal(|ui| {
                        ui.label("接頭辞");
                        ui.text_edit_singleline(&mut settings.filename_prefix);
                        ui.label("接尾辞");
                        ui.text_edit_singleline(&mut settings.filename_suffix);
                    });
                    ui.end_row();
                    ui.label("出力先");
                    ui.vertical(|ui| {
                        ui.radio_value(
                            &mut settings.use_input_directory,
                            true,
                            "入力動画と同じフォルダー",
                        );
                        ui.horizontal(|ui| {
                            ui.radio_value(
                                &mut settings.use_input_directory,
                                false,
                                "指定フォルダー",
                            );
                            if ui.button("選択…").clicked()
                                && let Some(path) = rfd::FileDialog::new().pick_folder()
                            {
                                settings.output_directory = path;
                                settings.use_input_directory = false;
                            }
                        });
                        if !settings.use_input_directory {
                            ui.label(settings.output_directory.display().to_string());
                        }
                    });
                    ui.end_row();
                });
        });
        if let Some(path) = self.output_directory() {
            ui.label(format!("保存先: {}", path.display()));
        }
    }

    fn preset_panel(&mut self, ui: &mut egui::Ui, enabled: bool) {
        ui.add_enabled_ui(enabled, |ui| {
            egui::CollapsingHeader::new("設定とプリセット").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("プリセット");
                    egui::ComboBox::from_id_salt("preset_combo")
                        .selected_text(if self.selected_preset.is_empty() {
                            "選択…"
                        } else {
                            &self.selected_preset
                        })
                        .show_ui(ui, |ui| {
                            for name in self.config.presets.keys() {
                                ui.selectable_value(&mut self.selected_preset, name.clone(), name);
                            }
                        });
                    if ui.button("読込").clicked()
                        && let Some(settings) =
                            self.config.presets.get(&self.selected_preset).cloned()
                    {
                        self.config.settings = settings;
                    }
                    if ui.button("削除").clicked() && !self.selected_preset.is_empty() {
                        self.config.presets.remove(&self.selected_preset);
                        self.selected_preset.clear();
                        let _ = self.config.save();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("名前");
                    ui.text_edit_singleline(&mut self.preset_name);
                    if ui.button("現在の設定を保存").clicked()
                        && !self.preset_name.trim().is_empty()
                    {
                        let name = self.preset_name.trim().to_owned();
                        self.config
                            .presets
                            .insert(name.clone(), self.config.settings.clone());
                        self.selected_preset = name;
                        self.preset_name.clear();
                        if let Err(error) = self.config.save() {
                            self.notice = Some(format!("プリセットを保存できません: {error}"));
                        }
                    }
                    if ui.button("初期値に戻す").clicked() {
                        self.config.settings = Default::default();
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("設定をエクスポート…").clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .set_file_name("thumbnailer-settings.json")
                            .save_file()
                        && let Err(error) = self.config.export_to(&path)
                    {
                        self.notice = Some(format!("設定をエクスポートできません: {error}"));
                    }
                    if ui.button("設定をインポート…").clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .add_filter("JSON", &["json"])
                            .pick_file()
                    {
                        match StoredConfig::import_from(&path) {
                            Ok(config) => self.config = config,
                            Err(error) => self.notice = Some(error),
                        }
                    }
                });
            });
        });
    }

    fn process_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("4. 生成");
        let processing = self.process.is_some();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !processing && self.media.is_some(),
                    egui::Button::new(RichText::new("生成開始").strong()),
                )
                .clicked()
            {
                if self.config.settings.conflict_policy == ConflictPolicy::Overwrite {
                    self.confirm_overwrite = true;
                } else {
                    self.begin_generation();
                }
            }
            if ui
                .add_enabled(processing, egui::Button::new("キャンセル"))
                .clicked()
                && let Some(handle) = &self.process
            {
                handle.cancel();
                self.progress_message = "キャンセル中…".into();
            }
        });
        if processing || self.progress > 0.0 {
            ui.add(
                egui::ProgressBar::new(self.progress)
                    .show_percentage()
                    .text(&self.progress_message),
            );
        }
        if let Some(result) = &self.result {
            match result {
                Ok(summary) => {
                    ui.label(
                        RichText::new(format!(
                            "完了: {}ファイルを保存しました（{:.1}秒）",
                            summary.files.len(),
                            summary.elapsed.as_secs_f32()
                        ))
                        .color(Color32::from_rgb(60, 170, 90))
                        .strong(),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("出力フォルダーを開く").clicked()
                            && let Err(error) = open_folder(&summary.output_directory)
                        {
                            self.notice = Some(error);
                        }
                        ui.label(summary.output_directory.display().to_string());
                    });
                    for warning in &summary.warnings {
                        ui.label(RichText::new(format!("警告: {warning}")).color(Color32::YELLOW));
                    }
                }
                Err(error) => {
                    ui.label(RichText::new(format!(
                        "処理できませんでした: {error}\n入力動画への変更・移動・削除は行っていません。"
                    )).color(Color32::from_rgb(220, 80, 80)));
                }
            }
        }
    }
}

impl eframe::App for ThumbnailApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_background_work(ctx);
        if self.process.is_none() {
            let dropped = ctx.input(|input| input.raw.dropped_files.clone());
            if let Some(path) = dropped.into_iter().find_map(|file| file.path) {
                self.select_video(path);
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("動画コンタクトシート");
            ui.label("動画全体を見渡せるサムネイル画像を作成します。");
            ui.separator();
            let enabled = self.process.is_none();
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.input_panel(ui, enabled);
                ui.separator();
                self.settings_panel(ui, enabled);
                ui.separator();
                self.output_panel(ui, enabled);
                self.preset_panel(ui, enabled);
                ui.separator();
                self.process_panel(ui);
                ui.add_space(16.0);
            });
        });

        if self.confirm_overwrite {
            egui::Window::new("上書きの確認")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("同名の出力ファイルがある場合は上書きします。続行しますか？");
                    if let Some(path) = self.output_directory() {
                        ui.label(path.display().to_string());
                    }
                    ui.horizontal(|ui| {
                        if ui.button("上書きして生成").clicked() {
                            self.confirm_overwrite = false;
                            self.begin_generation();
                        }
                        if ui.button("戻る").clicked() {
                            self.confirm_overwrite = false;
                        }
                    });
                });
        }

        if let Some(message) = self.notice.clone() {
            egui::Window::new("お知らせ")
                .collapsible(false)
                .resizable(true)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(message);
                    if ui.button("閉じる").clicked() {
                        self.notice = None;
                    }
                });
        }
    }
}

impl Drop for ThumbnailApp {
    fn drop(&mut self) {
        if let Some(handle) = &self.process {
            handle.cancel();
        }
        let _ = self.config.save();
    }
}

fn preview(ui: &mut egui::Ui, columns: u32, count: u32, background: [u8; 3], gap: u32) {
    ui.label("レイアウトプレビュー");
    let desired = egui::vec2(ui.available_width().min(520.0), 120.0);
    let (response, painter) = ui.allocate_painter(desired, egui::Sense::hover());
    let rect = response.rect;
    painter.rect_filled(
        rect,
        4.0,
        Color32::from_rgb(background[0], background[1], background[2]),
    );
    let shown_count = count.clamp(1, 30);
    let shown_columns = columns.min(shown_count).max(1);
    let rows = shown_count.div_ceil(shown_columns);
    let visual_gap = (gap as f32 / 4.0).clamp(2.0, 10.0);
    let margin = 8.0;
    let cell_w = (rect.width() - margin * 2.0 - visual_gap * (shown_columns - 1) as f32)
        / shown_columns as f32;
    let cell_h = (rect.height() - margin * 2.0 - visual_gap * (rows - 1) as f32) / rows as f32;
    for index in 0..shown_count {
        let column = index % shown_columns;
        let row = index / shown_columns;
        let min = egui::pos2(
            rect.left() + margin + column as f32 * (cell_w + visual_gap),
            rect.top() + margin + row as f32 * (cell_h + visual_gap),
        );
        let cell = egui::Rect::from_min_size(min, egui::vec2(cell_w.max(1.0), cell_h.max(1.0)));
        painter.rect_filled(cell, 1.0, Color32::from_rgb(85, 105, 130));
    }
}

fn install_japanese_ui_font(ctx: &egui::Context) {
    #[cfg(target_os = "windows")]
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
    ];
    #[cfg(not(target_os = "windows"))]
    const CANDIDATES: &[&str] = &[
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    if let Some(bytes) = CANDIDATES.iter().find_map(|path| fs::read(path).ok()) {
        let mut fonts = FontDefinitions::default();
        fonts
            .font_data
            .insert("system".into(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "system".into());
        ctx.set_fonts(fonts);
    }
}

fn format_file_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn open_folder(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let status = Command::new("explorer").arg(path).status();
    #[cfg(target_os = "linux")]
    let status = Command::new("xdg-open").arg(path).status();
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(path).status();
    status
        .map_err(|e| format!("出力フォルダーを開けません: {e}"))
        .and_then(|status| {
            status
                .success()
                .then_some(())
                .ok_or_else(|| "出力フォルダーを開けません。".into())
        })
}
