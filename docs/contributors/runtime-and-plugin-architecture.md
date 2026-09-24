# `runtime` と `plugin` の設計

- 対象読者: runtime、plugin host、transport、process handoff の責務境界を追う contributors
- この文書で扱う範囲: current architecture、authoritative state、crate dependency、plugin ABI、network ownership
- この文書で扱わないこと: operator 向け config key、cutover の詳細手順、plugin authoring のコード例
- 次に読む文書: [`core-reload-runtime-design.md`](core-reload-runtime-design.md)

RevyCraft の主要価値は、Java/TCP と Bedrock/UDP の live session を維持したまま runtime と executable を低停止時間で切り替えることです。reload を補助機能として扱わず、active state の authority、session ownership、core representation、transport ownershipをこの成立条件から決めます。

## レイヤーと責務

1. `apps/revy-server`
   process boot、admin surface supervision、executable handoff の parent/child orchestration を持ちます。
2. `crates/runtime/revy-server-runtime`
   `RuntimeEpoch` publication、listener ingress、session actor、versioned core、cutover protocol を持ちます。
3. `crates/runtime/revy-runtime-transfer`
   parent/child 間の versioned protobuf envelope、transfer identity、resource descriptor、bounded shared arena を持ちます。
4. `crates/network/revy-raknet`
   UDP receive authority、peer reliability state、ACK/NACK、retransmit、ordering、fragmentation、MTU、handshake、freeze snapshot を所有します。
5. `crates/core/revy-voxel-core`
   immutable `CoreVersion`、`CoreMutation`、semantic gameplay state と event generation を持ちます。
6. `crates/plugin/mc-plugin-host`
   packaged plugin discovery、ABI validation、generation lease、candidate selection、reload preparation を持ちます。
7. `crates/plugin/mc-plugin-contract` / `mc-plugin-abi`
   前者は safe semantic contract、後者は ABI 9 の raw FFI layout だけを持ちます。
8. `crates/plugin/mc-plugin-sdk-rust`
   Rust plugin authoring trait、manifest helper、export macro を持ちます。

`revy-voxel-semantic` は protocol / gameplay / storage が共有する semantic type の owner、`revy-server-types` は operator-facing DTO の owner です。engine internal を plugin contract の代用にしません。

## active runtime の唯一の authority

`RuntimeAuthority` が `Arc<RuntimeEpoch>` を一度に publish します。`RuntimeEpoch` は次を同じ revision の state として所有します。

- validated config と resolved plugin selection
- exact plugin artifact / generation lease
- active topology generation と admission view
- `CoreStore`
- online authentication generation

selection、topology、core は独立に commit できません。plugin host と topology resources は candidate を構築できますが、active state を変更する authority は `RuntimeAuthority` の epoch publication だけです。session actor は共有 epoch latch を購読し、commit 後の最初の data-plane 処理より前に prepared binding を activate します。

`ReloadCoordinator` が残す責務は config source、reload serialization、shutdown lifecycle です。active selection や commit state の authority ではありません。

## core の authority

`CoreStore` は `Arc<CoreVersion>` と `CoreRevision` を管理します。read path は `Arc` clone で immutable version を取得し、whole-core clone を行いません。command、tick、plugin effect は `CoreMutation` から `PreparedCoreCommit` を作り、base revision が active revision と一致した場合だけ同じ commit path で publish します。

plugin callback の read-set は取得元 revision に結び付きます。競合は stale outcome として返し、callback を暗黙に再実行しません。persistence は `persisted_revision` と最新 dirty revision を区別し、revision R の保存成功によって R より後の mutation を clean にしません。

同一 process reload の handoff は validated `Arc<CoreVersion>` capability です。executable upgrade は immutable pre-copy 後の mutation を bounded journal に保持し、freeze 中は final delta だけを seal します。journal budget を追い越した candidate は freeze 前の restage または machine-readable abort になり、freeze 中の full serialization へ劣化しません。

## session と directory projection

session actor が phase と transport state の authority です。phase は次の state machine で表現します。

- `Handshaking`
- `Status`
- `Login::{Negotiating, Authenticating, AcceptedWritePending}`
- `Play`
- `Closing`

player / entity / gameplay capability は `Play` だけが所有します。actor lifecycle は `Running`、`CutoverPrepared`、`TransferFrozen`、`Transferred` で、prepared state と active state を同居させる期間を型で限定します。

registry の player-to-connection view と executable session directory は actor transition から更新される projection です。projection は actor state から再構築可能で、独立 authority ではありません。各 actor は bounded handoff slot を事前確保し、plugin blob、read buffer、queued event、transport state を freeze 中にその slot へ seal します。

