# RevyCraft

RevyCraft は、Rust で実装された server-only の Minecraft workspace です。runtime は packaged plugin を読み込み、`ServerSupervisor` を公開入口として boot / status / reload / shutdown を扱います。

Java Edition の TCP adapter と Bedrock の UDP adapter を同一 process で扱う構成を前提にしていますが、どの plugin と profile が active になるかは `runtime/server.toml` の allowlist と selection で決まります。runtime は `target/` の build artifact を直接読まず、`runtime/plugins/<plugin-id>/plugin.toml` を起点に packaged plugin を解決します。

## まず読む場所

| 読者 | 最初に読む文書 |
| --- | --- |
| 全員 | [`docs/README.md`](docs/README.md) |
| 運用者 | [`docs/operators/getting-started.md`](docs/operators/getting-started.md) |
| 実装 contributors | [`docs/contributors/repository-overview.md`](docs/contributors/repository-overview.md) |
| plugin 作者 | [`docs/plugin-authors/plugin-model.md`](docs/plugin-authors/plugin-model.md) |

## 前提条件

- Rust stable toolchain と `cargo` が必要です。
- `runtime/server.toml` は開発起動と通常運用で使う active config です。`cargo run -p revy-server` は `REVY_SERVER_CONFIG` があればその path、無ければ `runtime/server.toml` を読みます。
- `runtime/server.toml.example` は sample config 兼、`cargo run -p xtask -- build-release-bundles` の既定 source です。`cargo run -p xtask -- package-plugins` は `runtime/server.toml` が無いときだけこれに fallback します。
- runtime の実行条件は「build 済み」ではなく「package 済み」です。server は `runtime/plugins/` を読み、`target/` を直接見ません。

## 最短の起動手順

```bash
cargo run -p xtask -- package-plugins
cargo run -p revy-server
```

`runtime/server.toml` がまだ無い場合は、先に `runtime/server.toml.example` を見て active config を用意してください。`package-plugins` は `--config` 指定が無い場合、`runtime/server.toml` を優先し、存在しないときだけ `runtime/server.toml.example` に fallback します。`server-bootstrap` は `REVY_SERVER_CONFIG` があればその path、無ければ `runtime/server.toml` を読みます。選ばれた path が存在しない場合は fail-fast で起動失敗します。

## 主要コマンド早見表

| コマンド | 使う場面 |
| --- | --- |
| `cargo run -p xtask -- package-plugins` | allowlist に入っている managed plugin だけを `runtime/plugins/` へ package したい |
| `cargo run -p xtask -- package-all-plugins` | optional plugin を含めて workspace 管理下の plugin を全量 package したい |
| `cargo run -p revy-server` | `runtime/server.toml` を使って server を起動したい |
| `cargo run -p xtask -- build-release-bundles --target <triple>` | target ごとの配布 bundle を作りたい |
| `cargo run -p xtask -- check-boundaries` | workspace の crate 境界 drift を確認したい |

## 何をしたいか

- 初回セットアップと最初の起動を進めたい  
  [`docs/operators/getting-started.md`](docs/operators/getting-started.md)
- 日常運用、gRPC admin 有効化、障害切り分けを見たい  
  [`docs/operators/operational-playbook.md`](docs/operators/operational-playbook.md)
- `runtime/server.toml` の key、reload 反映境界、admin surface の仕様を確認したい  
  [`docs/operators/configuration-and-reload.md`](docs/operators/configuration-and-reload.md)
- workspace 構成と boot path を掴みたい  
  [`docs/contributors/repository-overview.md`](docs/contributors/repository-overview.md)
- plugin の model と Rust SDK の使い分けを知りたい  
  [`docs/plugin-authors/plugin-model.md`](docs/plugin-authors/plugin-model.md)

package、release bundle、config、reload、admin surface、日常運用の詳細は [`docs/operators/getting-started.md`](docs/operators/getting-started.md)、[`docs/operators/operational-playbook.md`](docs/operators/operational-playbook.md)、[`docs/operators/configuration-and-reload.md`](docs/operators/configuration-and-reload.md) を参照してください。

## docs ハブ

`docs/` は読者別に正本を分けています。目的別の入口と用語集は [`docs/README.md`](docs/README.md) にまとめています。
