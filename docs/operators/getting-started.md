# 運用者向けスタートガイド

- 対象読者: RevyCraft を package、起動、release bundle 化したい運用者
- この文書で扱う範囲: 初回セットアップ、開発起動、config source の選ばれ方、よく使うコマンド、release bundle、`runtime/` ディレクトリの見方
- この文書で扱わないこと: `runtime/server.toml` の各 key の意味、reload mode の内部設計、admin surface の詳細権限、障害対応のチェックリスト
- 次に読む文書: [`operational-playbook.md`](operational-playbook.md)、[`configuration-and-reload.md`](configuration-and-reload.md)

## 前提条件

- Rust stable toolchain と `cargo` が必要です。
- 開発起動では `runtime/server.toml` を active config として扱います。`runtime/server.toml.example` は sample config 兼、release bundle 生成時の既定 source です。
- `cargo run -p xtask -- package-plugins` は `runtime/server.toml` が無いときだけ `runtime/server.toml.example` に fallback しますが、`cargo run -p revy-server` は `runtime/server.toml` または `REVY_SERVER_CONFIG` が指す file のどちらかが必須です。
- cross target な release bundle を作るときだけ、対象 triple に対応する Rust target component や linker を追加で用意します。

## 初回セットアップ

通常の初回セットアップは次の順番です。

1. `runtime/server.toml.example` を見て、使いたい profile と allowlist の形を確認する
2. 必要なら `runtime/server.toml` を active config として用意する
3. plugin を package する
4. server を起動する
5. 標準出力で listener bind と status summary を確認する

最短の実行例は次です。

```bash
cargo run -p xtask -- package-plugins
cargo run -p revy-server
```

起動時に別の config を使う場合は `REVY_SERVER_CONFIG` を指定します。

```bash
REVY_SERVER_CONFIG=runtime/server.toml cargo run -p revy-server
```

`package-plugins` は managed plugin を build して `runtime/plugins/` へ package します。`--config` を指定しない場合の config 解決順は次です。

1. `runtime/server.toml`
2. `runtime/server.toml.example`
3. どちらも無ければ error

このとき `live.plugins.allowlist` に含まれる plugin だけを package します。allowlist が無い、または空の場合は失敗します。managed plugin のうち allowlist から外れたものは packaging 対象から外れますが、workspace 外から持ち込んだ third-party plugin directory は消しません。

`server-bootstrap` は `REVY_SERVER_CONFIG` があればその path、無ければ `runtime/server.toml` を選びます。選ばれた path が存在しない場合は fail-fast で起動失敗します。

## 起動成功時に確認すること

起動直後は `apps/revy-server` が標準出力へ次を出します。

- listener の bind 結果
  `server listening on ...`
- runtime status summary
  active generation、listener 数、session 数、adapter 情報、MOTD などの要約

さらに config で console admin surface を有効化している場合は、同じ stdio 上で `help`、`status`、`sessions`、`reload runtime ...` などの operator command を受け付けます。どこを見るか迷ったら [`operational-playbook.md`](operational-playbook.md) の「起動確認チェックリスト」を先に参照してください。

## よく使うコマンド

| コマンド | 主な用途 |
| --- | --- |
| `cargo run -p xtask -- package-plugins` | allowlist に含まれる managed plugin だけを package する |
| `cargo run -p xtask -- package-all-plugins` | optional plugin を含めて workspace 管理下の plugin を全量 package する |
| `cargo run -p revy-server` | active config で server を起動する |
| `cargo run -p xtask -- build-release-bundles --target <triple>` | target ごとの release bundle を作る |
| `cargo run -p xtask -- check-boundaries` | crate 境界 drift を検査する |

## package と boot が読む config の違い

同じ `runtime/` 配下のファイルでも、コマンドごとに既定の source of truth が違います。

| コマンド | 既定の config source | path が無い場合 |
| --- | --- | --- |
| `cargo run -p xtask -- package-plugins` | `runtime/server.toml` を優先し、無ければ `runtime/server.toml.example` | error |
| `cargo run -p revy-server` | `REVY_SERVER_CONFIG` または `runtime/server.toml` | error |
| `cargo run -p xtask -- build-release-bundles` | `runtime/server.toml.example` | error |

開発 packaging は active config に寄せ、runtime boot と release bundle はどちらも選ばれた config の存在を必須にしています。

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

cross target build に必要な Rust target component や linker 設定は事前に用意してください。`--target` で指定した triple が host に入っていない場合は、事前に `rustup target add <triple>` が必要です。

## `runtime/` ディレクトリの見方

- `runtime/server.toml`
  開発時に優先して使う active config です。通常起動の source of truth になります。
- `runtime/server.toml.example`
  sample config 兼、release bundle 生成時の既定 source です。active config を新しく作るときの叩き台にもなります。
- `runtime/admin-grpc.toml.example`
  gRPC admin surface plugin 向けの sample config です。`server.toml` から `config = "admin-grpc.toml"` のように参照するときの雛形として使います。
- `runtime/plugins/<plugin-id>/`
  packaged plugin の配置先です。各 directory に `plugin.toml` と shared library が入ります。runtime はここを起点に plugin を発見します。
- `runtime/world/`
  sample config の既定 world data 置き場です。
- `runtime/admin/`
  sample には含まれませんが、gRPC admin token file などの運用用 secret を置く候補 directory です。

## 起動後に見えるもの

起動直後は listener の bind 結果と runtime status summary が標準出力へ出ます。さらに config で有効化した admin surface が起動します。

- console admin surface
  `[live.admin.surfaces.console]` のように `console-v1` surface を有効化し、`static.admin.principals."console:<instance>"` に permission を与えると、stdio 上の line-oriented console surface が起動します。
- gRPC admin surface
  `[live.admin.surfaces.<instance>]` で `profile = "grpc-v1"` と plugin-owned config path を指定すると、gRPC admin surface plugin が unary gRPC control plane を bind します。

permission、reload command、principal 設定、surface config の扱いは [`configuration-and-reload.md`](configuration-and-reload.md) を参照してください。

## 次にやること

- 起動後の確認項目、gRPC admin 有効化、失敗時の切り分けを見たい  
  [`operational-playbook.md`](operational-playbook.md)
- `runtime/server.toml` の key と reload mode の反映境界を確認したい  
  [`configuration-and-reload.md`](configuration-and-reload.md)
