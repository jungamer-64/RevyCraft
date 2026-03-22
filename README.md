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
- `runtime/`: `server.toml.example` と実行時 plugin/world 配置

## Run

通常起動では、`cargo run -p xtask -- package-plugins` が `runtime/server.toml` を優先して読み、存在しない場合だけ `runtime/server.toml.example` に fallback します。`[plugins].allowlist` に含まれる plugin だけを package し、workspace 外で持ち込んだ third-party plugin directory は消しません。

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

config を明示したいときは次を使います。

```bash
cargo run -p xtask -- package-plugins --config runtime/server.toml.example
```

`server-bootstrap` は `runtime/server.toml` を読みます。設定ファイルが無い場合は default config で起動し、plugin は `runtime/plugins/<plugin-id>/plugin.toml` から解決します。runtime は `target/` の build artifact を直接読みません。

起動フローは固定です。

1. config 読み込み
2. `mc-plugin-host` 構築
3. plugin activation
4. `LoadedPluginSet` snapshot 取得
5. `ServerBuilder` または `ServerBuilder::with_reload_host(...)` で listener / topology / session 起動

## Configuration Notes

- `[bootstrap].level_type = "flat"` のみ対応です。
- `[topology].be_enabled = true` を指定すると、同じ `[network].server_port` で `TCP(JE)` と `UDP(BE)` を同時 bind します。
- `[topology].enabled_adapters = ["je-1_7_10", "je-1_8_x", "je-1_12_2"]` で JE multi-version routing を有効化できます。
- `[topology].enabled_bedrock_adapters = ["be-26_3"]` で Bedrock baseline listener を有効化できます。
- `[profiles].default_gameplay` と `[profiles.gameplay_map]` で adapter ごとの gameplay profile を固定できます。
- `[plugins].reload_watch = true` と `[topology].reload_watch = true` は reload host を渡した `ReloadableRunningServer` でだけ使えます。
- 手動 reload は `ReloadableRunningServer::reload_plugins().await` / `ReloadableRunningServer::reload_topology().await` / `ReloadableRunningServer::reload_config().await` が使えます。
- `RunningServer::status().await` は active/draining topology、listener、session summary、plugin health を返します。

sample config は offline-first です。JE online auth や Bedrock XBL を使うときは `package-all-plugins` 実行後に allowlist と profile を明示してください。

- JE online auth:
  `auth-mojang-online` を allowlist に追加し、`[bootstrap].online_mode = true` と `[profiles].auth = "mojang-online-v1"`
- Bedrock XBL:
  `auth-bedrock-xbl` を allowlist に追加し、`[profiles].bedrock_auth = "bedrock-xbl-v1"`

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

plugin crate の in-process helper は production public surface ではなく、`in-process-testing` feature を有効にした test/dev build でだけ使う前提です。reusable な host fixture は `mc-plugin-host` 本体ではなく `mc-plugin-host-test-support` crate に分離されていて、`mc_plugin_host_test_support::TestPluginHostBuilder` と `TestPluginHost::discover(...)` を中心にした method-based API が正規入口です。custom fake plugin を差し込む raw escape hatch も `mc_plugin_host_test_support::raw::*` に集約されています。`mc_plugin_host::__test_hooks` はその shared crate を支える implementation-only の unsupported path です。

plugin host と SDK が共有する gameplay host blob helper は `mc_plugin_api::codec::gameplay::host_blob::*` にまとまっていて、runtime/host integration 専用の surface として扱います。

`mc_plugin_sdk_rust::__macro_support` は exported macro 用、`mc_proto_je_common::__version_support` と `mc_proto_be_common::__version_support` は protocol version crate 用の unsupported path です。binary / semantic codec の分離は `mc-plugin-api` crate 内部実装で、workspace 外からの直接利用や互換は保証しません。

## Notes

- 既定の実行時データは `runtime/` 配下に集約されます。
- gameplay profile は `canonical` / `readonly` が bundled sample です。
- storage profile sample は `je-anvil-1_7_10` です。
- auth profile sample は `offline-v1` / `bedrock-offline-v1` です。
- Bedrock は creative-style world interaction baseline までで、chat / containers / combat / mobs / Nether / End は未実装です。
- player inventory は window `0` 中心で、general container / crafting は未実装です。
- Linux では packaged `.so` を使った `dlopen + reload` integration test まで入っています。
