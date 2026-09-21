# screen-thumbnailer

動画から複数の静止画を抽出し、格子状に並べたコンタクトシートを生成するWindows/Linux向けGUIアプリです。動画を再生せずに内容を一覧できる画像を作成します。

主な機能:

- 動画全体からの均等抽出、または一定時間間隔での抽出
- 列数、サムネイル幅、余白、背景色、枠線の設定
- タイムスタンプと動画情報ヘッダーの表示
- JPEG/PNGのコンタクトシートと個別画像の出力
- 設定プリセット、進捗表示、処理のキャンセル
- WindowsおよびLinux、日本語を含むファイル名・パスへの対応

## 必要なもの

- Rust 1.85以降
- `ffmpeg` と `ffprobe`（両方を `PATH` から実行できること）
- 日本語を画像へ描画する場合は Noto Sans CJK、Yu Gothic、Meiryo のいずれか

FFmpegは動画の読み取りとフレーム抽出に使用します。アプリは入力動画を書き換えません。

GitHub Actionsで生成するWindows x86_64版はMSVC Cランタイムを静的リンクしているため、`VCRUNTIME140.dll`の事前インストールは不要です。FFmpegとffprobeは引き続き別途必要です。

## 実行

```sh
cargo run --release
```

初回ビルド時はRustクレートの取得が必要です。GUIから動画を1件選び、抽出条件、レイアウト、保存先を指定して「生成開始」を押してください。設定はOS標準のユーザー設定フォルダーへ保存されます。

## テスト

```sh
cargo test --all-targets
```

## ライセンス

このプロジェクト自身のコードは[MIT License](LICENSE)で提供します。Rust依存クレートの著作権表示とライセンス本文は[THIRD_PARTY_LICENSES.html](THIRD_PARTY_LICENSES.html)、実行ファイルへ埋め込まれるフォントの原文は[licenses/fonts](licenses/fonts/README.md)を参照してください。

推移依存の`option-ext 0.2.0`はMPL-2.0で提供されています。対応するソースコードは次の場所から入手できます。

- crates.io（ソースアーカイブ）: https://crates.io/api/v1/crates/option-ext/0.2.0/download
- upstream repository（公開時のコミット）: https://github.com/soc/option-ext/tree/272f22fc9ea1ac6b08f01704af52c4ac338df4e2

`option-ext`に変更を加えて配布する場合、その変更済みファイルにもMPL-2.0が適用されます。本プロジェクトは同クレートを変更せずに利用しています。

FFmpegとffprobeは本リポジトリや配布成果物には同梱していません。利用者が別途導入した実行ファイルを外部プロセスとして呼び出します。

第三者ライセンス一覧は`cargo-about 0.9.2`とリポジトリ内の`about.toml`、`about.hbs`から再生成できます。

```sh
cargo install cargo-about --version 0.9.2 --locked --features cli
cargo about generate about.hbs --frozen --fail --output-file THIRD_PARTY_LICENSES.html
```
