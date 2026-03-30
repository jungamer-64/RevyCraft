# `runtime` と `plugin` の設計

- 対象読者: `runtime` / plugin host / semantic boundary を理解したい contributors
- この文書で扱う範囲: 現行実装、目標境界、state owner、plugin の責務、目標 crate 構成、依存規則、境界チェック
- この文書で扱わないこと: operator 向け config key の意味、`reload runtime` の細かな transaction 手順、plugin authoring のコード例
- 次に読む文書: [`core-reload-runtime-design.md`](core-reload-runtime-design.md)

この文書は contributor 向け設計文書の主正本です。現行実装の説明に加えて、boundary redesign の目標境界、依存規則、境界チェックもここでまとめて扱います。

## 現行実装と目標境界

RevyCraft の current workspace は実装としては成立していますが、次の境界はまだ複数の crate にまたがって滲みやすい状態です。

- semantic contract と engine internal が `revy-voxel-core` / `mc-plugin-api` に混在している
- protocol と storage の責務が `mc-proto-*` / storage plugin にまたがっている
- admin / reload DTO が `revy-server-runtime`、`revy-server-config`、`mc-plugin-api`、`mc-plugin-host` に重複している
- config parsing / normalize / reload planning の読み順が 1 file に集中しやすい

この文書では、現在の `runtime` / plugin host の読み方を示しつつ、次の目標境界を正本として固定します。

- `revy-voxel-semantic`
  shared semantic contract を置く。plugin / protocol / storage が共有する world、player、gameplay、content DTO の canonical owner
- `revy-voxel-core`
  `ServerCore`、journal validate / apply、canonical event generation、inventory / world / runtime state machine のような engine internal を置く
- `mc-proto-common`
  protocol-only crate に寄せる
- `mc-storage-common`
  storage-only crate に寄せる
- `revy-server-types`
  operator-facing shared DTO の single source of truth に寄せる
- `revy-server-config`
  schema / document / normalize / validate / reload plan と neutral selection view を持つ

## レイヤー構成

1. `apps/revy-server`
   `server-bootstrap` binary を持つ `revy-server` package。config 読み込み、runtime boot、process-scope resource broker、admin surface supervisor を持ちます。
2. `crates/runtime/revy-server-runtime`
   listener、generation、session、status、reload、admin control plane を持つ orchestration 層です。
3. `crates/core/revy-core`
   id、capability、event targeting、revision control、session routing primitive を持つ internal kernel です。runtime や plugin ABI から直接見せる層ではありません。
4. `crates/core/revy-voxel-core`
   voxel / Minecraft 系の semantic state machine です。`revy-core` を内部 primitive として使います。
5. `crates/plugin/mc-plugin-host`
   packaged plugin discovery、activation、selection、reload、quarantine を担います。
6. `crates/plugin/mc-plugin-api` / `mc-plugin-sdk-rust`
   ABI 契約と Rust authoring helper です。
7. `plugins/*`
   protocol / gameplay / storage / auth / admin-surface の concrete plugin 実装です。

## `runtime` の state owner

`RuntimeServer` は facade で、実際の state owner は次の manager に分かれています。

- `SelectionManager`
  active config、`LoadedPluginSet`、auth / admin-surface selection、remote admin principal snapshot を持ちます。
- `TopologyManager`
  active / draining generation、listener worker、generation swap を持ちます。
- `RuntimeKernel`
  `ServerCore`、`revy-core` の revision primitive で包んだ kernel state、snapshot-isolated gameplay journal commit、tick / save、dirty flag、world_dir、`core` migration の export / materialize / reattach / swap / rollback を持ちます。
- `SessionRegistry`
  live session handle、accepted queue、`revy-core` の connection-id source、session task、routing-only の pending login route を持ちます。
- `ReloadCoordinator`
  config source、static reload boundary、reload host、consistency gate、shutdown request を持ちます。

runtime を読むときは `runtime/mod.rs` -> `selection.rs` -> `topology_manager.rs` -> `kernel.rs` -> `session/*` / `admin.rs` の順が追いやすいです。

## package / 発見 / 有効化

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
- admin-surface
  `live.admin.surfaces.<instance>` で選ばれた 0 個以上の surface instance を有効化

## `plugin.toml` と embedded manifest

plugin には 2 種類の manifest があります。

- package 形式の `plugin.toml`
  plugin directory の発見と artifact 解決に使います。
- shared library 内の `PluginManifestV1`
  ABI、plugin 種別、profile capability、reload capability の検証に使います。

Rust plugin 作者が `StaticPluginManifest` で書くのは後者です。host は `plugin.toml` で package を見つけ、library を load したあとに embedded manifest を検証します。

## `revy-voxel-core` と plugin の責務境界

`revy-core` と `revy-voxel-core` の境界は次のように固定します。

- `revy-core`
  `ConnectionId` / `PlayerId` / capability set、`EventTarget` / routed event、revision control、session routing primitive を持ちます。
- `revy-voxel-core`
  world state、inventory / container lifecycle、mining、login / bootstrap、`GameplayEffectBatch` validate/apply、canonical `CoreEvent` generation を持ちます。

plugin 種別ごとの責務は次です。

- protocol plugin
  handshake routing、status / login / play packet の decode / encode、transport / version 固有 session state を持ちます。
