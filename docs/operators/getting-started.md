# 運用者向けスタートガイド

この文書は、RevyCraft を package、起動、release bundle 化するための正本です。`runtime/server.toml` の各 key や reload の意味は [`configuration-and-reload.md`](configuration-and-reload.md) を参照してください。

## 開発起動

通常の開発起動は次の 2 コマンドです。

```bash
cargo run -p xtask -- package-plugins
cargo run -p server-bootstrap
```

`package-plugins` は managed plugin を build して `runtime/plugins/` へ package します。`--config` を指定しない場合の config 解決順は次です。

1. `runtime/server.toml`
2. `runtime/server.toml.example`
3. どちらも無ければ error

このとき `live.plugins.allowlist` に含まれる plugin だけを package します。allowlist が無い、または空の場合は失敗します。managed plugin のうち allowlist から外れたものは packaging 対象から外れますが、workspace 外から持ち込んだ third-party plugin directory は消しません。

`server-bootstrap` は `REVY_SERVER_CONFIG` があればその path、無ければ `runtime/server.toml` を選びます。選ばれた path が存在しない場合は fail-fast で起動失敗します。

## package と boot が読む config の違い

同じ `runtime/` 配下のファイルでも、コマンドごとに既定の source of truth が違います。

| コマンド | 既定の config source | path が無い場合 |
| --- | --- | --- |
| `cargo run -p xtask -- package-plugins` | `runtime/server.toml` を優先し、無ければ `runtime/server.toml.example` | error |
| `cargo run -p server-bootstrap` | `REVY_SERVER_CONFIG` または `runtime/server.toml` | error |
| `cargo run -p xtask -- build-release-bundles` | `runtime/server.toml.example` | error |

この差は意図的です。開発 packaging は active config に寄せ、runtime boot と release bundle はどちらも選ばれた config の存在を必須にしています。

## optional plugin を含めて全量 package したいとき

workspace 管理下の plugin を allowlist 無視で全量 package したいときだけ、次を使います。

```bash
cargo run -p xtask -- package-all-plugins
```

これは `auth-mojang-online`、`auth-bedrock-xbl`、`auth-online-stub`、`be-placeholder` のような optional plugin も含めます。通常の開発起動では `package-plugins` のほうが、実際に起動する selection と package 結果を揃えやすくなります。

## release bundle を作るとき

配布用 bundle は target ごとに生成します。

```bash
cargo run -p xtask -- build-release-bundles \
  --target x86_64-unknown-linux-gnu \
  --target aarch64-apple-darwin
```

既定では `runtime/server.toml.example` を読み、`dist/releases/<target>/` に bundle を生成します。bundle には次が入ります。

- `server-bootstrap` の release binary
- `runtime/server.toml`
- allowlist に一致する packaged plugin 群
- source config が既定の `runtime/server.toml.example` だった場合のみ、その example file

次は含みません。

- `world` などの運用データ
- admin token などの秘匿情報

出力先や config source を変えるときは明示的に指定します。

```bash
cargo run -p xtask -- build-release-bundles \
  --target x86_64-pc-windows-msvc \
  --output-dir artifacts/releases \
  --config runtime/server.toml.example
```

cross target build に必要な Rust target component や linker 設定は事前に用意してください。

## runtime ディレクトリの見方

- `runtime/server.toml`
  開発時に優先して使う active config です。
- `runtime/server.toml.example`
  sample config 兼、release bundle 生成時の既定 source です。
- `runtime/plugins/<plugin-id>/`
  packaged plugin の配置先です。各 directory に `plugin.toml` と shared library が入ります。
- `runtime/world/`
  sample config の既定 world data 置き場です。

## 起動後に見えるもの

起動直後は listener の bind 結果と runtime status summary が標準出力へ出ます。さらに次の admin surface が条件つきで有効になります。

- local console
  `live.admin.ui_profile` で有効な admin-ui profile が解決できた場合に、stdio 上の line-oriented operator loop が起動します。
- remote gRPC transport
  `static.admin.remote.transport_profile = "grpc-v1"` の場合に、admin-transport plugin 経由で unary gRPC control plane が bind されます。

console EOF の扱い、permission、reload command、remote admin principal 設定は [`configuration-and-reload.md`](configuration-and-reload.md) を参照してください。
