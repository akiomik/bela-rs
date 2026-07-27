# Rust on Bela Gem: バインディング整備の実現性調査 兼 別リポジトリ立ち上げハンドオフ

調査日: 2026-07-27 / 前提: ADR 0009 で Bela Gem Stereo Starter Kit を購入済み

> **このドキュメントの位置づけ**
> Bela の Rust バインディング (`bela-sys` / `bela-rs`) 開発は oxtt 本体のスコープを
> 広げすぎるため、**oxtt とは独立した専用リポジトリ**で行う。本書はその新リポジトリを
> 立ち上げ、後任の開発者がゼロから着手できるようにするためのハンドオフ資料。
> oxtt 側は将来、完成したクレートを**依存として取り込むだけ**（下記「oxtt 側との境界」を参照）。

## 結論

**実現可能。難易度は「中」。** ADR 0009 が予告したとおり「既製ラッパーではなく
bindgen による自前バインディング」が正解。

- 提案の 3 クレート構成（bela-bindgen / bela-sys / bela-rs）ではなく
  **2 クレート（`bela-sys` + `bela-rs`）**を推奨。bindgen は独立クレートではなく
  `bela-sys` の `build.rs` 内の一手段にすぎない。
- 最大の論点: **旧 `bela-rs` / `bela-sys` / `xc-bela-cmake` は全て 32-bit ARM
  (`armv7-unknown-linux-gnueabihf`) 前提で、そのままでは Gem に使えない。**
  Gem は PocketBeagle 2 (TI AM6232/AM6254) で **aarch64
  (`aarch64-unknown-linux-gnu`)**、かつ新しいカーネル/Xenomai 世代。
  「古いクレートを fork して直す」より「設計を参考に aarch64 向けに組み直す」のが実態。

## 新リポジトリの構成

- **リポジトリ**: 例 `akiomik/bela-rs`（1 リポジトリに 2 クレートを置く Cargo workspace）。
  `*-sys` + safe ラッパーのペアを 1 リポジトリにまとめる Rust の慣例に従う。
  クレート名 `bela` / `bela-sys` / `bela-rs` は **crates.io に未登録**（2026-07-27 に
  API で確認、いずれも 404）。andrewcsmith / padenot のものは GitHub のみで
  crates.io には公開されていないため、名前はそのまま使える。
- **workspace レイアウト（想定）**:
  ```
  bela-rs/
    Cargo.toml            # [workspace] members = ["bela-sys", "bela-rs"]
    bela-sys/             # bindgen による生 FFI (-sys クレート)
      build.rs
      src/lib.rs
      wrapper.h           # bindgen 入力: Bela.h 等を include
    bela-rs/              # safe ラッパー
      src/lib.rs
    examples/             # 単体で動く最小の render 例（sine/passthrough）
    docs/
      cross-compile.md    # ツールチェーン・sysroot 手順
      board-facts.md      # フェーズ 0 で採取した実測値の記録
  ```
- **ライセンス**: Bela コアソフトは **LGPL 3.0**（リポジトリ README に明記、確認済み）。
  バインディングクレート自体は oxtt に合わせ MIT/Apache-2.0 デュアルで問題ない。
  LGPL の義務は libbela をリンクした**最終バイナリ**側に掛かる：動的リンクなら
  そのまま配布可、静的リンクなら再リンク可能なオブジェクト提供が必要
  （個人利用・OSS 配布なら実質問題にならない）。

## 実現性を左右する技術事実

### 1. コア API は C-ABI（bindgen に好都合）
`setup` / `render` / `cleanup` は `extern "C"` 宣言、引数は `BelaContext*`
(POD 構造体) + `void* userData`。純粋な C 表面なので bindgen がきれいに効く。
`bela-sys` は本質的に bindgen 作業、という ADR の見立ては正しい。