- gameplay plugin
  semantic な `GameplayCommand` を評価し、invocation-scoped read/effect recorder を通じて snapshot read と `GameplayEffectBatch` を返します。live core への validate / apply は runtime / `revy-voxel-core` 側が担当します。
- storage plugin
  world snapshot の load / save / import / export を担います。`core` migration blob は process-local であり、persistent storage schema とは共有しません。
- auth plugin
  Java offline / online、Bedrock offline / XBL の認証を担います。
- admin-surface plugin
  console / gRPC などの operator surface、identity mapping、surface-owned config、process / handoff resource を担います。

plugin / protocol authoring 側の依存もこの境界に合わせます。`mc-plugin-sdk-rust`、`mc-plugin-api`、`mc-proto-common` が公開 surface として shared semantic type を再公開するので、外側の crate は `revy-voxel-core` を直接依存先にせず、まずこれらの surface 経由で型を参照する前提で扱います。

## 迷ったときの境界判断

### app と runtime

`apps/revy-server` に置くのは process-scope の boot、stdio / gRPC admin surface、upgrade 協調です。session や world state の owner は `crates/runtime/revy-server-runtime` に寄せます。

### runtime と plugin host

runtime は「どの plugin を今の runtime view で使うか」を決めて使います。packaged plugin の discovery、activation、reload、quarantine は `mc-plugin-host` に寄せます。

### semantic と engine internal

plugin や protocol 共通層が共有してよい型は `revy-voxel-semantic` までです。`revy-voxel-core` と `revy-core` は engine internal として扱います。

### config と runtime translation

`revy-server-config` は schema、document load、normalize、validate、reload plan、neutral selection view に寄せます。runtime は `ServerConfig` から host view を受け取り、`mc-plugin-host` は自分の `config::*` へ変換する owner として扱います。

### build-time と run-time

実行時の正本は `target/` ではなく `runtime/plugins/<plugin-id>/plugin.toml` を起点にした packaged plugin です。runtime は「build 済みかどうか」ではなく「package 済みかどうか」を実行条件にします。

## 目標 crate 構成

```text
apps/revy-server
  -> revy-server-runtime
     -> revy-server-config
     -> revy-server-gameplay-bridge
     -> revy-server-types
     -> mc-plugin-host
     -> revy-voxel-core
        -> revy-voxel-semantic
           -> revy-core

mc-plugin-host
  -> revy-server-gameplay-bridge

mc-plugin-api
  -> revy-voxel-semantic
  -> revy-server-types

mc-plugin-sdk-rust
  -> mc-plugin-api
  -> revy-voxel-semantic
  -> revy-server-types

mc-proto-common
  -> revy-voxel-semantic

mc-storage-common
  -> revy-voxel-semantic

versioned protocol crates
  -> mc-proto-common
  -> edition-family helper

storage crates
  -> mc-storage-common
```

## 依存規則

次の規則を boundary redesign の境界チェックとして固定します。

- no crate outside runtime / core engine depends on `revy-voxel-core` or `revy-core` as a shared public contract proxy
- no storage crate depends on versioned protocol crate
- no config crate depends on `mc-plugin-host`
- no host crate depends on `revy-voxel-core`; gameplay read access is bridged through `revy-server-gameplay-bridge`
- no duplicated canonical admin / reload DTO definitions across `revy-server-config` / `revy-server-runtime` / `mc-plugin-api` / `mc-plugin-host`

最初の rule は、`ServerCore` / `GameplayTransaction` を public surface から消し、plugin-facing contract を `GameplayEffectBatch` と `GameplayReadView` に寄せることが目的です。runtime kernel が `GameplayLoginPreview` / snapshot adapter を所有し、`mc-plugin-host` は bridge trait 越しに読むだけにします。

## 移行順序と境界チェック

boundary redesign の進め方は次を前提にします。

1. docs と boundary check を入れて、current debt を explicit allowlist として固定する
2. `revy-voxel-semantic` を導入し、shared semantic type を移す
3. `mc-storage-common` と `mc-storage-je-anvil-1_7_10` を導入し、protocol / storage を切り離す
4. `revy-server-types` を導入し、admin / reload DTO を寄せる
5. plugin-host translation を runtime 直書き helper から neutral selection view + host-owned conversion へ移し、`mc-plugin-host` の gameplay read path を `revy-server-gameplay-bridge` 越しにする
6. `revy-server-config` を schema / document / normalize / validate / reload plan の分割構造に保つ

境界チェックの入口は次です。

```bash
cargo run -p xtask -- check-boundaries
```

この check は `cargo metadata` から direct workspace dependency を読み、forbidden edge を検出し、`tools/xtask/boundary-check.toml` の explicit allowlist と tracked duplicate symbol を照合します。いま全部 clean であることよりも、新しい drift を増やさないことを目的に使います。

local baseline と既知の failing test は、鮮度管理を分離するため [`known-issues.md`](known-issues.md) に切り出して追跡します。boundary redesign の code motion は、その文書にある baseline を green に戻すか、明示的に quarantine してから始める前提です。

## 関連文書

- current baseline、既知の failing test、quarantine 前提
  [`known-issues.md`](known-issues.md)
- reload の意味論、`consistency_gate`、`core` migration
  [`core-reload-runtime-design.md`](core-reload-runtime-design.md)
- play / login の command / event flow
  [`core-command-event-flow.md`](core-command-event-flow.md)
