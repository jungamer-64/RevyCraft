# reload の意味論と plugin 境界

この文書は、RevyCraft の reload が内部でどう成立しているかを contributors 向けに整理した正本です。operator 向けの command 説明ではなく、selection / topology / consistency gate / failure policy の観点でまとめます。

## 公開 reload surface

外向けの入口は `ServerSupervisor` です。

- `reload_plugins()`
- `reload_generation()`
- `reload_config()`

admin surface からは `reload plugins`、`reload generation`、`reload config` として見えますが、実装上の本体は [`../../crates/runtime/server-runtime/src/runtime/core_loop/reload.rs`](../../crates/runtime/server-runtime/src/runtime/core_loop/reload.rs) にあります。

## reload の前提

reload は reload-capable supervisor boot が必要です。`server-bootstrap` の通常起動では plugin host を渡して boot するので reload 可能ですが、reload host を持たない boot path では manual reload も watch reload も使えません。

`plugins.reload_watch` や `topology.reload_watch` は、reload host なしの builder では config error になります。

## scope ごとの内部動作

### `reload plugins`

`reload plugins` は config source を再読込しません。現在の selection config をそのまま使い、write consistency lock を取ったうえで `reconcile_runtime_selection(...)` を実行します。

流れ:

1. write consistency lock を取得
2. live protocol / gameplay session snapshot と world snapshot から `RuntimeReloadContext` を作る
3. 現在の selection config を使って plugin host を reconcile する
4. candidate `LoadedPluginSet` から `ResolvedRuntimeSelection` を再構築する
5. selection を差し替える

generation swap は行わず、config の読み直しもしません。

### `reload generation`

`reload generation` は最新 config を load しますが、active config にコピーするのは `network` と `topology` だけです。

流れ:

1. restart-required な static 差分が無いことを確認
2. 現在の config を clone
3. loaded config から `network` と `topology` だけ差し替える
4. candidate topology generation を materialize する
5. generation を activate し、selection 側には `network` / `topology` だけ反映する

allowlist、profile selection、buffer limit、failure policy、admin principal map は current state を維持します。

### `reload config`

`reload config` は selection と topology をまとめて更新します。

流れ:

1. write consistency lock を取得
2. restart-required な static 差分が無いことを確認
3. loaded config をもとに plugin host の runtime selection を reconcile
4. candidate `ResolvedRuntimeSelection` を構築
5. candidate topology generation を materialize
6. generation reload 成功後に selection を差し替える

candidate selection の構築か generation reload のどちらかで失敗した場合、plugin host には best-effort で previous selection を戻します。

## consistency gate

reload の中心にあるのが `ReloadCoordinator` の `consistency_gate` です。これは async `RwLock<()>` で、次の目的に使います。

- session spawn や event dispatch 側は read lock を取る
- reload 側は write lock を取る

結果として次が成り立ちます。

- in-flight の整合性 reader がいるあいだ manual reload / watch reload は待機する
- reload が write lock を持っているあいだ、新しい session command の進行は止まる

この性質は `runtime/tests/reload/protocol.rs` でも直接検証されています。

## watch reload

watch reload は `reload config` の簡略版ではなく、実質的に config-scoped reload と同じ意味を持ちます。artifact 差分だけではなく selection と topology を再評価します。

また、loaded config か active config のどちらかで watch flag が有効なら watch tick は継続されます。これは「watch を off にした変更」も次回 tick で観測できるようにするためです。

## rollback しきらない理由

reload は全体として transactional rollback ではありません。

- plugin host 側の reconcile と topology generation swap は別段階
- generation swap 後に selection replace が走る
- failure policy によっては pending fatal が残る

したがって「途中まで成功した変更をすべて元に戻す」保証はありません。実装は failure 時に previous selection の restore を試みますが、これは best-effort です。

## failure policy の内部的な意味

既定値は次です。

- protocol = `quarantine`
- gameplay = `quarantine`
- storage = `fail-fast`
- auth = `skip`
- admin-ui = `skip`

許可される action は kind ごとに違います。

- protocol / gameplay / admin-ui
  `quarantine` / `skip` / `fail-fast`
- storage / auth
  `skip` / `fail-fast`

読み方:

- `skip`
  candidate failure を見送り、旧世代を維持する
- `quarantine`
  壊れた candidate artifact や active plugin を隔離する
- `fail-fast`
  runtime 全体の重大障害として扱い、pending fatal や graceful stop につなぐ

## generation と plugin generation

runtime には少なくとも 2 種類の世代があります。

- topology generation
  listener と routing の世代です。new connection がどの listener / adapter に入るかを決めます。
- plugin generation
  protocol / gameplay / storage / auth / admin-ui plugin 側の世代です。

session status に両方が出るため、reload 後に「どの接続がどの世代へ pin されているか」を追えます。

## protocol と gameplay の境界

reload を読むときに重要なのは、protocol と gameplay が近いようで責務が違うことです。

- protocol
  wire format、routing、transport 固有 session state、session transfer blob を持つ
- gameplay
  semantic `GameplayCommand` を評価し、callback 単位の `GameplayTransaction` を commit する

この分離のおかげで、version 固有 inventory state や active window のようなものは protocol reload 側で扱い、semantic rule の差分は gameplay reload 側で扱えます。

## 読む順番

1. [`../../crates/runtime/server-runtime/src/runtime/reload_coordinator.rs`](../../crates/runtime/server-runtime/src/runtime/reload_coordinator.rs)
2. [`../../crates/runtime/server-runtime/src/runtime/core_loop/reload.rs`](../../crates/runtime/server-runtime/src/runtime/core_loop/reload.rs)
3. [`../../crates/runtime/server-runtime/src/runtime/topology_manager.rs`](../../crates/runtime/server-runtime/src/runtime/topology_manager.rs)
4. [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)
