# RevyCraft

RevyCraft は、Rust で実装された server-only の Minecraft workspace です。runtime は protocol / gameplay / storage / auth / admin-ui plugin を packaged artifact から読み込み、`ServerSupervisor` を公開入口として boot / status / reload / shutdown を扱います。

Java Edition の TCP adapter と Bedrock の UDP adapter を同一 process で扱う構成を前提にしていますが、何が active になるかは `runtime/server.toml` の allowlist と profile selection に依存します。runtime は `target/` の build artifact を直接読まず、`runtime/plugins/<plugin-id>/plugin.toml` を起点に packaged plugin を解決します。

## Docs

- docs ハブ: [`docs/README.md`](docs/README.md)
- 運用者向けの入口: [`docs/operators/getting-started.md`](docs/operators/getting-started.md)
- 実装 contributors 向けの入口: [`docs/contributors/repository-overview.md`](docs/contributors/repository-overview.md)
- plugin 作者向けの入口: [`docs/plugin-authors/plugin-model.md`](docs/plugin-authors/plugin-model.md)

## 最短の起動手順

```bash
cargo run -p xtask -- package-plugins
cargo run -p revy-server
```

`package-plugins` は `--config` 指定が無い場合、`runtime/server.toml` を優先し、存在しないときだけ `runtime/server.toml.example` に fallback します。`live.plugins.allowlist` に含まれる managed plugin だけを package し、workspace 外から持ち込んだ third-party plugin directory は残します。

`server-bootstrap` は `REVY_SERVER_CONFIG` があればその path、無ければ `runtime/server.toml` を読みます。選ばれた path が存在しない場合は warning を出し、built-in default config で起動します。この default では `runtime/plugins` と `runtime/world` を使います。

release bundle、config/reload、admin console / gRPC の正本は [`docs/operators/getting-started.md`](docs/operators/getting-started.md) と [`docs/operators/configuration-and-reload.md`](docs/operators/configuration-and-reload.md) にあります。

## リポジトリの入口

- `apps/revy-server`
  `server-bootstrap` binary です。config 読み込み、runtime 起動、stdio / gRPC admin surface をここで束ねます。
- `crates/runtime/revy-server-runtime`
  protocol 非依存の orchestration 層です。listener、generation、session、status、reload、admin control plane を持ちます。
- `crates/plugin`
  `mc-plugin-api`、`mc-plugin-host`、`mc-plugin-sdk-rust` をまとめた plugin 基盤です。
- `plugins/*/*`
  protocol / gameplay / storage / auth / admin-ui の concrete plugin 実装です。
- `runtime/`
  実行時 config、packaged plugin、world データの配置先です。
