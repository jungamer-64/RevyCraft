# reload の意味論と plugin 境界

## 概要

この文書は、RevyCraft の reload 振る舞いと、`protocol` / `gameplay` plugin の責務境界を contributors 向けに整理したものです。reload scope、failure policy、kind 間の相互影響を参照しやすい形でまとめます。

## 対象読者

- reload 周りの設計意図を確認したい contributors
- `protocol` と `gameplay` の責務分離を見直したい人

## この文書でわかること

- `reload_plugins()` / `reload_config()` / `reload_generation()` の違い
- partial failure が rollback しない理由
- `skip` / `quarantine` / `fail-fast` の実務上の読み方
- `protocol` と `gameplay` がどこで結合し、どこで独立しているか

## 関連資料

- [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)
- [`../operators/configuration-and-reload.md`](../operators/configuration-and-reload.md)
- [`../plugin-authors/plugin-model.md`](../plugin-authors/plugin-model.md)

## 先に押さえる結論

- `reload plugins` は差分 artifact reload です。現在の selection config を固定したまま、更新済み plugin だけを差し替えます。
- `reload config` は live config を読み直し、plugin selection と topology generation を必要に応じて更新します。
- `reload generation` は主に network / topology の変更を反映します。selection config は current state を維持します。
- reload は transactional rollback ではありません。途中まで成功した generation swap を巻き戻さずに失敗することがあります。

## 公開 reload surface

runtime の外向け入口は `ServerSupervisor` です。

- `ServerSupervisor::reload_plugins()`
- `ServerSupervisor::reload_config()`
- `ServerSupervisor::reload_generation()`

admin UI からは次の operator command として見えます。

- `reload plugins`
- `reload config`
- `reload generation`

## `reload plugins` の意味

### 基本挙動

`reload plugins` は「全 plugin を無条件に再初期化する」操作ではありません。現在 managed な plugin を順に見て、artifact が更新されているものだけを候補にします。

各 managed plugin について行うことは次です。

1. manifest を再読込する
2. `modified_at` を確認する
3. 以前に load した世代より新しい artifact だけを候補にする
4. kind ごとの migration 条件を満たしたときだけ generation を swap する

返り値は、実際に generation swap した plugin id の一覧です。

### `reload plugins` が見ないもの

`reload plugins` は selection config を固定したまま動くので、次の差分は見ません。

- allowlist の変更
- auth / gameplay / admin-ui profile の切替
- gameplay profile map の変更
- network / topology の変更
- listener / port / MOTD の変更

新しい plugin id を追加したいときや、profile を切り替えたいときは `reload config` が必要です。

## `reload config` と `reload generation` の違い

### `reload config`

`reload config` は config source を再読込し、plugin host に runtime selection を再評価させたうえで、candidate topology generation も更新します。allowlist、failure policy、auth profile、gameplay profile map、admin principal map の変更はここで反映されます。

ただし restart-required な変更は拒否されます。現行実装では次が reload 不可です。

- `static.bootstrap` 全体
- `static.plugins` 全体
- `static.admin.grpc.enabled`
- `static.admin.grpc.bind_addr`
- `static.admin.grpc.allow_non_loopback`

補足として、`static.admin.grpc.principals.<id>.token_file` と `permissions` は reload で更新されます。transport だけが restart-required です。

### `reload generation`

`reload generation` は config source から最新値を読みますが、更新対象は `network` と `topology` に限定されます。plugin selection、allowlist、profile 選択は現行 state を維持します。

主に次の変更を live 反映したいときに使います。

- `live.network.server_ip`
- `live.network.server_port`
- `live.network.motd`
- `live.network.max_players`
- `live.topology.be_enabled`
- `live.topology.default_adapter`
- `live.topology.enabled_adapters`
- `live.topology.default_bedrock_adapter`
- `live.topology.enabled_bedrock_adapters`
- `live.topology.drain_grace_secs`

## watch reload

`live.plugins.reload_watch = true` または `live.topology.reload_watch = true` を有効にすると、runtime loop が定期的に config source を読み直し、`reload_config` 相当の処理を試みます。

watch mode の重要な点は、artifact 差分だけではなく config と topology を含めて再評価することです。sample config では両方とも `false` なので、既定では手動 reload 前提です。

## partial failure semantics

### all-or-nothing ではない

reload は transactional rollback ではありません。kind ごとに順番に処理されるため、前半の plugin が reload 済みの状態で後半が失敗することがあります。`fail-fast` が混ざると、その時点までの成功を巻き戻さずに `PluginFatal` を返すことがあります。

### failure policy の単位

failure policy は kind ごとに独立しています。

- protocol
- gameplay
- storage
- auth
- admin-ui

sample config の既定値は次のとおりです。

- protocol = `quarantine`
- gameplay = `quarantine`
- storage = `fail-fast`
- auth = `skip`
- admin-ui = `skip`

## `skip` / `quarantine` / `fail-fast`

### `skip`

`skip` は「その候補だけ見送り、旧世代を維持して継続する」方針です。

reload candidate failure 時:

- 新しい artifact は採用しない
- 旧世代を維持する
- reload 全体は継続する
- 同じ壊れた artifact は、更新時刻が進むまで再試行されにくい

