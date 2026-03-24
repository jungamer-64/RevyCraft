# 運用者向けスタートガイド

## 概要

この文書は、RevyCraft を package、起動、配布する運用者向けの最短ガイドです。実行コマンド、sample plugin、runtime directory の見方をまとめます。

## 対象読者

- 開発中またはローカル検証用にサーバーを起動したい人
- release bundle を作りたい人
- `runtime/` 配下に何が置かれるか把握したい人

## この文書でわかること

- 通常起動に必要なコマンド
- sample plugin と allowlist packaging の前提
- `package-all-plugins` と release bundle の使い分け
- 起動後に admin console / gRPC がどう現れるか

## 関連資料

- [`../../README.md`](../../README.md)
- [`configuration-and-reload.md`](configuration-and-reload.md)
- [`../contributors/repository-overview.md`](../contributors/repository-overview.md)

## 最短の起動手順

通常の開発起動は次の 2 コマンドです。

```bash
cargo run -p xtask -- package-plugins
cargo run -p server-bootstrap
```

`package-plugins` は `runtime/server.toml` を優先して読み、存在しない場合だけ `runtime/server.toml.example` に fallback します。`live.plugins.allowlist` に含まれる plugin だけを `runtime/plugins/` に package し、workspace 外で持ち込んだ third-party plugin directory は消しません。

`server-bootstrap` は `runtime/server.toml` を読みます。設定ファイルが無い場合は default config で起動し、plugin は `runtime/plugins/<plugin-id>/plugin.toml` から解決します。runtime は `target/` の build artifact を直接読みません。

別 path の config で起動したいときは、`REVY_SERVER_CONFIG=/path/to/server.toml cargo run -p server-bootstrap` を使います。指定 path が見つからない場合は warning を出し、default config fallback で起動を試みます。

## sample に含まれる plugin

default sample で想定している plugin は次の 10 個です。

- `je-5`
- `je-47`
- `je-340`
- `be-924`
- `gameplay-canonical`
- `gameplay-readonly`
- `storage-je-anvil-1_7_10`
- `auth-offline`
- `auth-bedrock-offline`
- `admin-ui-console`

optional plugin として、`auth-mojang-online`、`auth-bedrock-xbl`、`auth-online-stub`、`be-placeholder` もあります。JE online auth や Bedrock XBL を使うときは allowlist と profile を明示してください。

## 全量 package したいとき

workspace 管理下の plugin を全量 build / package したいときだけ、次を使います。

```bash
cargo run -p xtask -- package-all-plugins
```

これは optional plugin も含めて package します。通常の開発起動では `package-plugins` を使うほうが、active config と実行内容が一致しやすくなります。

## release bundle を作るとき

配布用の runnable bundle を target ごとにまとめて作るときは次を使います。

```bash
cargo run -p xtask -- build-release-bundles \
  --target x86_64-unknown-linux-gnu \
  --target aarch64-apple-darwin
```

既定では `runtime/server.toml.example` を source of truth として読み、`dist/releases/<target>/` に bundle を生成します。bundle には次が入ります。

- `server-bootstrap` の release binary
- `runtime/server.toml`
- 既定 config を使った場合の `runtime/server.toml.example`
- allowlist に一致する packaged plugin 群

次は含みません。

- `world` などの運用データ
- admin token などの秘匿情報

出力先や config を変えたいときは、明示的に指定します。

```bash
cargo run -p xtask -- build-release-bundles \
  --target x86_64-pc-windows-msvc \
  --output-dir artifacts/releases \
  --config runtime/server.toml.example
```

cross target build に必要な Rust target component と linker 設定は事前に用意してください。

## runtime ディレクトリの見方

- `runtime/server.toml`
  実行時に優先して読む config です。
- `runtime/server.toml.example`
  sample config 兼、bundle 生成時の既定 source of truth です。
- `runtime/plugins/`
  packaged plugin の配置先です。
- `runtime/world/`
  world データの既定配置先です。

## 起動後に何が起きるか

起動フローは概ね次の順です。

1. config 読み込み
2. `mc-plugin-host` 構築
3. plugin activation
4. `LoadedPluginSet` snapshot 取得
5. `ServerSupervisor::boot(...)` で listener / generation / session supervision を開始

有効な `live.admin.ui_profile` が解決できた場合は、stdio 上に line-oriented operator loop を起動します。`static.admin.grpc.enabled = true` のときは同じ process に unary gRPC control plane も bind します。

## 次に読む文書

設定項目、reload 可能範囲、watch reload、admin console / gRPC の詳細は [`configuration-and-reload.md`](configuration-and-reload.md) を参照してください。
