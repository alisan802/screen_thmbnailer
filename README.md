# screen-thumbnailer

動画から複数の静止画を抽出し、コンタクトシートを生成するWindows/Linux向けGUIアプリです。

## 必要なもの

- Rust 1.85以降
- `ffmpeg` と `ffprobe`（両方を `PATH` から実行できること）
- 日本語を画像へ描画する場合は Noto Sans CJK、Yu Gothic、Meiryo のいずれか

FFmpegは動画の読み取りとフレーム抽出に使用します。アプリは入力動画を書き換えません。

## 実行

```sh
cargo run --release
```

初回ビルド時はRustクレートの取得が必要です。GUIから動画を1件選び、抽出条件、レイアウト、保存先を指定して「生成開始」を押してください。設定はOS標準のユーザー設定フォルダーへ保存されます。

## テスト

```sh
cargo test --all-targets
```