### 2. 上位ライブラリ（Scope / Trill / Fft / Gui / Midi）は C++ クラス → bindgen 不可
直接バインドできず、`cxx` か手書き C シムが必要。バインディングの最小スコープは
**コア API のみ**とし、上位ライブラリは「必要になった時点で増分」の非目標に置く。
（oxtt の現行 DSP は IIR のみ = ADR 0009 finding 3 なのでコア API で足りる。
将来 phase vocoder をやる場合のみ NE10 ベースの `Fft` 用 C++ シム層が要る。）

### 3. 統合モデルは「libbela をリンクした単体バイナリ」
Bela は "Using the Bela core with other programs" として、任意プログラムが
render/setup/cleanup を定義し `libbela` をリンク → `Bela_initAudio` /
`Bela_startAudio` で起動する方式を公式サポート。bela-rs も glicol-bela も
`.so` プラグインではなく単体 Rust バイナリを作る方式。
「PC でクロスビルド → ボードに転送」ワークフローに合致。

### 4. リアルタイム安全性が設計上の核心
（electric-snow の FFI 設計記事が既に整理済み）
- render スレッドでは alloc / syscall / IO 禁止。
- **FFI 境界を越える unwind は UB。** `catch_unwind` は箱化にヒープを使うため
  render 経路で使えない → padenot の解は `BelaApplication` を `unsafe trait` にして
  RT 安全性を実装者責任に押し付ける。
- バイナリを **`panic = "abort"`** でビルドすれば unwind 問題は根本回避できる。
- `rt_printf` と CLI パースは未ラップのまま残された（ADR の「rt_printf 欠落」の出所）。

### 5. Xenomai 世代の差
- 旧 Bela = Xenomai 3 + I-pipe (4.x カーネル)。
- Gem = 新カーネル (PB2 は v6.12-ti-arm64 系) なので **Xenomai 3 + Dovetail**
  （ユーザ空間は依然 `libcobalt`）と見られる。
- Rust から見れば「`libbela` + Xenomai ラッパ lib をリンクする」だけで、
  具体的な `-l` フラグはボード上で Bela の Makefile を verbose ビルドすれば判明する
  （低リスク）。

### 6. Gem 固有の新 API: コアごとの render()
quad-core PB2 向けに「CPU コアごとに `render()` を呼べる」拡張が入っている。
旧 bela-rs には無い設計要素で、safe ラッパー側で考慮が要る。

### 7. Gem での API 差分は公式に文書化済み（Migrating to Bela Gem）
「ほぼソース互換だが完全ではない」。safe ラッパー設計に直結する差分:
- **analog out は audio out に統合**: `analogWrite` 系のコードはフレーム数が
  `audioFrames` 基準になり、チャンネル番号が +2 オフセット。自動保持も廃止。
- **`uniformSampleRate` がデフォルト有効**: audio/analog のフレーム比が旧来の
  0.5 前提から変わる。
- **マルチコア化でスレッド間通信の安全性要件が上がる**（atomic / lock-free queue 前提）。
- OS は Debian 12 Bookworm（旧 Bela は Debian 9）。
→ ラッパーは旧 Bela との互換を狙わず **Gem のセマンティクスを正**として設計してよい
（このリポジトリのターゲットは Gem のみ）。

## 作業計画（新リポジトリでのフェーズ）

> **注意（2026-07-27 更新）**: Bela Gem は**未着**。フェーズ番号は依存関係の順序を
> 表すが、着手順は「ボード到着前 / 到着後」で分ける（下記「ボード到着前に
> 進められる作業」参照）。フェーズ 0 はボード到着後の最初のタスク。

### ボード到着前に進められる作業（今やる）
ボードが必要なのは「リンク〜実行」だけ。bindgen とラッパー設計はヘッダと
ホストのテストで完結する。
- **リポジトリ雛形**: git init、workspace 構成、MIT/Apache-2.0 デュアル、
  CI（ホストで `cargo check` / `clippy` / `test`）。
- **フェーズ 1 の前半**: `rustup target add aarch64-unknown-linux-gnu`、
  aarch64 GCC ツールチェーン導入、`.cargo/config.toml` の雛形。
  sysroot 取得だけがボード待ち。
