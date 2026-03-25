# RevyCraft Docs

この `docs/` は、読者ごとに「どの文書を正本として読めばよいか」を揃えるためのハブです。リポジトリの入口は [`../README.md`](../README.md)、ここは audience ごとの導線と topic ごとの正本を示す場所として扱います。

## 読者別の入口

| 読者 | 最初に読む文書 | その後の正本 |
| --- | --- | --- |
| 運用者 | [`operators/getting-started.md`](operators/getting-started.md) | [`operators/configuration-and-reload.md`](operators/configuration-and-reload.md) |
| 実装 contributors | [`contributors/repository-overview.md`](contributors/repository-overview.md) | [`contributors/runtime-and-plugin-architecture.md`](contributors/runtime-and-plugin-architecture.md) |
| plugin 作者 | [`plugin-authors/plugin-model.md`](plugin-authors/plugin-model.md) | [`plugin-authors/rust-sdk-and-manifest.md`](plugin-authors/rust-sdk-and-manifest.md) |

## トピックごとの正本

- package / 起動 / release bundle
  [`operators/getting-started.md`](operators/getting-started.md)
- `runtime/server.toml` の解釈、相対 path 解決、reload、admin console / gRPC
  [`operators/configuration-and-reload.md`](operators/configuration-and-reload.md)
- workspace の責務分割、boot path、公開 surface と内部 surface
  [`contributors/repository-overview.md`](contributors/repository-overview.md)
- runtime / plugin host / session lifecycle の責務境界
  [`contributors/runtime-and-plugin-architecture.md`](contributors/runtime-and-plugin-architecture.md)
- `CoreCommand` / `GameplayEffect` / `CoreEvent` の流れ
  [`contributors/core-command-event-flow.md`](contributors/core-command-event-flow.md)
- reload の内部意味論、failure policy、consistency gate
  [`contributors/reload-semantics-and-boundaries.md`](contributors/reload-semantics-and-boundaries.md)
- plugin kind、packaged layout、discovery と activation
  [`plugin-authors/plugin-model.md`](plugin-authors/plugin-model.md)
- Rust SDK、macro、manifest、ABI `3.5`
  [`plugin-authors/rust-sdk-and-manifest.md`](plugin-authors/rust-sdk-and-manifest.md)

## 共通用語

- `packaged plugin`
  `runtime/plugins/<plugin-id>/` 配下にある `plugin.toml` と shared library の組です。
- `LoadedPluginSet`
  `mc-plugin-host` が runtime selection を解決した結果として返す immutable snapshot です。
- `ServerSupervisor`
  runtime の外向け公開入口です。boot、status、reload、shutdown、admin control plane をここから扱います。
- `generation`
  topology reload と plugin reload をまたいで観測するための世代番号です。
- `profile`
  auth / gameplay / storage / admin-ui の kind ごとに config で選ぶ実行プロファイルです。
- `quarantine`
  壊れた candidate artifact や active plugin を隔離する failure policy です。

## 読み方

ドキュメントどうしの説明が食い違うときは、topic ごとの正本を優先してください。それでも実装と差がある場合は、`apps/server`、`tools/xtask`、`crates/config/server-config`、`crates/runtime/server-runtime` が source of truth です。
