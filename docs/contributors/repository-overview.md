# リポジトリ概要

## 概要

この文書は、RevyCraft の実装 contributors 向けの入口です。workspace の役割分担、起動経路、公開 surface と内部 surface の境界を最初に整理します。

## 対象読者

- runtime / plugin host / plugin crates の実装を追う contributors
- どこからコードを読み始めるべきか知りたい人

## この文書でわかること

- RevyCraft を読むときに最初に押さえる設計上の前提
- ワークスペースの責務分割
- `ServerSupervisor` を中心にした boot path
- 公開してよい surface と implementation-only surface の違い

## 関連資料

- [`../README.md`](../README.md)
- [`../operators/getting-started.md`](../operators/getting-started.md)
- [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)
- [`reload-semantics-and-boundaries.md`](reload-semantics-and-boundaries.md)

## 先に押さえる結論

- RevyCraft は monolithic server ではなく、packaged plugin を前提にした plugin-first runtime です。
- runtime の外向け entrypoint は `ServerSupervisor` です。boot、status、admin、manual reload はここから扱います。
- runtime が見るのは mutable registry ではなく `LoadedPluginSet` snapshot です。plugin catalog と active runtime view は同じではありません。
- 実行条件は「build 済み」ではなく「package 済み」です。`server-bootstrap` は `target/` を直接見ず、`runtime/plugins/<plugin-id>/plugin.toml` を基準にします。

## 最初に追うファイル

- `README.md`
- `docs/README.md`
- `runtime/server.toml.example`
- `apps/server/src/main.rs`
- `crates/runtime/server-runtime/src/runtime/mod.rs`
- `crates/runtime/server-runtime/src/runtime/bootstrap/builder.rs`
- `crates/plugin/mc-plugin-host/src/lib.rs`
- `crates/plugin/mc-plugin-host/src/registry.rs`
- `crates/plugin/mc-plugin-api/src/lib.rs`
- `crates/plugin/mc-plugin-sdk-rust/src/lib.rs`

## ワークスペース構成

| パス | 役割 |
| --- | --- |
| `apps/server` | `server-bootstrap` binary。config 読み込み、plugin host 構築、runtime 起動、stdio / gRPC admin transport の起動 |
| `crates/core/mc-core` | protocol 非依存の state machine。world/player state、command 適用、event 生成 |
| `crates/runtime/server-runtime` | transport、listener/topology、session supervision、status snapshot、admin control plane |
| `crates/plugin/mc-plugin-host` | packaged plugin discovery、activation、generation reload、quarantine、runtime selection |
| `crates/plugin/mc-plugin-api` | ABI `3.0`、manifest、host API、typed codec |
| `crates/plugin/mc-plugin-sdk-rust` | Rust plugin authoring SDK。manifest helper、traits、export macro |
| `crates/protocol/mc-proto-*` | edition/version adapter と codec 実装 |
| `plugins/*/*` | protocol / gameplay / storage / auth / admin-ui の concrete plugin 実装 |
| `crates/testing/*` | packaged harness、plugin-host fixture、protocol test support |
| `tools/xtask` | active-config packaging、full packaging、release bundle 生成 |
| `runtime` | 実行時設定、packaged plugin 配置先、world データ |

## 起動経路

通常の開発フローは次の順で追うと把握しやすいです。

1. `cargo run -p xtask -- package-plugins`
2. `cargo run -p server-bootstrap`

内部では次の順で責務が流れます。

1. `xtask` が `runtime/server.toml` を優先し、無ければ `runtime/server.toml.example` を読んで allowlist 対象だけを `runtime/plugins/` に package します。
2. `ServerSupervisor::boot(ServerConfigSource)` が config を materialize します。
3. `mc_plugin_host::host::plugin_host_from_config(...)` が packaged plugin catalog を構築します。
4. `PluginHost::load_plugin_set(...)` が runtime selection を解決し、`LoadedPluginSet` を返します。
5. `server-runtime` 内部の `boot_server(...)` が protocol / profile を有効化し、listener bind と runtime loop 起動を行います。
6. `ServerSupervisor` は内部の `RunningServer` を包み、status / reload / shutdown を外へ公開します。

## 公開 surface と内部 surface

| 区分 | 入口 | 使い方 |
| --- | --- | --- |
| 公開 runtime surface | `ServerSupervisor` | runtime の boot、manual reload、status、admin control plane はここを基準に扱います |
| 公開 plugin ABI | `mc_plugin_api::{abi, manifest, host_api, codec::*}` | host と plugin の wire 契約です |
| 公開 Rust authoring surface | `mc_plugin_sdk_rust::{protocol, gameplay, storage, auth, admin_ui, manifest, capabilities}` | Rust plugin 作者向けの正規入口です |
| runtime 内部 | `RunningServer`, `RuntimeServer`, `boot_server(...)` | runtime 実装を読むときだけ使う lower-level detail です |
| implementation-only | `mc_plugin_sdk_rust::__macro_support` | exported macro を支える内部実装で、直接依存しません |
| implementation-only | `mc_plugin_host::__test_hooks` | shared test crate を支える内部 path です |
| implementation-only | `mc_proto_je_common::__version_support`, `mc_proto_be_common::__version_support` | protocol version crate 用の内部 path です |
| integration-specific | `mc_plugin_api::codec::gameplay::host_blob::*` | runtime / host integration helper として使い、通常の plugin authoring surface とは分けて扱います |

## 主要 subsystem の責務

### `mc-core`

`mc-core` は protocol を知らない state machine です。runtime は `CoreCommand` を流し、plugin や transport は `CoreEvent` を各 wire format に変換します。

### `mc-plugin-host`

`mc-plugin-host` は discovery、manifest / ABI validation、profile activation、runtime selection、generation reload、quarantine を担います。runtime に渡すのは mutable registry ではなく immutable な `LoadedPluginSet` です。

### `server-runtime`

`server-runtime` は orchestration 層です。config、listener、session、tick / save loop、reload、status snapshot、admin control plane を束ねます。

### `mc-plugin-api` と `mc-plugin-sdk-rust`

`mc-plugin-api` は host と plugin が共有する契約、`mc-plugin-sdk-rust` は Rust からその契約を扱いやすくする helper です。plugin 作者向けには SDK、runtime / host 実装者向けには API crate を基準に読むのがわかりやすいです。

## テストで見るべき場所

- `crates/runtime/server-runtime/src/runtime/tests.rs`
- `crates/runtime/server-runtime/src/runtime/tests/reload/`
- `crates/plugin/mc-plugin-host/src/plugin_host/tests.rs`
- `crates/testing/mc-plugin-test-support`
- `crates/testing/mc-plugin-host-test-support`
- `crates/testing/mc-proto-test-support`

packaged integration tests は `mc-plugin-test-support` crate の `PackagedPluginHarness::shared()` を入口にし、`xtask package-all-plugins` を source of truth として扱います。in-process plugin fixture は `mc-plugin-host-test-support` crate に集約されていて、`mc_plugin_host_test_support::TestPluginHostBuilder` と `TestPluginHost::discover(...)` が正規入口です。
