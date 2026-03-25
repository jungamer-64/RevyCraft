# runtime と plugin architecture

この文書は、runtime / plugin host / session lifecycle の責務境界をまとめた正本です。operator 向けの config key や reload 手順そのものは [`../operators/configuration-and-reload.md`](../operators/configuration-and-reload.md)、reload の内部意味論は [`reload-semantics-and-boundaries.md`](reload-semantics-and-boundaries.md) を参照してください。

## レイヤー構成

1. `apps/server`
   `server-bootstrap` binary。config 読み込み、runtime boot、stdio / gRPC admin surface を起動します。
2. `crates/runtime/server-runtime`
   listener、generation、session、status、reload、admin control plane を持つ orchestration 層です。
3. `crates/core/mc-core`
   protocol 非依存の semantic state machine です。
4. `crates/plugin/mc-plugin-host`
   packaged plugin discovery、activation、selection、reload、quarantine を担います。
5. `crates/plugin/mc-plugin-api` / `mc-plugin-sdk-rust`
   ABI 契約と Rust authoring helper です。
6. `plugins/*`
   protocol / gameplay / storage / auth / admin-ui の concrete plugin 実装です。

## runtime の state owner

`RuntimeServer` は façade で、実際の state owner は次の manager に分かれています。

- `SelectionManager`
  active config、`LoadedPluginSet`、auth/admin-ui selection、remote admin principal snapshot を持ちます。
- `TopologyManager`
  active / draining generation、listener worker、generation swap を持ちます。
- `RuntimeKernel`
  `ServerCore`、tick / save、dirty flag、world_dir を持ちます。
- `SessionRegistry`
  live session handle、accepted queue、connection id、session task を持ちます。
- `ReloadCoordinator`
  config source、static reload boundary、reload host、consistency gate、shutdown request を持ちます。

runtime を読むときは `runtime/mod.rs` -> `selection.rs` -> `topology_manager.rs` -> `kernel.rs` -> `session/*` / `admin.rs` の順が追いやすいです。

## package / discovery / activation

runtime が直接扱うのは packaged plugin です。workspace crate や `target/` の shared library をそのまま読むわけではありません。

### package

`xtask` は managed plugin を build し、`runtime/plugins/<plugin-id>/` に次を配置します。

- `plugin.toml`
- current host target 向け shared library

### discovery

`plugin_host_from_config(...)` は `static.plugins.plugins_dir` を走査し、`plugin.toml` を持つ directory を package として catalog 化します。この段階で見るのは plugin id、kind、platform に一致する artifact の有無です。

### activation

catalog に載った plugin がそのまま active になるわけではありません。active runtime view は config で決まります。

- protocol
  active adapter として registry に入る
- gameplay
  `default_gameplay` と `gameplay_map` で参照された profile だけ有効化
- storage
  `static.bootstrap.storage_profile` の 1 つだけ有効化
- auth
  `auth` と、Bedrock 有効時の `bedrock_auth` を有効化
- admin-ui
  `live.admin.ui_profile` で選ばれた profile を有効化

## `plugin.toml` と embedded manifest

plugin には 2 種類の manifest があります。

- packaged layout の `plugin.toml`
  plugin directory の発見と artifact 解決に使います。
- shared library 内の `PluginManifestV1`
  ABI、plugin kind、profile capability、reload capability の validation に使います。

Rust plugin 作者が `StaticPluginManifest` で書くのは後者です。host は `plugin.toml` で package を見つけ、library を load したあとに embedded manifest を検証します。

## `mc-core` と plugin の責務境界

- protocol plugin
  handshake routing、status / login / play packet の decode / encode、transport/version 固有の session state を持ちます。
- gameplay plugin
  semantic な `GameplayCommand` を評価し、invocation-scoped `GameplayTransaction` を通じて world / entity を直接更新します。
- storage plugin
  world snapshot の load / save / import / export を担います。
- auth plugin
  Java offline / online、Bedrock offline / XBL の認証を担います。
- admin-ui plugin
  local console の parse / render を担います。

`mc-core` 自体は semantic command / event / inventory state machine に徹します。raw slot layout、JE の echo / reject、Bedrock の active window rewrite のような version / wire 差分は protocol plugin 側に残します。

## bootstrap 時の selection

`SelectionResolver::resolve_bootstrap(...)` は次を行います。

- storage profile を解決し、`world_dir` から snapshot を読む
- active auth profile を解決し、`online_mode` と descriptor mode の整合を確認する
- Bedrock 有効時だけ bedrock auth profile を解決する
- active admin-ui profile を解決する
- `LoadedPluginSet` から runtime が使う handle 群を確定する

このため、auth mode の整合や gameplay profile の存在確認は session 開始前にかなり弾かれます。

## session lifecycle

### Java / TCP

- accept 時点では adapter 未確定
- handshake frame を protocol registry の probe に流して adapter を決める
- `Status` は protocol plugin が decode / encode し、runtime が MOTD と online player 数を埋める
- `Login` は auth plugin で `PlayerId` を得て、`CoreCommand::LoginStart` へ変換する

`online_mode = true` の場合、runtime は RSA 鍵と verify token を持ち、暗号化 handshake を挟みます。auth plugin reload が起きても、進行中 login は開始時点の auth generation で完結します。

### Bedrock / UDP

- `live.topology.be_enabled = true` のときだけ UDP listener が bind される
- network settings request で adapter を確定する
- login 後に `bedrock_auth` profile を使って認証する

### Play

play phase では次の流れになります。

1. protocol plugin が wire packet を `CoreCommand` へ decode
2. runtime が command を direct-core と gameplay-owned に分岐する
3. gameplay-owned command は `GameplayCommand` として gameplay plugin callback へ渡される
4. gameplay plugin は `GameplayTransaction` 上で read / write し、`Ok(())` のときだけ commit される
5. `mc-core` / commit 層が canonical `CoreEvent` を生成
5. protocol plugin が `CoreEvent` を wire packet 群へ encode

型の細かい流れは [`core-command-event-flow.md`](core-command-event-flow.md) を参照してください。

## admin control plane

operator surface は `server-runtime` に集約されています。

- local principal
  `local-console`
- remote principal
  `static.admin.grpc.principals.<id>`
- local console の parse / render
  active admin-ui plugin が担当
- gRPC transport
  plugin を経由せず `AdminControlPlaneHandle` を直接叩く

`reload config` で admin-ui profile や remote principal map が変わっても、進行中 request は開始時点の snapshot で完了し、次の request から新設定へ切り替わります。

## どこで reload を読むか

reload を深く追うときは次を順に読むと把握しやすいです。

1. [`../../crates/runtime/server-runtime/src/runtime/core_loop/reload.rs`](../../crates/runtime/server-runtime/src/runtime/core_loop/reload.rs)
2. [`../../crates/runtime/server-runtime/src/runtime/topology_manager.rs`](../../crates/runtime/server-runtime/src/runtime/topology_manager.rs)
3. [`../../crates/runtime/server-runtime/src/runtime/reload_coordinator.rs`](../../crates/runtime/server-runtime/src/runtime/reload_coordinator.rs)
4. [`reload-semantics-and-boundaries.md`](reload-semantics-and-boundaries.md)