runtime failure 時:

- active quarantine には入れない
- gameplay なら no-op / default effect に落としやすい
- auth や admin-ui はその request を失敗として返しやすい

### `quarantine`

`quarantine` は「壊れたものを明示的に隔離する」方針です。

reload candidate failure 時:

- 新しい artifact は採用しない
- 旧世代を維持する
- `artifact_quarantine` に記録する
- 同じ artifact は以後スキップされる

runtime failure 時:

- active plugin を `active_quarantine` に入れる
- kind ごとの degraded behavior へ切り替える

代表例:

- protocol
  quarantined error を返しやすくなります。
- gameplay
  hook を no-op として扱います。

### `fail-fast`

`fail-fast` は「その失敗を runtime 全体の重大障害として扱う」方針です。

reload candidate failure 時:

- `PluginFatal` を返して reload 呼び出し自体を失敗にする
- pending fatal を記録する

runtime failure 時:

- runtime loop が pending fatal を拾う
- graceful shutdown に入る

`storage = fail-fast` は、「world persistence を維持できないなら続行しない」という強い運用判断です。

## `protocol` と `gameplay` の近い点

- どちらも generation を持つ hot-swappable plugin です。
- どちらも capability set と generation id を持ちます。
- どちらも reload 時に live session context を見ながら migration します。
- どちらも failure policy に従って degraded behavior を選びます。

## `protocol` と `gameplay` の決定的な違い

- `protocol`
  wire format、handshake routing、status / login / play packet の decode / encode を担います。
- `gameplay`
  decode 済み `CoreCommand` を評価し、`GameplayEffect` や `CoreEvent` を生成するルール層です。

一言で言うと、`protocol` は「どう話すか」、`gameplay` は「何を起こすか」を担います。

## なぜ分けると有利か

### multi-version 対応を protocol 側へ押し込める

JE `1.7.10` / `1.8.x` / `1.12.2` の packet 差分は protocol 側が吸収し、上位の gameplay は共通の `CoreCommand` / `CoreEvent` を見れば済みます。

### adapter ごとに gameplay policy を切り替えやすい

session では adapter 確定後に gameplay profile が選ばれます。複数 adapter が同じ gameplay を共有する構成も、特定 adapter だけ別 gameplay を使う構成も取りやすいです。

### reload 単位を分けられる

protocol 側は route topology と session migration、gameplay 側は gameplay session migration を見るので、片方だけを安全に差し替える余地があります。

## 相互影響: gameplay -> protocol

runtime は protocol が decode した `CoreCommand` を受け、gameplay で `CoreEvent` を生成し、protocol が再び packet に encode します。したがって gameplay 変更は protocol 実装を直接触らなくても client-visible behavior に波及します。

例:

- block placement ルールを変える
  `BlockChanged` が出る条件やタイミングが変わります。
- inventory 拒否ルールを変える
  inventory 系 event の出方が変わります。
- readonly policy を強める
  rollback packet だけが返るような差が出ます。

`CoreEvent` を増やすと protocol codec と各 adapter 側の encode 実装も追従が必要です。JE と BE では event の扱いに差があるため、edition ごとの見え方も変わり得ます。

## 相互影響: protocol -> gameplay

protocol adapter は client packet を `CoreCommand` に decode して gameplay へ渡します。protocol 側の変更は gameplay が受け取る入力意味そのものを変えます。

例:

- `PlaceBlock` decode が face / hand / held item の扱いを変える
- `InventoryClick` decode が transaction 解釈を変える
- `MoveIntent` decode が座標や `on_ground` の扱いを変える

また、handshake routing で選ばれた adapter id は gameplay profile 解決にも使われます。default adapter や enabled adapter の変更は、間接的に「どの gameplay profile が紐づくか」を変えます。

## 実務上の確認ポイント

### gameplay 変更時

- どの `CoreEvent` が増減するか
- JE / BE 各 adapter がその event をどう encode するか
- 特定 adapter だけ別 gameplay profile を使っていないか
- reload migration で session を維持できるか

### protocol 変更時

- `CoreCommand` の意味が変わっていないか
- handshake routing 変更で gameplay profile 選択が変わらないか
- encode / decode 変更で client-visible behavior が変わらないか
- protocol reload compatibility を壊していないか

## sample config の読み方

現在の sample config は次の方針です。

- protocol / gameplay は `quarantine`
- storage は `fail-fast`
- auth / admin-ui は `skip`

これは概ね次のように読めます。

- 通信と gameplay ルールは、壊れた候補や壊れた active plugin を隔離して続ける
- storage は、整合性を保てないなら続行しない
- auth / admin-ui は、request 単位で見送ったり失敗を返したりしやすい

## まとめ

- `reload plugins` は差分 artifact reload で、selection config 変更までは見ません。
- `reload config` は selection と topology generation をまとめて再評価します。
- `reload generation` は network / topology だけを live 反映します。
- partial failure は rollback せず、kind ごとの failure policy で意味が決まります。
- `protocol` と `gameplay` は host abstraction 上は近いですが、責務は明確に異なります。