- **フェーズ 2 の大半**: BelaPlatform/Bela（master）を submodule か build.rs で
  取得し、`Bela.h` に bindgen を実行。**`cargo check --target
  aarch64-unknown-linux-gnu` はリンクしないので sysroot なしで通せる**。
  リンクフラグの emit だけフェーズ 0 の結果待ちにする
  （ヘッダの取得元は env var で差し替え可能にしておき、到着後にボード実機の
  Bela バージョンへピン留めし直す）。
- **フェーズ 3 の設計と大半の実装**: unsafe RT トレイト、トランポリン、
  builder、RAII は生成済みの `BelaContext` 型があれば書ける。テストでは
  `BelaContext` をホスト上で手組みして render を直接呼び、**ハードなしで
  ユニットテスト**する。examples も `cargo check` までは通せる。
- ドキュメント: `docs/cross-compile.md` のドラフト、RT 安全性ルールの明文化。

### ボード到着後にやる作業
フェーズ 0 → sysroot 取得（フェーズ 1 残り）→ リンクフラグ emit
（フェーズ 2 残り）→ 実機で examples 実行（フェーズ 3 の受け入れ基準）。
到着前の作業が済んでいれば、到着後は「事実の採取と接続」だけになる。

### フェーズ 0: ボード上で事実確定（到着後最優先・数時間）
未知をここで潰す。成果物は `docs/board-facts.md` に記録する。
1. `uname -m` で aarch64 確認、Xenomai バージョン確認
2. C++ サンプルを verbose ビルド（`make` の実行コマンド）して
   **正確な include パス・`-l` フラグ・`libbela` の場所**を採取
3. Bela IDE のデフォルトプログラムを止める運用（`systemctl stop bela` 等）を確認
- **完了条件**: リンクに必要な `-I` / `-l` / ライブラリ探索パスが全て文書化されている。

### フェーズ 1: クロスビルド環境（`docs/cross-compile.md`）
4. aarch64 sysroot 取得（ボードの `/` を rsync、Bela の `SyncBelaSysroot` 相当）
   → `BELA_SYSROOT` 設定
5. `aarch64-linux-gnu` GCC ツールチェーン + `rustup target add
   aarch64-unknown-linux-gnu` + `.cargo/config.toml`
   （linker と `BINDGEN_EXTRA_CLANG_ARGS` で sysroot 指定）
- **完了条件**: 空の Rust バイナリが aarch64 向けにクロスビルド→ボードで起動できる。

### フェーズ 2: bela-sys
6. `build.rs` で `wrapper.h`（`Bela.h` + 必要なコアヘッダ）に bindgen、
   C API のみ allowlist、フェーズ 0 で採取したリンクフラグを emit
- **完了条件**: `BelaContext` / `setup` / `render` / `cleanup` / `Bela_initAudio`
  等の FFI シンボルが生成され、リンクが通る。

### フェーズ 3: bela-rs（safe ラッパー）
7. builder パターン + userData ステートマシン + トランポリン関数 +
   `unsafe` RT トレイト + `Bela_startAudio` / `stopAudio` / `cleanupAudio` の RAII。
   Gem のコア別 render も設計に織り込む
- **完了条件**: `examples/` の passthrough / sine が safe API だけで書け、
  実機で音が出る（受け入れ基準）。

### 非目標（この新リポジトリではやらない）
- C++ 上位ライブラリ（Scope / Trill / Gui / Fft / Midi）の完全ラップ。必要時に増分。
- `rt_printf` の型安全マクロ化。
- oxtt の DSP 統合（下記の通り oxtt 側の作業）。

## oxtt 側との境界（このリポジトリの外）

新リポジトリの成果物が揃った後、oxtt 側で別途行う作業（本書のスコープ外・参考）:
- 新バックエンド（host adapter）として `bela-rs` を git 依存で追加し、
  `src/dsp/` を呼ぶ。full std なので DSP は verbatim コンパイル（ADR 0009 finding 3）。
