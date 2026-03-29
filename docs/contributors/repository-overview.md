# リポジトリ概要

この文書は、RevyCraft の実装 contributors 向け入口です。workspace の責務分割、boot path、公開 surface と内部 surface を最初に揃えます。

## 最初に追う順番

1. [`../../README.md`](../../README.md)
2. [`../README.md`](../README.md)
3. [`../../runtime/server.toml.example`](../../runtime/server.toml.example)
4. [`../../apps/revy-server/src/main.rs`](../../apps/revy-server/src/main.rs)
5. [`../../crates/runtime/revy-server-runtime/src/runtime/mod.rs`](../../crates/runtime/revy-server-runtime/src/runtime/mod.rs)
6. [`../../crates/plugin/mc-plugin-host/src/lib.rs`](../../crates/plugin/mc-plugin-host/src/lib.rs)

この順で「どこが入口で、何が package 済み前提で、どこまでが公開 API か」を先に掴むと読みやすくなります。

## workspace map

| パス | 役割 |
| --- | --- |
| `apps/revy-server` | `server-bootstrap` binary を持つ `revy-server` package。config 読み込み、runtime 起動、stdio / gRPC admin surface を束ねる |
| `crates/runtime/revy-server-config` | `runtime/server.toml` の load / normalize / validate を担う |
| `crates/runtime/revy-server-runtime` | listener、generation、session、status、reload、admin control plane を持つ orchestration 層 |
| `crates/core/revy-core` | id、capability、event targeting、revision、routing を持つ internal kernel primitive |
| `crates/core/revy-voxel-core` | protocol 非依存の semantic state machine |
| `crates/plugin/mc-plugin-api` | plugin ABI `5.0`、manifest、host API、typed codec |
| `crates/plugin/mc-plugin-host` | packaged plugin discovery、activation、selection、reload、quarantine |
| `crates/plugin/mc-plugin-sdk-rust` | Rust plugin authoring 向けの trait、manifest helper、macro |
| `crates/protocol/mc-proto-{common,je-common,be-common}` | shared protocol trait、wire codec、edition-family helper |
| `plugins/protocol/<adapter-id>/{mc-proto-*,mc-plugin-proto-*}` | version ごとの protocol bundle。codec / adapter 実装と host plugin wrapper を同居させる |
| `plugins/*/*` | gameplay / storage / auth / admin-surface の concrete plugin 実装 |
| `crates/testing/*` | packaged harness、plugin-host fixture、protocol test support |
| `tools/xtask` | package-plugins、package-all-plugins、build-release-bundles |
| `runtime/` | active config、packaged plugin、world data |

## boot path

通常の開発フローは次の 2 段階です。

1. `cargo run -p xtask -- package-plugins`
2. `cargo run -p revy-server`

内部では概ね次の責務順で流れます。

1. `xtask` が config から allowlist を読み、managed plugin を `runtime/plugins/` へ package する
2. `ServerSupervisor::boot(ServerConfigSource)` が config を materialize する
3. `mc_plugin_host::host::plugin_host_from_config(...)` が packaged plugin catalog を作る
4. `PluginHost::load_plugin_set(...)` が runtime selection を解決し、`LoadedPluginSet` を返す
5. `revy-server-runtime` が storage profile から world snapshot を読み、listener / generation / session supervision を起動する
6. `ServerSupervisor` が外向けの status / reload / shutdown / admin handle を公開する

重要なのは、runtime の実行条件が「build 済み」ではなく「package 済み」であることです。`server-bootstrap` は `target/` を直接見ず、`runtime/plugins/<plugin-id>/plugin.toml` を起点にします。

## 公開 surface

日常的に API として扱ってよいものは次です。

- `ServerSupervisor`
  boot、status、session_status、reload、shutdown、admin control plane の公開入口です。
- `revy_server_config::*`
  config schema と validation を扱う公開入口です。
- `mc_plugin_api`
  host と plugin 間の ABI 契約です。
- `mc_plugin_sdk_rust`
  Rust plugin authoring の正規入口です。

## implementation-only surface

次は内部実装として扱います。

- `RunningServer`、`RuntimeServer`
  runtime 実装を読むときの lower-level detail です。
- `revy-core`
  `revy-voxel-core` の下で使う internal kernel です。plugin ABI や protocol/storage plugin から直接参照しない前提で扱います。
- `mc_plugin_sdk_rust::__macro_support`
  macro の内部実装です。
- `mc_plugin_host::__test_hooks`
  test support 用の内部 surface です。
- `mc_proto_je_common::__version_support`
- `mc_proto_be_common::__version_support`

これらは external contract と見なさない前提で読んだほうが安全です。

## 主要な読みどころ

- runtime / plugin host の責務境界
  [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)
- `CoreCommand` / `GameplayCommand` / `GameplayTransaction` / `CoreEvent` の流れ
  [`core-command-event-flow.md`](core-command-event-flow.md)
- reload の意味論と failure policy
  [`reload-semantics-and-boundaries.md`](reload-semantics-and-boundaries.md)

## テストの入口

- packaged integration
  `crates/testing/mc-plugin-test-support`
- plugin host fixture
  `crates/testing/mc-plugin-host-test-support`
- protocol test support
  `crates/testing/mc-proto-test-support`

packaged integration では `xtask package-all-plugins` 系の成果物を source of truth とし、in-process fixture は `mc-plugin-host-test-support` 側に寄せています。
