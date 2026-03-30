# `CoreCommand` から `CoreEvent` までの流れ

- 対象読者: play 処理と login 処理で `CoreCommand`、`GameplayCommand`、`GameplayEffectBatch`、`CoreEvent` の流れを追いたい contributors
- この文書で扱う範囲: 型の役割、runtime 側の分岐、login special-case、event dispatch までの流れ
- この文書で扱わないこと: reload coordinator の実装、boundary redesign の crate graph、operator 向け command 運用
- 次に読む文書: [`core-reload-runtime-design.md`](core-reload-runtime-design.md)

この文書は単独で読む詳細編です。`runtime` / plugin host の全体像は [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md) を先に読むと追いやすくなります。

## 一枚で見る流れ

```text
client packet
  -> protocol plugin decode
  -> CoreCommand
  -> runtime dispatch
     -> CoreCommand::LoginStart
        -> GameplayLoginPreview::new(...)
        -> gameplay plugin HandlePlayerJoin
        -> validate_and_apply_login_effects(...)
        -> Vec<TargetedEvent>
     -> direct-core command
        -> ServerCore::apply_command(...)
        -> Vec<TargetedEvent>
     -> gameplay-owned command
        -> GameplayCommand
        -> snapshot-backed GameplayReadView + detached GameplayEffectBatch
        -> gameplay plugin callback
        -> validate_and_apply_gameplay_effects(...)
        -> Vec<TargetedEvent>
  -> TargetedEvent dispatch
  -> protocol plugin encode
  -> wire packets
```

### login special-case の補足

```text
LoginStart accepted
  -> connection-targeted LoginAccepted を queue
  -> session task が login success packet を write
  -> player_id / entity_id / phase / session_capabilities を commit
  -> pending login route を閉じて Play session へ進める
```

login は gameplay transaction の special-case ですが、`LoginAccepted` を emit した瞬間に shared session state が `Play` へ進むわけではありません。session task が login success packet を write した時点で commit されるため、この短い window だけ `SessionRegistry` の pending login route が `EventTarget::Player` 配送を bridge します。

## 型の役割

- `CoreCommand`
  runtime / protocol 境界で使う semantic input です。定義は [`../../crates/core/revy-voxel-semantic/src/events.rs`](../../crates/core/revy-voxel-semantic/src/events.rs) にあります。
- `GameplayCommand`
  gameplay plugin に見せる gameplay-owned command だけを抜き出した入力です。`CoreCommand` から分離されます。定義は [`../../crates/core/revy-voxel-semantic/src/events.rs`](../../crates/core/revy-voxel-semantic/src/events.rs) にあります。
- `GameplayEffectBatch`
  gameplay callback 単位で host が返す invocation-scoped result です。plugin は read callback と effect recorder を通じて snapshot を読み、`read-set + effect list` を batch に積みます。runtime は live core を直接触らせず、この batch を `ServerCore::validate_and_apply_*` へ渡して validate/apply します。定義は [`../../crates/core/revy-voxel-semantic/src/gameplay.rs`](../../crates/core/revy-voxel-semantic/src/gameplay.rs)、apply 側は [`../../crates/core/revy-voxel-core/src/core/transaction.rs`](../../crates/core/revy-voxel-core/src/core/transaction.rs) にあります。
- `CoreEvent`
  core から外へ出る出力です。最終的に protocol plugin が encode します。定義は [`../../crates/core/revy-voxel-semantic/src/events.rs`](../../crates/core/revy-voxel-semantic/src/events.rs) にあります。
- `TargetedEvent`
  `CoreEvent` に配送先を付けた wrapper です。routing primitive 自体は `revy-core` にあり、`revy-voxel-core` は `TargetedEvent = RoutedEvent<CoreEvent>` として re-export します。runtime はこれを session / connection / broadcast へ dispatch します。

## command の分岐点

runtime 側の本体は [`../../crates/runtime/revy-server-runtime/src/runtime/kernel.rs`](../../crates/runtime/revy-server-runtime/src/runtime/kernel.rs) にあります。現在の分岐は大きく 3 種類です。

### login special-case

`CoreCommand::LoginStart` は gameplay profile があれば、runtime kernel が先に `GameplayLoginPreview::new(...)` で login prelude を作り、reject をここで short-circuit します。success のときだけ preview-backed `GameplayReadView` を `prepare_player_join(...)` へ渡し、戻ってきた `GameplayEffectBatch` を `validate_and_apply_login_effects(...)` で live core へ commit します。host は detached read/effect batch を返すだけで、`begin_login(...)` / `finalize_login(...)` の owner ではありません。実装は [`../../crates/runtime/revy-server-runtime/src/runtime/kernel.rs`](../../crates/runtime/revy-server-runtime/src/runtime/kernel.rs)、[`../../crates/plugin/mc-plugin-host/src/host/profiles/gameplay.rs`](../../crates/plugin/mc-plugin-host/src/host/profiles/gameplay.rs)、[`../../crates/core/revy-voxel-core/src/core/transaction.rs`](../../crates/core/revy-voxel-core/src/core/transaction.rs) にあります。

### direct-core command

次の command は gameplay policy を通らず、core が直接処理します。

- `UpdateClientView`
- `ClientStatus`
- `InventoryTransactionAck`
- `InventoryClick`
- `CloseContainer`
- `KeepAliveResponse`
- `Disconnect`

特に `InventoryClick` は [`../../crates/core/revy-voxel-core/src/core/inventory/click.rs`](../../crates/core/revy-voxel-core/src/core/inventory/click.rs) で直接処理されます。gameplay plugin transaction は経由しません。

### gameplay-owned command

次の command は gameplay policy へ渡されます。

- `MoveIntent`
- `SetHeldSlot`
- `CreativeInventorySet`
- `DigBlock`
- `PlaceBlock`
- `UseBlock`

ここで runtime が `CoreCommand` を `GameplayCommand` へ落とし、gameplay plugin の `prepare_command(...)` を snapshot-backed read view 上で 1 回だけ実行します。plugin は host effect API を通じて detached `GameplayEffectBatch` を組み立て、runtime はその batch を live core に対して validate/apply します。read-set が stale なら callback は再実行せず、結果を authoritative resync / drop に寄せます。

## login 時に何が足されるか

`validate_and_apply_login_effects(...)` は gameplay callback が成功したあと、runtime bootstrap に必要な event をまとめて積みます。

- `LoginAccepted`
- `PlayBootstrap`
- `ChunkBatch`
- `InventoryContents`
- `SelectedHotbarSlotChanged`
- 既存 player の spawn event

この順番を追いたいときは [`../../crates/core/revy-voxel-core/src/core/transaction.rs`](../../crates/core/revy-voxel-core/src/core/transaction.rs) を読むのが最短です。

ただし `LoginAccepted` は core の accept point であって、その場で shared session state を `Play` へ進めるわけではありません。runtime は `LoginAccepted` をまず connection-targeted event として queue に積み、session task が login success packet を write できた時点で `player_id / entity_id / phase / session_capabilities` を commit します。commit 前の短い window は `SessionRegistry` の pending login route が `EventTarget::Player` 配送だけを bridge します。

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

reload 観点の責務境界は [`core-reload-runtime-design.md`](core-reload-runtime-design.md) を参照してください。
