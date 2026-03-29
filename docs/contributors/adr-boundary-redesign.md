# ADR: Boundary Redesign And Migration Guardrails

## Status

Accepted.

## Context

RevyCraft の current workspace は、実装としては成立している一方で、次の境界が複数の crate にまたがって滲んでいます。

- semantic contract と engine internal が `revy-voxel-core` / `mc-plugin-api` に混在している
- protocol と storage の責務が `mc-proto-*` / storage plugin にまたがっている
- admin / reload DTO が `revy-server-runtime`、`revy-server-config`、`mc-plugin-api`、`mc-plugin-host` に重複している
- `revy-server-config` が validated config だけでなく plugin-host translation まで持っている

このまま大規模 refactor を始めると、型移動や crate split のたびに境界が再び崩れやすくなります。そこで、最終形の crate graph と dependency rule を先に固定し、`xtask` による guardrail を入れてから code motion を進めます。

## Decision

次の target architecture を正本として扱います。

### semantic と engine を分離する

- `revy-voxel-semantic`
  shared semantic contract を置く
  `CoreCommand`、`CoreEvent`、`GameplayCommand`、`RuntimeCommand`、`SessionCommand`、`PlayerSnapshot`、`WorldSnapshot`、`CoreConfig`、ID / capability-facing alias
- `revy-voxel-core`
  engine internal だけを置く
  `ServerCore`、`CoreRuntimeStateBlob`、journal validate/apply、canonical event generation、inventory/world/runtime state machine

`mc-plugin-api`、`mc-plugin-sdk-rust`、`mc-proto-common` は最終的に `revy-voxel-semantic` を参照し、engine internal を public surface に再公開しない。

### protocol と storage を分離する

- `mc-proto-common`
  protocol-only crate にする
- `mc-storage-common`
  `StorageAdapter` / `StorageError` を置く
- `mc-storage-je-anvil-1_7_10`
  1.7.10 Anvil 実装を `mc-proto-je-5` から切り出す

storage crate は versioned protocol crate に直接依存しない。

### server/admin DTO を統一する

- `revy-server-types`
  operator-facing shared DTO の single source of truth
  `AdminPermission`、`RuntimeReloadMode`、`ListenerBinding`、`PluginFailureAction`、`PluginFailureMatrix`、`PluginHostStatusSnapshot`、admin request/response payload 群

`revy-server-runtime`、`mc-plugin-api`、`mc-plugin-host`、`revy-server-config` に分散した canonical DTO はここへ集約する。

### config と plugin-host translation を分離する

- `revy-server-config`
  validated config と reload planning のみに寄せる
- runtime / bootstrap
  plugin-host bootstrap / selection config への translation を持つ

`revy-server-config` は `mc-plugin-host` internal に依存しない。

## Target Crate Graph

```text
apps/revy-server
  -> revy-server-runtime
     -> revy-server-config
     -> revy-server-types
     -> mc-plugin-host
     -> revy-voxel-core
        -> revy-voxel-semantic
           -> revy-core

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

## Dependency Rules

次の rule を boundary redesign の guardrail として固定します。

- no crate outside runtime/core engine depends on `revy-voxel-core` or `revy-core` as a shared public contract proxy
- no storage crate depends on versioned protocol crate
- no config crate depends on `mc-plugin-host`
- no duplicated canonical admin/reload DTO definitions across `revy-server-config` / `revy-server-runtime` / `mc-plugin-api` / `mc-plugin-host`

最初の rule は、最終的には `ServerCore` / `GameplayTransaction` を public surface から消すことが目的です。migration 中は crate-level dependency check で proxy を張り、`revy-voxel-semantic` 導入後に tighten します。

## Migration Order

1. docs / ADR / boundary check を入れて、current debt を explicit allowlist として固定する
2. `revy-voxel-semantic` を導入し、shared semantic type を移す
3. `mc-storage-common` と `mc-storage-je-anvil-1_7_10` を導入し、protocol/storage を切り離す
4. `revy-server-types` を導入し、admin / reload DTO を寄せる
5. plugin-host translation を runtime 側へ移し、`revy-server-config` を validated config に閉じる

## Boundary Check

`cargo run -p xtask -- check-boundaries` を boundary guardrail の入口とする。

- `cargo metadata` から direct workspace dependency を読み、forbidden edge を検出する
- current debt は `tools/xtask/boundary-check.toml` の explicit allowlist に固定する
- canonical DTO duplication は tracked symbol set と expected owner path の一致で検出する
- allowlist に無い新規 drift は CI failure とする

この check は「いま全部 clean である」ことを前提にしない。代わりに、いまある違反を repo に明示し、それ以上広げないための fence として使う。

## Phase 0 Baseline

2026-03-29 に `cargo test -p revy-server-runtime --lib --quiet` を再実行した local baseline では、次の 5 件が failure した。

- `runtime::tests::gameplay::container_windows::world_backed_crafting_table_opens_and_crafts_chest_via_protocol`
- `runtime::tests::gameplay::furnace::world_backed_furnace_opens_smelts_and_closes_via_protocol`
- `runtime::tests::gameplay::furnace::world_backed_furnace_output_persists_across_restart`
- `runtime::tests::gameplay::world_chest::world_backed_chest_place_open_and_persist_across_restart`
- `runtime::tests::gameplay::world_chest::world_backed_chest_syncs_slot_updates_to_other_viewers`

boundary redesign の code motion は、この baseline を green に戻すか、明示的に quarantine してから始める。
