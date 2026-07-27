# クロスコンパイル環境の構築

ホスト (macOS) から Bela Gem (`aarch64-unknown-linux-gnu`) 向けにビルドするための手順。

> **状態: ドラフト。** ボード未着のため、sysroot 以降は未検証。
> 確定した値は随時この文書に反映する。

## 1. Rust ターゲット

リポジトリの `rust-toolchain.toml` が `aarch64-unknown-linux-gnu` ターゲットを
宣言しているため、rustup 管理下ならリポジトリ内で cargo を実行するだけで
自動インストールされる。

リンクを伴わないチェックはこれだけで通る:

```sh
cargo check --workspace --target aarch64-unknown-linux-gnu
```

## 2. クロスリンカ (macOS)

リンクが必要になった段階で導入する。macOS では
[messense/homebrew-macos-cross-toolchains](https://github.com/messense/homebrew-macos-cross-toolchains)
が手軽:

```sh
brew tap messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu
```

導入後、`.cargo/config.toml` の `linker` 設定のコメントを外す。

## 3. sysroot(ボード到着後)

libbela と Xenomai ラッパー lib、および依存ヘッダをボードから同期する
(Bela 公式クロスビルド環境の `SyncBelaSysroot` 相当)。

```sh
# 例: ボードの IP が bela.local の場合
rsync -avz --delete \
  --include-from=<採取した必要パスのリスト> \
  root@bela.local:/ ./bela-sysroot/
```

同期対象のパス(`/usr/lib`, `/usr/include`, `/root/Bela` など)は
フェーズ 0 で採取して `board-facts.md` に記録してから確定する。

設定箇所:

- `.cargo/config.toml` の `rustflags` に `--sysroot`
- `BINDGEN_EXTRA_CLANG_ARGS_aarch64_unknown_linux_gnu` に `--sysroot`

## 4. 転送と実行(ボード到着後)

Rust バイナリは Bela IDE ではビルドできないため、scp + ssh で運用する:

```sh
cargo build -p bela-rs --release --target aarch64-unknown-linux-gnu --example sine
scp target/aarch64-unknown-linux-gnu/release/examples/sine root@bela.local:
ssh root@bela.local ./sine
```

Bela IDE のデフォルトプログラムの停止方法(`systemctl stop bela` 等)は
フェーズ 0 で確認して追記する。
