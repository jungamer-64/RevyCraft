# runtime と plugin architecture

この文書は、runtime / plugin host / session lifecycle の責務境界をまとめた正本です。ここでは `reload runtime <mode>` を前提に、`core` を reloadable boundary の内側へ移した現在の architecture を説明します。operator 向けの config key や command surface は [`../operators/configuration-and-reload.md`](../operators/configuration-and-reload.md)、reload の内部意味論は [`reload-semantics-and-boundaries.md`](reload-semantics-and-boundaries.md)、`core` migration の詳細は [`core-reload-runtime-design.md`](core-reload-runtime-design.md) を参照してください。

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
  単なる `ServerCore` owner ではなく、reloadable `core runtime` owner として `ServerCore`、kernel revision、snapshot-isolated gameplay journal commit、tick / save、dirty flag、world_dir、`core` migration の export / materialize / reattach / swap / rollback を持ちます。
- `SessionRegistry`
  live session handle、accepted queue、connection id、session task、routing-only の pending login route を持ちます。
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
  semantic な `GameplayCommand` を評価し、invocation-scoped `GameplayTransaction` を通じて snapshot を読みつつ op journal を積みます。live core への validate/apply は runtime / `mc-core` 側が担当します。
- storage plugin
  world snapshot の load / save / import / export を担います。`core` migration blob は process-local であり、persistent storage schema とは共有しません。
- auth plugin
  Java offline / online、Bedrock offline / XBL の認証を担います。
- admin-ui plugin
  local console の parse / render を担います。

`mc-core` 自体は semantic command / event / inventory state machine に徹します。raw slot layout、JE の echo / reject、Bedrock の active window rewrite のような version / wire 差分は protocol plugin 側に残します。一方で reloadable boundary の観点では、`mc-core` は world snapshot だけではなく keepalive、dropped item、active mining、open window のような live-only state も含む `core runtime` として扱います。

## bootstrap 時の selection

`SelectionResolver::resolve_bootstrap(...)` は次を行います。

- storage profile を解決し、`world_dir` から snapshot を読む
- active auth profile を解決し、`online_mode` と descriptor mode の整合を確認する
- Bedrock 有効時だけ bedrock auth profile を解決する
- active admin-ui profile を解決する
- `LoadedPluginSet` から runtime が使う handle 群を確定する

このため、auth mode の整合や gameplay profile の存在確認は session 開始前にかなり弾かれます。

boot 時点では storage snapshot から `ServerCore` を materialize しますが、reload 時は同じ path を使いません。reload は [`core-reload-runtime-design.md`](core-reload-runtime-design.md) で定義する `CoreRuntimeStateBlob` と `SessionReattachRecord` を使い、online player を saved-player に落とさずに candidate core へ再接続禁止で張り替えます。

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
3. gameplay-owned command は `GameplayCommand` として detached gameplay callback へ渡される
4. gameplay plugin は snapshot-isolated な `GameplayTransaction` 上で read / write し、journal を返す
5. runtime / `mc-core` が live core に対して journal を validate/apply し、成功した commit だけ canonical `CoreEvent` を生成する
6. protocol plugin が `CoreEvent` を wire packet 群へ encode

`CoreEvent::LoginAccepted` は core 側の accept pointですが、shared session state の authoritative
な `Login -> Play` 遷移は session task が login success packet を実際に write できたあとに commit
します。その短いあいだだけ `SessionRegistry` の pending login route が `EventTarget::Player`
配送を bridge します。

型の細かい流れは [`core-command-event-flow.md`](core-command-event-flow.md) を参照してください。

stale snapshot conflict が起きた場合、runtime は gameplay callback を再実行しません。play command は authoritative resync/drop、login は transient failure disconnect、gameplay tick はその session の当該 tick だけ破棄して次 tick へ持ち越します。

play 中 session は `reload runtime core` と `reload runtime full` の primary target です。protocol 固有 session blob、gameplay 固有 session blob、`core` runtime blob を別々に export / import しつつ、connection 自体は切らない前提で再アタッチします。

## reload runtime の責務分割

公開 reload surface は `reload runtime artifacts / topology / core / full` を前提にします。

- `artifacts`
  selection を固定したまま protocol / gameplay / storage generation を入れ替える
- `topology`
  listener / routing generation を入れ替える
- `core`
  `ServerCore` を live session を切らずに差し替える
- `full`
  selection / topology / core migration を単一 transaction として扱う

この分割により、protocol 固有 session state、gameplay callback state、world-semantic state を別々に export / import しつつ、commit point は runtime 側で一元管理できます。

## admin control plane

operator surface は `server-runtime` に集約されています。

- local principal
  `local-console`
- remote principal
  `static.admin.principals.<id>`
- local console の parse / render
  active admin-ui plugin が担当
- gRPC transport
  active admin-transport plugin が認証後に `AdminControlPlaneHandle` を叩く

admin reload surface は `reload runtime <mode>` に統一されています。permission も `reload-runtime` に一本化されており、進行中 request は開始時点の snapshot で完了し、次の request から新設定へ切り替わります。

## どこで reload を読むか

reload を深く追うときは次を順に読むと把握しやすいです。

1. [`core-reload-runtime-design.md`](core-reload-runtime-design.md)
2. [`../../crates/runtime/server-runtime/src/runtime/core_loop/reload.rs`](../../crates/runtime/server-runtime/src/runtime/core_loop/reload.rs)
3. [`../../crates/runtime/server-runtime/src/runtime/topology_manager.rs`](../../crates/runtime/server-runtime/src/runtime/topology_manager.rs)
4. [`../../crates/runtime/server-runtime/src/runtime/reload_coordinator.rs`](../../crates/runtime/server-runtime/src/runtime/reload_coordinator.rs)
5. [`reload-semantics-and-boundaries.md`](reload-semantics-and-boundaries.md)
