# RevyCraft ドキュメント

この `docs/` は、読者ごとに「どの文書を正本として読めばよいか」を揃えるためのハブです。リポジトリの入口は [`../README.md`](../README.md)、ここは docs の導線と正本をまとめる場所として扱います。

## 読者別入口

| 読者 | 最初に読む文書 | 続けて読む文書 |
| --- | --- | --- |
| 運用者 | [`operators/getting-started.md`](operators/getting-started.md) | [`operators/configuration-and-reload.md`](operators/configuration-and-reload.md) |
| 実装 contributors | [`contributors/repository-overview.md`](contributors/repository-overview.md) | [`contributors/runtime-and-plugin-architecture.md`](contributors/runtime-and-plugin-architecture.md)、[`contributors/core-reload-runtime-design.md`](contributors/core-reload-runtime-design.md) |
| plugin 作者 | [`plugin-authors/plugin-model.md`](plugin-authors/plugin-model.md) | 同じ文書の Rust 実装・packaging 節 |

## やりたいこと別入口

- 開発環境で server を起動したい
  [`operators/getting-started.md`](operators/getting-started.md)
- `runtime/server.toml` の key と reload 反映境界を確認したい
  [`operators/configuration-and-reload.md`](operators/configuration-and-reload.md)
- plugin の kind、manifest、Rust SDK の使い分けを知りたい
  [`plugin-authors/plugin-model.md`](plugin-authors/plugin-model.md)
- workspace の入口、公開 surface、boot path を掴みたい
  [`contributors/repository-overview.md`](contributors/repository-overview.md)
- runtime / plugin host / semantic boundary を理解したい
  [`contributors/runtime-and-plugin-architecture.md`](contributors/runtime-and-plugin-architecture.md)
- `reload runtime` と `core` migration の内部設計を追いたい
  [`contributors/core-reload-runtime-design.md`](contributors/core-reload-runtime-design.md)
- `CoreCommand` / `GameplayCommand` / `GameplayTransaction` / `CoreEvent` の流れを追いたい
  [`contributors/core-command-event-flow.md`](contributors/core-command-event-flow.md)

## 正本一覧

| 分類 | 正本 |
| --- | --- |
| operator | [`operators/getting-started.md`](operators/getting-started.md) |
| operator | [`operators/configuration-and-reload.md`](operators/configuration-and-reload.md) |
| plugin author | [`plugin-authors/plugin-model.md`](plugin-authors/plugin-model.md) |
| contributor | [`contributors/repository-overview.md`](contributors/repository-overview.md) |
| contributor | [`contributors/runtime-and-plugin-architecture.md`](contributors/runtime-and-plugin-architecture.md) |
| contributor | [`contributors/core-reload-runtime-design.md`](contributors/core-reload-runtime-design.md) |
| contributor deep dive | [`contributors/core-command-event-flow.md`](contributors/core-command-event-flow.md) |

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
  auth / gameplay / storage / admin-surface の kind ごとに config で選ぶ実行プロファイルです。
- `quarantine`
  壊れた candidate artifact や active plugin を隔離する failure policy です。
