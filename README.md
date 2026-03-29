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

## 最短の起動手順

```bash
cargo run -p xtask -- package-plugins
cargo run -p revy-server
```

`package-plugins` は `--config` 指定が無い場合、`runtime/server.toml` を優先し、存在しないときだけ `runtime/server.toml.example` に fallback します。`server-bootstrap` は `REVY_SERVER_CONFIG` があればその path、無ければ `runtime/server.toml` を読みます。選ばれた path が存在しない場合は fail-fast で起動失敗します。

package、release bundle、config、reload、admin surface の詳細は [`docs/operators/getting-started.md`](docs/operators/getting-started.md) と [`docs/operators/configuration-and-reload.md`](docs/operators/configuration-and-reload.md) を参照してください。

## docs ハブ

`docs/` は読者別に正本を分けています。目的別の入口と用語集は [`docs/README.md`](docs/README.md) にまとめています。
