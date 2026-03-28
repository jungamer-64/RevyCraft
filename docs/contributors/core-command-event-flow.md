# `CoreCommand` / `GameplayCommand` / `GameplayTransaction` / `CoreEvent` の流れ

この文書は、play 処理と login 処理で `CoreCommand`、`GameplayCommand`、`GameplayTransaction`、`CoreEvent` がどうつながるかを見るための正本です。

## 一枚で見る流れ

```text
client packet
  -> protocol plugin decode
  -> CoreCommand
  -> runtime dispatch
     -> direct-core command
        -> ServerCore::apply_command(...)
        -> Vec<TargetedEvent>
     -> gameplay-owned command
        -> GameplayCommand
        -> snapshot clone + detached GameplayTransaction
        -> gameplay plugin callback
        -> validate_and_apply_gameplay_journal(...)
        -> Vec<TargetedEvent>
  -> runtime dispatch
  -> protocol plugin encode
  -> wire packets
```

login は gameplay transaction の special-case です。

```text
login/auth flow
  -> CoreCommand::LoginStart
  -> GameplayTransaction::begin_login(...)
  -> gameplay plugin HandlePlayerJoin
  -> GameplayTransaction::finalize_login(...)
  -> detached journal validate/apply
  -> CoreEvent 群
```

## 型の役割

- `CoreCommand`
  runtime / protocol 境界で使う semantic input です。定義は [`../../crates/core/mc-core/src/events.rs`](../../crates/core/mc-core/src/events.rs) にあります。
- `GameplayCommand`
  gameplay plugin に見せる gameplay-owned command だけを抜き出した入力です。`CoreCommand` から分離されます。定義は [`../../crates/core/mc-core/src/events.rs`](../../crates/core/mc-core/src/events.rs) にあります。
- `GameplayTransaction`
  gameplay callback 単位で host が開始する invocation-scoped transaction です。plugin はここを通じて world / player / inventory / block を読み書きします。runtime は live core を直接触らず、snapshot を読みながら `read-set + op journal` を積み、最後に live core へ validate/apply します。定義は [`../../crates/core/mc-core/src/core/transaction.rs`](../../crates/core/mc-core/src/core/transaction.rs) にあります。
- `CoreEvent`
  core から外へ出る出力です。最終的に protocol plugin が encode します。定義は [`../../crates/core/mc-core/src/events.rs`](../../crates/core/mc-core/src/events.rs) にあります。
- `TargetedEvent`
  `CoreEvent` に配送先を付けた wrapper です。runtime はこれを session / connection / broadcast へ dispatch します。

## command の分岐点

runtime 側の本体は [`../../crates/runtime/revy-server-runtime/src/runtime/kernel.rs`](../../crates/runtime/revy-server-runtime/src/runtime/kernel.rs) にあります。現在の分岐は大きく 3 種類です。

### login special-case

`CoreCommand::LoginStart` は gameplay profile があれば `prepare_player_join(...)` へ入り、transaction の `begin_login(...)` / `finalize_login(...)` を経由して detached journal を作ります。runtime はその journal を live core へ validate/apply します。実装は [`../../crates/plugin/mc-plugin-host/src/host/profiles/gameplay.rs`](../../crates/plugin/mc-plugin-host/src/host/profiles/gameplay.rs) と [`../../crates/core/mc-core/src/core/transaction.rs`](../../crates/core/mc-core/src/core/transaction.rs) にあります。

### direct-core command

次の command は gameplay policy を通らず、core が直接処理します。

- `UpdateClientView`
- `ClientStatus`
- `InventoryTransactionAck`
- `InventoryClick`
- `CloseContainer`
- `KeepAliveResponse`
- `Disconnect`

特に `InventoryClick` は [`../../crates/core/mc-core/src/core/inventory/click.rs`](../../crates/core/mc-core/src/core/inventory/click.rs) で直接処理されます。gameplay plugin transaction は経由しません。

### gameplay-owned command

次の command は gameplay policy へ渡されます。

- `MoveIntent`
- `SetHeldSlot`
- `CreativeInventorySet`
- `DigBlock`
- `PlaceBlock`
- `UseBlock`

ここで runtime が `CoreCommand` を `GameplayCommand` へ落とし、gameplay plugin の `prepare_command(...)` を detached transaction 上で 1 回だけ実行します。plugin は host mutation API を通じて draft state を更新し、runtime はその journal を live core に対して validate/apply します。read-set が stale なら callback は再実行せず、結果を authoritative resync/drop に寄せます。

## login 時に何が足されるか

`GameplayTransaction::finalize_login(...)` は gameplay callback が成功したあと、runtime bootstrap に必要な event をまとめて積みます。

- `LoginAccepted`
- `PlayBootstrap`
- `ChunkBatch`
- `InventoryContents`
- `SelectedHotbarSlotChanged`
- 既存 player の spawn event

この順番を追いたいときは [`../../crates/core/mc-core/src/core/transaction.rs`](../../crates/core/mc-core/src/core/transaction.rs) を読むのが最短です。

ただし `LoginAccepted` は core の accept pointであって、その場で shared session state を `Play`
へ進めるわけではありません。runtime は `LoginAccepted` をまず connection-targeted event として
queue に積み、session task が login success packet を write できた時点で
`player_id / entity_id / phase / session_capabilities` を commit します。commit 前の短い window は
`SessionRegistry` の pending login route が `EventTarget::Player` 配送だけを bridge します。

## runtime 側の受け渡し

core の前後で見るべき runtime 側の入口は次です。

- play packet の decode
  [`../../crates/runtime/revy-server-runtime/src/runtime/session/play.rs`](../../crates/runtime/revy-server-runtime/src/runtime/session/play.rs)
- command の適用と event dispatch
  [`../../crates/runtime/revy-server-runtime/src/runtime/core_loop/events.rs`](../../crates/runtime/revy-server-runtime/src/runtime/core_loop/events.rs)
- outgoing packet の encode
  [`../../crates/runtime/revy-server-runtime/src/runtime/session/outgoing.rs`](../../crates/runtime/revy-server-runtime/src/runtime/session/outgoing.rs)

## この分割で守っていること

- raw slot や version ごとの inventory quirks は protocol plugin が吸収する
- core が受け取るのは semantic な `CoreCommand`
- gameplay plugin は `GameplayCommand` と host transaction API だけに集中する
- gameplay callback は invocation 開始時点の snapshot を読み、runtime は callback を再実行しない
- runtime は dispatch と session orchestration に徹する

protocol / gameplay の責務境界を reload 観点で見たいときは [`reload-semantics-and-boundaries.md`](reload-semantics-and-boundaries.md) を参照してください。