- コントロールサーフェスは render 内で `analogRead`（ADR 0009 finding 4、別スレッド不要）。
- この統合は ADR 0009 の platform 決定 ADR / 後続 ADR で正式に扱う。

## リスク・要確認事項

- **C++ 上位ライブラリ**: 将来の phase vocoder で `Fft`(NE10) が要る場合のみ
  cxx シムが必要。コア API のバインディングには不要。
- **panic 安全性**: `panic = "abort"` で回避可能だが、正しさに直結するので明示設計を。
- **Bela IDE ワークフローから外れる**: Rust バイナリは IDE でコンパイルできず、
  scp+ssh 起動になる。運用手順の整備が要る。
- **ライセンス（解決済み）**: Bela コアソフトは LGPL 3.0 と確認。クレートは
  MIT/Apache-2.0 デュアルで確定してよい（上記「新リポジトリの構成」参照）。
- **クレート名の衝突（解決済み）**: `bela` / `bela-sys` / `bela-rs` は crates.io に
  未登録と確認。衝突なし。
- **ヘッダのバージョンずれ**: ボード到着前は BelaPlatform/Bela master のヘッダで
  bindgen するが、Gem 出荷イメージの Bela が別ブランチ/バージョンの可能性がある
  （公開情報では未確認）。フェーズ 0 でボード上の Bela バージョンを確認し、
  ヘッダの取得元をそこへピン留めし直す。
- **メンテ負担**: ニッチクレートを自前保有することになるが、コア限定なら表面積は小さい。

## 推奨する最初の一手

ボード未着の現時点では、**workspace 雛形 + ヘッダ vendoring + bindgen で
`cargo check --target aarch64-unknown-linux-gnu` を通す**ところまでが最初の
マイルストーン。次いで safe ラッパーをホストのユニットテスト付きで実装する。
ボードが届いたらフェーズ 0（`docs/board-facts.md` の採取）を最優先で行い、
リンクフラグを接続して実機受け入れ（passthrough / sine で音が出る）に進む。

## 参照

### コア API / 統合方式
- Bela: Using the core with other programs —
  https://learn.bela.io/using-bela/advanced-topics/using-the-bela-core-with-other-programs/
- BelaContext Struct Reference — https://docs.bela.io/structBelaContext.html
- Bela.h — https://github.com/BelaPlatform/Bela/blob/master/include/Bela.h

### 既存 Rust バインディング / クロスビルド事例（設計の参考元）
- andrewcsmith/bela-sys — https://github.com/andrewcsmith/bela-sys
- andrewcsmith/bela-rs — https://github.com/andrewcsmith/bela-rs
- padenot/bela-sys — https://github.com/padenot/bela-sys
- electric-snow: Rust ❤ Bela – FFI API Design —
  https://electric-snow.net/2021/10/08/rust-heart-bela-ffi-api-design/
- glicol/glicol-bela — https://github.com/glicol/glicol-bela
- maxmarsc/xc-bela-cmake — https://github.com/maxmarsc/xc-bela-cmake

### Bela Gem / PocketBeagle 2 / Xenomai
- Migrating to Bela Gem（API 差分の公式ドキュメント）—
  https://learn.bela.io/get-started-guide/migrating-to-bela-gem/
- Bela Gem on PocketBeagle 2 (BeagleBoard) —
  https://www.beagleboard.org/blog/2025-07-10-bela-gem-brings-ultra-low-latency-audio-to-pocketbeagle-2
- Crowd Supply: I/O & Parallel Processing —
  https://www.crowdsupply.com/bela/bela-gem-stereo-and-multi/updates/i-o-and-parallel-processing
- PocketBeagle 2 Debian images (v6.12 arm64) — https://www.beagleboard.org/distros
- Xenomai 3 / Cobalt / Dovetail — https://v3.xenomai.org/overview/
