# Mac版（Apple silicon）

Windows版と同じアプリを、Apple silicon（M1〜）のMacで動かすための版です。翻訳・音声認識・文字認識は、Windows版と同じく、すべてこのMacの中で動きます。

## 動作環境

- macOS 13（Ventura）以降、Apple silicon（M1 / M2 / M3 / M4 …）
- Intel Macには対応していません（翻訳・音声認識の公式の実行環境がないため）

## はじめかた

1. [Mac版の最新リリース](https://github.com/kubot64/talk-style-live-translation/releases/tag/mac-latest)から `talk-style-live-translation-<版>-macos-arm64.zip` をダウンロードして開き、`Talk風Live翻訳.app` を「アプリケーション」フォルダへ移します。
   - Releases にまだZIPがないときは、[GitHub Actions の macOS の実行結果](https://github.com/kubot64/talk-style-live-translation/actions/workflows/macos.yml)の最新の成功した回から、「talk-style-live-translation-macos-arm64」をダウンロードできます（GitHub へのログインが必要です）。
2. 最初の1回だけ、Finderでアプリを **右クリック →「開く」** を選び、確認の画面で「開く」を押します。
   - Apple の開発者署名がない（アドホック署名の）アプリのため、ダブルクリックだけでは「開発元を確認できない」と出て開けません。
   - それでも開けないときは、ターミナルで `xattr -dr com.apple.quarantine "/Applications/Talk風Live翻訳.app"` を実行してから開きます。
3. 初回の画面で翻訳モデルの大きさを選び、「はじめる」を押します。Windows版と同じものを、Mac用の実行環境でダウンロードします。
4. 「● 聞く」を初めて押すと、**画面収録とシステムオーディオ録音**の許可を求められます。「システム設定」→「プライバシーとセキュリティ」→「画面収録とシステムオーディオ録音」で「Talk風Live翻訳」をオンにして、アプリを開き直してください。
   - PCで再生中の音声を取り出すのに、macOSの ScreenCaptureKit を使うためです。マイクは使いません。画面の映像は保存も送信もしません。

## Windows版との違い

| 項目 | Windows版 | Mac版 |
| --- | --- | --- |
| 翻訳（llama.cpp b12119） | CUDA / Vulkan / CPU | **Metal**（GPU）/ CPU |
| PC音声の取得 | WASAPI ループバック | **ScreenCaptureKit**（このアプリ自身の音は自動で除外） |
| 英語の音声認識（Nemotron Speech 0.3.0） | Vulkan / CPU | **Metal** / CPU |
| 英語の文章区切り補助（sherpa-onnx 2.0.1） | ○ | ○ |
| 画面・画像の文字認識 | Windows OCR | **Apple Vision**（追加ダウンロードなし） |
| PaddleOCR-VL | ○ | ○（Metal） |
| 読み上げ（speech.cpp 0.10.1） | ○ | ○（Metal。再生は `afplay`） |
| Qwen3-ASR / Whisper（多言語の音声認識） | ○ | ×（準備スクリプトがPowerShellのため。今後の対応） |
| Indra OCR | ○ | ×（同上） |
| データの保存先 | 展開したフォルダの `data` | `~/Library/Application Support/Talk風Live翻訳` |

- 設定画面の「GPU」は、Macでは Metal を指します（設定ファイルの値は Windows版と同じ `vulkan` のままです）。
- GPUメモリの欄は、Metal が使ってよいとする量（ユニファイドメモリの一部）と、いま空いているメモリから見積もった「目安」です。
- 範囲翻訳で画面を取り込むとき、Windows版のようにアプリの窓を一時的に隠すことはしません。

## 自分でビルドする

Xcode のコマンドラインツールと Rust（1.88 以降）を入れたMacで:

```sh
rustup target add aarch64-apple-darwin
scripts/package-macos.sh 1.0.0
```

`dist/Talk風Live翻訳.app` と `dist/talk-style-live-translation-1.0.0-macos-arm64.zip` ができます。`.cargo/config.toml` の既定のターゲットは Windows なので、`cargo` を直接使うときは `--target aarch64-apple-darwin` を付けてください（例：`cargo run --target aarch64-apple-darwin`）。

GitHub Actions（`.github/workflows/macos.yml`）でも同じZIPを作ります。`main` への push のたびに `mac-latest` のプレリリースを作り直して最新のZIPを置き、`v*` のタグを付けたときは、そのタグのリリースにもZIPを添付します。

## 固定している取得先（Mac用）

| もの | ファイル | SHA-256 |
| --- | --- | --- |
| llama.cpp b12119 | `llama-b12119-bin-macos-arm64.tar.gz` | `51fc160bc6f6a6b53fef3c50d12b27314cfaf8fc37766fc605e0bd30d87a79b8` |
| NeMo-Speech.cpp 0.3.0（Metal） | `nemo-speech-0.3.0-macos-aarch64-metal.tar.gz` | `2b972ed8b5dba07492212acf8cb75cbf34ecb141198ffa1997febe84b7c4d9ed` |
| NeMo-Speech.cpp 0.3.0（CPU） | `nemo-speech-0.3.0-macos-aarch64-cpu.tar.gz` | `2c3a7a719b778daeb14f147e011891f16e4b2ac62388d8b8d8b365cc2eebdf15` |
| sherpa-onnx 2.0.1 | `sherpa-onnx-v2.0.1-osx-arm64-shared-no-tts.tar.bz2` | `4fcc6c77c8d8d2e2f04cfdc17c6fd224c01469b78529370eb9d8e0e5ec97ac79` |
| speech.cpp 0.10.1（Metal） | `speech-0.10.1-macos-arm64-metal.zip` | `ff24f6f58c9d2dedb01caa88ba043ae845c76eff228e4cbfae5de749174404c1` |

大きさとURLは `src/installer.rs` と `scripts/*-macos.json` にあります。モデル（PLaMo・Nemotron・句読点・Irodori-TTS）は Windows版と同じファイルです。アーカイブ内のシンボリックリンク（`libggml.dylib` など）は、リンクとしてではなく、リンク先と同じ内容のファイルとして展開します。

## 確かめたこと・まだのこと

- Linux 上で macOS（aarch64-apple-darwin）向けに `cargo check` / `cargo clippy` が通り、Windows 向けのビルドも変わらず通ることを確かめました。tar.gz の展開（上位フォルダの除去、リンクのコピー、実行権限、外を指すリンクの拒否）は検査を足しました。
- 実際のMacでの動作（音声の取得、Metal での翻訳・音声認識）は、GitHub Actions の macOS ランナーでのビルドまでで、実機ではまだ確かめていません。
