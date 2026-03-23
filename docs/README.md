# RevyCraft Docs

## 概要

このディレクトリは、RevyCraft のドキュメントを読者別に整理した入口です。README はリポジトリ全体のポータル、ここは docs 全体のハブとして扱います。

## 対象読者

- コードを読む・直す実装 contributors
- サーバーを package / 起動 / reload する運用者
- plugin を追加実装する外部または workspace 内の plugin 作者

## この文書でわかること

- どの読者が、どの docs から読み始めればよいか
- docs 全体で共通に使う主要用語
- README と `docs/` の役割分担

## 関連資料

- [`../README.md`](../README.md)
- [`contributors/repository-overview.md`](contributors/repository-overview.md)
- [`operators/getting-started.md`](operators/getting-started.md)
- [`plugin-authors/plugin-model.md`](plugin-authors/plugin-model.md)

## 読み始める場所

| 読者 | 最初に読む文書 | 次に読む文書 |
| --- | --- | --- |
| 実装 contributors | [`contributors/repository-overview.md`](contributors/repository-overview.md) | [`contributors/runtime-and-plugin-architecture.md`](contributors/runtime-and-plugin-architecture.md) |
| 運用者 | [`operators/getting-started.md`](operators/getting-started.md) | [`operators/configuration-and-reload.md`](operators/configuration-and-reload.md) |
| plugin 作者 | [`plugin-authors/plugin-model.md`](plugin-authors/plugin-model.md) | [`plugin-authors/rust-sdk-and-manifest.md`](plugin-authors/rust-sdk-and-manifest.md) |

## 主要用語

- `packaged plugin`
  `runtime/plugins/<plugin-id>/` 配下に置かれる、shared library と `plugin.toml` の組です。runtime は `target/` を直接読みません。
- `LoadedPluginSet`
  `mc-plugin-host` が runtime selection を解決した結果として返す immutable snapshot です。protocol registry と、active な gameplay / storage / auth / admin-ui profile を含みます。
- `ServerSupervisor`
  runtime を外から操作する公開 entrypoint です。boot、status、session_status、admin control plane、manual reload をここから扱います。
- `RunningServer`
  `ServerSupervisor` の内部で使う lower-level handle です。runtime 実装を読むときにだけ意識すれば十分です。
- `generation`
  reload 境界をまたぐ世代番号です。topology generation と plugin generation の両方があり、session がどの世代に pin されているかの観測にも使います。
- `profile`
  config で選択される kind ごとの実行プロファイルです。例として gameplay profile、auth profile、admin-ui profile があります。
- `quarantine`
  壊れた candidate artifact や runtime failure を起こした active plugin を隔離し、同じ失敗を繰り返さないようにする failure policy です。

## 文書構成

- `contributors/`
  runtime の責務分離、plugin host、reload、安全境界、test harness、`CoreCommand` / `CoreEvent` / `GameplayEffect` の流れを追いたい実装 contributors 向けです。
  主な文書: [`contributors/repository-overview.md`](contributors/repository-overview.md), [`contributors/runtime-and-plugin-architecture.md`](contributors/runtime-and-plugin-architecture.md), [`contributors/core-command-event-flow.md`](contributors/core-command-event-flow.md), [`contributors/reload-semantics-and-boundaries.md`](contributors/reload-semantics-and-boundaries.md)
- `operators/`
  package、起動、設定、manual reload、watch reload、admin console / gRPC 運用を確認したい人向けです。
- `plugin-authors/`
  plugin kind、manifest、SDK、unsupported path、packaged layout を把握したい plugin 作者向けです。
