# RevyCraft

Minecraft JE `1.7.10` / `1.8.x` / `1.12.2` と Bedrock `be-26_3` を同一プロセス・同一ポート番号で扱う Rust 製 server-only workspace です。runtime は protocol / gameplay / storage / auth / admin-ui plugin を packaged artifact から読み込み、`mc-plugin-host` がそれを `LoadedPluginSet` にまとめ、`ServerSupervisor` が listener / generation / session supervision を起動します。

`LoadedPluginSet` は pure snapshot で、公開 API の起点は `ServerSupervisor` です。手動 reload は `ServerSupervisor::reload_plugins()` / `reload_config()` / `reload_generation()` から扱います。

## Workspace

- `apps/server`: `server-bootstrap` binary
- `crates/core`: `mc-core`
- `crates/plugin`: `mc-plugin-api`, `mc-plugin-host`, `mc-plugin-sdk-rust`
- `crates/protocol`: `mc-proto-*`
- `crates/runtime`: `server-runtime`
- `plugins/auth`: `mc-plugin-auth-*`
- `plugins/admin-ui`: `mc-plugin-admin-ui-*`
- `plugins/gameplay`: `mc-plugin-gameplay-*`
- `plugins/protocol`: `mc-plugin-proto-*`
- `plugins/storage`: `mc-plugin-storage-*`
- `tools/xtask`: plugin packaging task
- `runtime/`: `server.toml.example` と実行時 plugin/world 配置

## Run

通常起動では、`cargo run -p xtask -- package-plugins` が `runtime/server.toml` を優先して読み、存在しない場合だけ `runtime/server.toml.example` に fallback します。`[live.plugins].allowlist` に含まれる plugin だけを package し、workspace 外で持ち込んだ third-party plugin directory は消しません。

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
- `admin-ui-console`

optional plugin も含めて全量 package したいときだけ、明示的に次を使います。

```bash
cargo run -p xtask -- package-all-plugins
```

配布用の release bundle を target ごとにまとめて作るときは次を使います。

```bash
cargo run -p xtask -- build-release-bundles \
  --target x86_64-unknown-linux-gnu \
  --target aarch64-apple-darwin
```

既定では `runtime/server.toml.example` を source of truth として読み、`dist/releases/<target>/` に runnable bundle を生成します。bundle には `server-bootstrap` の release binary、`runtime/server.toml`、既定 config を使った場合の `runtime/server.toml.example`、allowlist に一致する packaged plugin 群だけが入ります。`world` や admin token などの運用データは含みません。

cross target の build に必要な Rust target component と linker 設定は事前に用意してください。出力先や config を変えたいときは次を使います。

```bash
cargo run -p xtask -- build-release-bundles \
  --target x86_64-pc-windows-msvc \
  --output-dir artifacts/releases \
  --config runtime/server.toml.example
```

config を明示したいときは次を使います。

```bash
cargo run -p xtask -- package-plugins --config runtime/server.toml.example
```

`server-bootstrap` は `runtime/server.toml` を読みます。設定ファイルが無い場合は default config で起動し、plugin は `runtime/plugins/<plugin-id>/plugin.toml` から解決します。runtime は `target/` の build artifact を直接読みません。`[live.admin].ui_profile` で有効な admin-ui profile が解決できた場合は、stdio 上に line-oriented operator loop を起動し、parse/render は plugin に委譲されます。`[static.admin.grpc].enabled = true` のときは同じ process に unary gRPC control plane も起動します。

起動フローは固定です。

1. config 読み込み
2. `mc-plugin-host` 構築
3. plugin activation
4. `LoadedPluginSet` snapshot 取得
5. `ServerSupervisor::boot(...)` で listener / generation / session 起動

## Configuration Notes

- `[static.bootstrap].level_type = "flat"` のみ対応です。
- `[live.topology].be_enabled = true` を指定すると、同じ `[live.network].server_port` で `TCP(JE)` と `UDP(BE)` を同時 bind します。
- `[live.topology].enabled_adapters = ["je-1_7_10", "je-1_8_x", "je-1_12_2"]` で JE multi-version routing を有効化できます。
- `[live.topology].enabled_bedrock_adapters = ["be-26_3"]` で Bedrock baseline listener を有効化できます。
- `[live.profiles].default_gameplay` と `[live.profiles.gameplay_map]` で adapter ごとの gameplay profile を固定できます。
- `[live.admin].ui_profile` で active admin UI profile、`[live.admin].local_console_permissions` で stdio operator の権限を指定できます。
- `[static.admin.grpc]` で unary gRPC control plane を opt-in できます。既定では loopback bind のみ許可され、non-loopback bind には `allow_non_loopback = true` が必要です。`enabled` / `bind_addr` / `allow_non_loopback` は restart-required で、`principals.<id>.token_file` と `permissions` は `reload_config()` で live 更新されます。
- `[live.plugins].reload_watch = true` と `[live.topology].reload_watch = true` は reload-capable な `ServerSupervisor::boot(...)` でだけ使えます。
- 手動 reload は `ServerSupervisor::reload_plugins().await` / `ServerSupervisor::reload_generation().await` / `ServerSupervisor::reload_config().await` が使えます。
- operator command は `ServerSupervisor::admin_control_plane()` が実行し、stdio と gRPC の両方で `status` / `sessions` / `reload config` / `reload plugins` / `reload generation` / `shutdown` を request 単位で permission check します。
- built-in gRPC transport は plaintext h2 のみです。public bind は `allow_non_loopback = true` で明示 opt-in が必要で、TLS と public edge policy は reverse proxy / ingress 側で終端してください。
- `ServerSupervisor::status().await` は active/draining generation、listener、session summary、plugin health を返します。

sample config は offline-first です。JE online auth や Bedrock XBL を使うときは `package-all-plugins` 実行後に allowlist と profile を明示してください。

- JE online auth:
  `auth-mojang-online` を allowlist に追加し、`[static.bootstrap].online_mode = true` と `[live.profiles].auth = "mojang-online-v1"`
- Bedrock XBL:
  `auth-bedrock-xbl` を allowlist に追加し、`[live.profiles].bedrock_auth = "bedrock-xbl-v1"`

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
