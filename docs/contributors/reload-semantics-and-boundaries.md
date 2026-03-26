# reload の意味論と reloadable boundary

この文書は、RevyCraft の reload を contributors 向けに整理した正本です。ここで扱うのは target design としての `reload runtime <mode>` であり、旧 `reload plugins` / `reload generation` / `reload config` の公開 surface は扱いません。operator 向けの command 説明は [`../operators/configuration-and-reload.md`](../operators/configuration-and-reload.md)、`core` migration の詳細設計は [`core-reload-runtime-design.md`](core-reload-runtime-design.md) を参照してください。

## 公開 reload surface

外向けの入口は `ServerSupervisor::reload_runtime(mode)` です。

- `reload runtime artifacts`
- `reload runtime topology`
- `reload runtime core`
- `reload runtime full`

admin surface、gRPC、permission もこの 4 mode を前提にそろえます。

## reload の前提

reload は reload-capable supervisor boot が必要です。`server-bootstrap` の通常起動では reload host を伴う boot path を使い、manual reload と watch reload を許可します。reload host を持たない custom boot path では manual reload も watch reload も使えません。

`plugins.reload_watch` や `topology.reload_watch` は watch trigger であり、実際に実行する処理は `reload runtime full` と同じ意味論を持ちます。

## mode ごとの内部動作

### `artifacts`

active selection を固定したまま managed plugin の artifact 差分だけを reload します。

流れ:

1. write consistency lock を取得
2. live protocol / gameplay session snapshot と `core` runtime blob を固定
3. current selection config で plugin host を reconcile
4. protocol / gameplay / storage generation を migration
5. selection を差し替え

core swap と topology generation swap は行いません。

### `topology`

最新 config を読みますが、candidate に反映するのは `network` と `topology` だけです。

流れ:

1. restart-required な static 差分が無いことを確認
2. current config を clone
3. loaded config から `network` / `topology` だけ差し替える
4. candidate topology generation を materialize
5. active generation を切り替え、旧 generation を draining へ移す

selection と core は current state を維持します。

### `core`

selection / topology / transport を変えずに `ServerCore` だけを migration します。

流れ:

1. write consistency lock を取得
2. current selection と active topology generation を固定
3. live runtime から `CoreRuntimeStateBlob` を export
4. candidate core を materialize
5. play session を candidate core へ reattach
6. protocol / gameplay へ必要な resync event を発行
7. 成功時のみ core owner を swap

失敗時は旧 core を維持し、session を切断しません。

### `full`

selection / topology / core migration を単一 transaction として扱います。

流れ:

1. write consistency lock を取得
2. loaded config から candidate selection を resolve
3. candidate topology generation を materialize
4. `CoreRuntimeStateBlob` を export して candidate core を materialize
5. plugin generation migration と session reattach を実行
6. commit 条件がそろった場合のみ selection / topology / core を一括反映

`full` は `config-scoped reload` の別名ではなく、artifact / topology / core をまとめた公開 mode です。

## consistency gate

reload の中心にあるのは `ReloadCoordinator` の `consistency_gate` です。これは async `RwLock<()>` で、次の目的に使います。

- session spawn、command dispatch、event dispatch、tick 側は read lock を取る
- reload 側は write lock を取る

結果として次が成り立ちます。

- in-flight の reader がいるあいだ manual reload / watch reload は待機する
- reload が write lock を持っているあいだ、新しい session command の進行は止まる
- `full` は selection / topology / core の commit point まで同じ write lock の中で完結する

この性質は protocol reload 系テストで検証されている性質をそのまま設計の基盤にし、`core` migration でも同じ gate を使います。

## rollback と transaction 境界

`artifacts` と `topology` は現行実装に近い best-effort reload ですが、`core` と `full` は rollback-first で扱います。

- `artifacts`
  candidate generation failure は旧 selection を維持する
- `topology`
  candidate listener / routing materialize failure は旧 generation を維持する
- `core`
  candidate core materialize failure、reattach failure、resync failure は old core 維持で rollback する
- `full`
  core migration が失敗した時点では selection / topology も commit しない

特に `full` は旧 `reload config` より transaction 性を強く持ちます。

## failure policy の意味

plugin kind ごとの failure policy は引き続き次です。

- protocol = `quarantine`
- gameplay = `quarantine`
- storage = `fail-fast`
- auth = `skip`
- admin-ui = `skip`

読み方:

- `skip`
  壊れた candidate を見送り、旧 generation を維持する
- `quarantine`
  壊れた candidate artifact や active plugin を隔離する
- `fail-fast`
  runtime 全体の重大障害として扱う

ただし `core` migration failure は plugin failure policy ではなく runtime rollback policy で扱います。既定動作は rollback-first で、rollback 不可能な不整合だけを fail-fast 条件にします。

## generation と migration の境界

runtime には少なくとも 2 種類の世代があります。

- topology generation
  listener と routing の世代
- plugin generation
  protocol / gameplay / storage / auth / admin-ui plugin の世代

`core` migration は topology generation のような別番号を持つ公開概念ではなく、live session を同一 connection / entity identity のまま新しい core owner に張り替える内部 operation として扱います。

## protocol / gameplay / core の境界

reload を読むときの責務分割は次です。

- protocol
  wire format、routing、transport 固有 session state、session transfer blob を持つ
- gameplay
  semantic `GameplayCommand` を評価し、callback 単位の `GameplayTransaction` を commit する
- core
  world / entity / inventory / keepalive / dropped item / active mining を含む canonical runtime state を持つ

`core` を reloadable boundary に出すことで、protocol 固有 session blob と gameplay 固有 session blob に加えて、world-semantic な live state も migration 対象へ入ります。

## 読む順番

1. [`core-reload-runtime-design.md`](core-reload-runtime-design.md)
2. [`../../crates/runtime/server-runtime/src/runtime/reload_coordinator.rs`](../../crates/runtime/server-runtime/src/runtime/reload_coordinator.rs)
3. [`../../crates/runtime/server-runtime/src/runtime/core_loop/reload.rs`](../../crates/runtime/server-runtime/src/runtime/core_loop/reload.rs)
4. [`../../crates/runtime/server-runtime/src/runtime/topology_manager.rs`](../../crates/runtime/server-runtime/src/runtime/topology_manager.rs)
5. [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)
