# RevyCraft

Minecraft JE `1.7.10` / `1.8.x` / `1.12.2` と Bedrock `be-26_3` を同一プロセス・同一ポート番号で扱う Rust 製 server-only workspace です。runtime は protocol / gameplay / storage / auth plugin を packaged artifact から読み込み、`mc-plugin-host` がそれを `LoadedPluginSet` にまとめ、`ServerBuilder` が listener / topology / session supervision を起動します。

`LoadedPluginSet` は pure snapshot で、reload capability は別です。reload を使うときだけ `ServerBuilder::with_reload_host(...)` から `ReloadableRunningServer` を作ります。

## Workspace

- `apps/server`: `server-bootstrap` binary
- `crates/core`: `mc-core`
- `crates/plugin`: `mc-plugin-api`, `mc-plugin-host`, `mc-plugin-sdk-rust`
- `crates/protocol`: `mc-proto-*`
- `crates/runtime`: `server-runtime`
- `plugins/auth`: `mc-plugin-auth-*`
- `plugins/gameplay`: `mc-plugin-gameplay-*`
- `plugins/protocol`: `mc-plugin-proto-*`
- `plugins/storage`: `mc-plugin-storage-*`
- `tools/xtask`: plugin packaging task
- `runtime/`: `server.properties.example` と実行時 plugin/world 配置

## Run

通常起動は sample-first です。`runtime/server.properties.example` の `plugin-allowlist` を source of truth にして、最小 9 plugin だけを package します。

```bash
cargo run -p xtask -- package-plugins
cargo run -p server-bootstrap
```

sample に含まれる plugin:

- `je-1_7_10`
- `je-1_8_x`
- `je-1_12_2`
- `be-26_3`
- `gameplay-canonical`
- `gameplay-readonly`
- `storage-je-anvil-1_7_10`
- `auth-offline`
- `auth-bedrock-offline`

optional plugin も含めて全量 package したいときだけ、明示的に次を使います。

```bash
cargo run -p xtask -- package-all-plugins
```

`server-bootstrap` は `runtime/server.properties` を読みます。設定ファイルが無い場合は default config で起動し、plugin は `runtime/plugins/<plugin-id>/plugin.toml` から解決します。runtime は `target/` の build artifact を直接読みません。

起動フローは固定です。

1. config 読み込み
2. `mc-plugin-host` 構築
3. plugin activation
4. `LoadedPluginSet` snapshot 取得
5. `ServerBuilder` または `ServerBuilder::with_reload_host(...)` で listener / topology / session 起動

## Configuration Notes

- `level-type=FLAT` のみ対応です。
- `be-enabled=true` を指定すると、同じ `server-port` で `TCP(JE)` と `UDP(BE)` を同時 bind します。
- `enabled-adapters=je-1_7_10,je-1_8_x,je-1_12_2` で JE multi-version routing を有効化できます。
- `enabled-bedrock-adapters=be-26_3` で Bedrock baseline listener を有効化できます。
- `default-gameplay-profile` と `gameplay-profile-map` で adapter ごとの gameplay profile を固定できます。
- `plugin-reload-watch=true` と `topology-reload-watch=true` は reload host を渡した `ReloadableRunningServer` でだけ使えます。
- 手動 reload も `ReloadableRunningServer::reload_plugins().await` / `ReloadableRunningServer::reload_topology().await` に限定されます。
- `RunningServer::status().await` は active/draining topology、listener、session summary、plugin health を返します。

sample config は offline-first です。JE online auth や Bedrock XBL を使うときは `package-all-plugins` 実行後に allowlist と profile を明示してください。

- JE online auth:
  `auth-mojang-online` を allowlist に追加し、`online-mode=true` と `auth-profile=mojang-online-v1`
- Bedrock XBL:
  `auth-bedrock-xbl` を allowlist に追加し、`bedrock-auth-profile=bedrock-xbl-v1`

`auth-online-stub` と `be-placeholder` は test / optional tooling 向けで、default sample には含めていません。

## Server Features

- Minecraft JE `1.7.10 / protocol 5`, `1.8.x / protocol 47`, `1.12.2 / protocol 340`
- Bedrock baseline `be-26_3 / protocol 924`
- handshake / status / login / play
- offline-mode / online-mode auth
- superflat overworld generation
- initial chunk send
- creative-style block break / place
- inventory window `0` sync
- held item / hotbar sync
- `1.12.2` offhand persistence with legacy degradation
- multiple players
- other-player spawn / teleport / head rotation sync
- block change sync
- keepalive
- `level.dat`, `playerdata/*.dat`, `region/*.mca` read/write
- dynamic plugin host / quarantine / generation reload
- same-process shared-port `TCP(JE)` + `UDP(BE)`
- read-only runtime / plugin introspection snapshots

## Tests

```bash
cargo test --workspace
```

`server-runtime` と `mc-plugin-host` の packaged integration tests は `mc-plugin-test-support` crate が共有する packaged-plugin harness を使い、`PackagedPluginHarness::shared()` を入口に `xtask package-all-plugins` を source of truth として扱います。

plugin crate の in-process helper は production public surface ではなく、`in-process-testing` feature を有効にした test/dev build でだけ使う前提です。`mc-plugin-host` 側の test-only helper は `mc_plugin_host::test_support::TestPluginHostBuilder` と `TestPluginHost::discover(...)` を中心にした method-based API を正規入口に固定し、custom fake plugin を差し込む raw escape hatch だけを同 module に残しています。

plugin host と SDK が共有する gameplay host blob helper は `mc_plugin_api::codec::gameplay::host_blob::*` にまとまっていて、runtime/host integration 専用の surface として扱います。

`mc_plugin_sdk_rust::__macro_support` と `mc_plugin_api::codec::__internal` は実装都合の unsupported path です。workspace 外からの直接利用や互換は保証しません。

## Notes

- 既定の実行時データは `runtime/` 配下に集約されます。
- gameplay profile は `canonical` / `readonly` が bundled sample です。
- storage profile sample は `je-anvil-1_7_10` です。
- auth profile sample は `offline-v1` / `bedrock-offline-v1` です。
- Bedrock は creative-style world interaction baseline までで、chat / containers / combat / mobs / Nether / End は未実装です。
- player inventory は window `0` 中心で、general container / crafting は未実装です。
- Linux では packaged `.so` を使った `dlopen + reload` integration test まで入っています。
