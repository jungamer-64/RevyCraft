# リポジトリ概要

- 対象読者: RevyCraft の実装 contributors
- この文書で扱う範囲: workspace 構成、起動経路、公開 `surface`、主要な入口、テストと境界チェック
- この文書で扱わないこと: runtime / plugin host の詳細境界、`reload runtime` の内部設計、plugin authoring の実装細部
- 次に読む文書: [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)

この文書は contributors 向けの入口です。最初に「どこが入口で、何が packaged plugin 前提で、どこまでが公開 surface か」を揃えることに集中します。

## 最初に追う順番

1. [`../../README.md`](../../README.md)
2. [`../README.md`](../README.md)
3. [`../../runtime/server.toml.example`](../../runtime/server.toml.example)
4. [`../../apps/revy-server/src/main.rs`](../../apps/revy-server/src/main.rs)
5. [`../../crates/runtime/revy-server-runtime/src/runtime/mod.rs`](../../crates/runtime/revy-server-runtime/src/runtime/mod.rs)
6. [`../../crates/plugin/mc-plugin-host/src/lib.rs`](../../crates/plugin/mc-plugin-host/src/lib.rs)

この順で読むと、外向け入口、config source、runtime boot、plugin host の責務が最短でつながります。

## workspace 構成

| パス | 役割 |
| --- | --- |
| `apps/revy-server` | `server-bootstrap` binary。config 読み込み、runtime 起動、stdio / gRPC admin surface を束ねる |
| `crates/runtime/revy-server-config` | `runtime/server.toml` の load / normalize / validate と reload plan |
| `crates/runtime/revy-server-runtime` | listener、generation、session、status、reload、admin control plane を持つ orchestration 層 |
| `crates/core/revy-core` | id、capability、routing、revision を持つ internal kernel primitive |
| `crates/core/revy-voxel-core` | protocol 非依存の semantic state machine |
| `crates/plugin/mc-plugin-api` | plugin ABI `5.0`、manifest、host API、typed codec |
| `crates/plugin/mc-plugin-host` | packaged plugin discovery、activation、selection、reload、quarantine |
| `crates/plugin/mc-plugin-sdk-rust` | Rust plugin authoring 向け trait、manifest helper、macro |
| `crates/protocol/mc-proto-{common,je-common,be-common}` | shared protocol trait、wire codec、edition-family helper |
| `plugins/*/*` | protocol / gameplay / storage / auth / admin-surface の concrete plugin 実装 |
| `crates/testing/*` | packaged harness、plugin-host fixture、protocol test support |
| `tools/xtask` | `package-plugins`、`package-all-plugins`、`build-release-bundles`、`check-boundaries` |
| `runtime/` | active config、packaged plugin、world data |

## 起動経路

通常の開発フローは次の 2 段階です。

1. `cargo run -p xtask -- package-plugins`
2. `cargo run -p revy-server`

内部では概ね次の責務順で流れます。

1. `xtask` が config から allowlist を読み、managed plugin を `runtime/plugins/` へ package する
2. `ServerSupervisor::boot(ServerConfigSource)` が config を materialize する
3. `mc_plugin_host::host::plugin_host_from_config(...)` が packaged plugin catalog を作る
4. `PluginHost::load_plugin_set(...)` が runtime selection を解決し、`LoadedPluginSet` を返す
5. `revy-server-runtime` が storage profile から world snapshot を読み、listener / generation / session supervision を起動する
6. `ServerSupervisor` が status / reload / shutdown / admin handle を公開する

重要なのは、runtime の実行条件が「build 済み」ではなく「package 済み」であることです。`server-bootstrap` は `target/` を直接見ず、`runtime/plugins/<plugin-id>/plugin.toml` を起点にします。

## 公開 `surface`

日常的に API として扱ってよいものは次です。

- `ServerSupervisor`
  boot、status、session_status、reload、shutdown、admin control plane の公開入口です。
- `revy_server_config::*`
  config schema と validation を扱う公開入口です。
- `mc_plugin_api`
  host と plugin 間の ABI 契約です。
- `mc_plugin_sdk_rust`
  Rust plugin authoring の正規入口です。

## 内部専用 `surface`

次は内部実装として扱います。

- `RunningServer`、`RuntimeServer`
  runtime 実装を読むときの lower-level detail です。
- `revy-core`
  `revy-voxel-core` の下で使う internal kernel です。plugin ABI や protocol / storage plugin から直接参照しない前提で扱います。
- `mc_plugin_sdk_rust::__macro_support`
  macro の内部実装です。
- `mc_plugin_host::__test_hooks`
  test support 用の内部 surface です。
- `mc_proto_je_common::__version_support`
- `mc_proto_be_common::__version_support`

## 次に読む主正本

- runtime / plugin host / semantic boundary を把握する
  [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)
- `reload runtime` と `core` migration の意味論を把握する
  [`core-reload-runtime-design.md`](core-reload-runtime-design.md)
- play / login の command / event flow を追う
  [`core-command-event-flow.md`](core-command-event-flow.md)

## テストと境界チェック

- packaged integration
  `crates/testing/mc-plugin-test-support`
- plugin host fixture
  `crates/testing/mc-plugin-host-test-support`
- protocol test support
  `crates/testing/mc-proto-test-support`

boundary redesign の current debt と新規 drift を固定する check は次です。

```bash
cargo run -p xtask -- check-boundaries
```

この command は `tools/xtask/boundary-check.toml` を読み、forbidden dependency edge と canonical symbol owner / tracked duplicate symbol を検証します。