## listener と RakNet

TCP/UDP listener は topology resource として candidate bind、pause、resume、transfer が区別されます。inactive listener の bind と descriptor duplication は freeze 前に完了します。

Bedrock transport は `revy-raknet` が所有します。socket router が唯一の UDP receive authority、peer actor が各接続の reliability authority です。24-bit sequence は専用型で wraparound を扱い、ACK/NACK、retransmit、reliable ordering、sequencing、fragment reassembly、timeout を bounded state として保持します。freeze は router receive を止め、writer queue を seal し、peer snapshot を並列に確定します。外部 RakNet transport crate は production dependency にしません。

## plugin ABI 9

`mc-plugin-contract` は semantic request / response と safe enum、`mc-plugin-abi` は `repr(C)` table、raw slice、owned buffer、tag だけを所有します。manifest と function table は ABI version と `struct_size` を先頭に持ちます。

host は invocation 前に tag、function pointer、pointer/count/null、`len <= cap`、`isize::MAX`、configured buffer limit、UTF-8 を検証します。plugin-owned buffer は free callback と generation lease を持つ guard が一度だけ解放します。gameplay host authority は invocation ごとの explicit call context から渡し、ambient TLS scope を使いません。

plugin object は load ごとに host が生成・所有し、同じ artifact の別 load と可変 state を共有しません。code / immutable manifest の共有と object identity は別です。全 invocation と返却 buffer は object lease を保持し、最後の lease の解放で object を destroy してから library lease を解放します。session handoff は artifact hash が等しい場合も必要で、candidate の preparation は active object を書き換えません。

live reload に参加する protocol / gameplay plugin は `max_session_handoff_bytes` を manifest で宣言します。prepare が slot capacity を検証するため、freeze 中に plugin blob 用の再 allocation は行いません。native plugin boundary は ABI 9 だけを受理します。

## executable handoff

child は freeze 前に起動し、config、transfer protocol、plugin artifact hash、candidate epoch、shared arena、duplicated resource を検証します。parent は versioned session directory を pre-stage し、child が現在 revision を acknowledge してから freeze へ進みます。

control plane は length-limited `RuntimeTransferProtocolV1` protobuf envelope と `TransferId` を使います。大きな core / session / RakNet payload は shared arena に置き、control envelope には bounded descriptor だけを載せます。Unix は shared memory と descriptor passing、Windows は file mapping と socket duplication を使います。

`Commit` 後に結果が不明な parent は data plane を再開しません。同じ `TransferId` の status で committed / aborted を解決し、解消不能なら fail closed にして親子同時 authority を防ぎます。

## 依存方向

```text
apps/revy-server
  -> revy-server-runtime
  -> revy-runtime-transfer

revy-server-runtime
  -> revy-server-config
  -> revy-server-types
  -> mc-plugin-host
  -> revy-raknet
  -> revy-voxel-core

mc-plugin-host
  -> mc-plugin-contract
  -> mc-plugin-abi

mc-plugin-sdk-rust
  -> mc-plugin-contract
  -> mc-plugin-abi
```

storage は versioned protocol crate に依存せず、plugin host は `revy-voxel-core` を public contract proxy にしません。境界の正規 check は次です。

```bash
cargo run -p xtask -- check-boundaries
```

## 読む順番

1. [`../../crates/runtime/revy-server-runtime/src/runtime/authority.rs`](../../crates/runtime/revy-server-runtime/src/runtime/authority.rs)
2. [`../../crates/runtime/revy-server-runtime/src/runtime/cutover.rs`](../../crates/runtime/revy-server-runtime/src/runtime/cutover.rs)
3. [`../../crates/runtime/revy-server-runtime/src/runtime/core_store.rs`](../../crates/runtime/revy-server-runtime/src/runtime/core_store.rs)
4. [`../../crates/runtime/revy-server-runtime/src/runtime/session/types.rs`](../../crates/runtime/revy-server-runtime/src/runtime/session/types.rs)
5. [`../../crates/runtime/revy-server-runtime/src/runtime/executable.rs`](../../crates/runtime/revy-server-runtime/src/runtime/executable.rs)
6. [`../../crates/network/revy-raknet/src/lib.rs`](../../crates/network/revy-raknet/src/lib.rs)

cutover の測定境界と failure contract は [`core-reload-runtime-design.md`](core-reload-runtime-design.md)、command/event の semantic flow は [`core-command-event-flow.md`](core-command-event-flow.md) を参照してください。
