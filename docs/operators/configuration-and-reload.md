# 設定と reload 運用

この文書は、`runtime/server.toml` の解釈、relative path 解決、`reload runtime <mode>`、admin surface の正本です。ここで扱う reload は現在の operator surface である `reload runtime artifacts / topology / core / full` であり、旧 `reload plugins` / `reload generation` / `reload config` は扱いません。package / 起動 / release bundle の入口は [`getting-started.md`](getting-started.md) を参照してください。

`core` migration の内部設計は contributor 向けの [`../contributors/core-reload-runtime-design.md`](../contributors/core-reload-runtime-design.md) を参照してください。

## config file の選ばれ方

`server-bootstrap` は次の順で config path を決めます。

1. `REVY_SERVER_CONFIG`
2. `runtime/server.toml`

選ばれた path が存在しない場合は fail-fast で boot error になります。`ServerConfig::default()` への fallback は行いません。manual reload / watch reload が config を再読込するときも同じで、選ばれた path が無ければ reload error になります。

`package-plugins` と `build-release-bundles` は別の既定 path を持ちます。そちらは [`getting-started.md`](getting-started.md) を参照してください。

## relative path 解決

TOML file が存在して読み込まれる場合、relative path はその TOML file の親 directory を基準に解決されます。現在の実装でこの挙動になる主な key は次です。

- `static.bootstrap.world_dir`
- `static.plugins.plugins_dir`
- `live.admin.surfaces.<instance>.config`

補足:

- `static.bootstrap.world_dir` を省略すると、`<config-dir>/<level_name>` が使われます。`level_name` の既定値は `"world"` です。
- `static.plugins.plugins_dir` を省略すると、`<config-dir>/plugins` が使われます。
- `live.admin.surfaces.<instance>.config` を指定した場合、その path は存在する file へ解決される必要があります。file の中身は host では解釈されず、surface plugin にそのまま渡されます。

## 設定セクションと restart 境界

| セクション | 主な役割 | 反映方法 |
| --- | --- | --- |
| `static.bootstrap.online_mode` / `level_type` / `world_dir` / `storage_profile` | auth mode、world layout、world data path、storage profile | restart-required |
| `static.bootstrap.level_name` / `game_mode` / `difficulty` / `view_distance` | core meta と gameplay 初期値 | `reload runtime core` または `reload runtime full` |
| `static.plugins` | plugins dir、plugin ABI range | restart-required |
| `static.admin.principals.*` | permissions | `reload runtime full` |
| `live.network.server_ip` / `server_port` / `motd` | listener bind と status 表示 | `reload runtime topology` または `reload runtime full` |
| `live.network.max_players` | listener status と core meta | `reload runtime topology` / `reload runtime core` / `reload runtime full` |
| `live.topology` | adapter 有効化、Bedrock 有効化、watch flag、drain | `reload runtime topology` または `reload runtime full` |
| `live.plugins` | allowlist、buffer limits、failure policy、watch flag | `reload runtime full` |
| `live.profiles` | auth / bedrock auth / gameplay profile selection | `reload runtime full` |
| `live.admin.surfaces.*` | admin surface selection、surface profile、plugin-owned config path | `reload runtime full` |

`static.bootstrap.level_type` は現在 `"flat"` のみ対応です。`storage_profile` は restart-required のままです。

## live selection の基本

plugin package が `runtime/plugins/` に存在しても、そのまま active になるわけではありません。runtime が使う集合は次で決まります。

- `live.plugins.allowlist`
  runtime selection の候補に入れる plugin id の集合です。
- `live.profiles.auth`
  Java Edition 側で使う auth profile です。
- `live.profiles.bedrock_auth`
  Bedrock を有効化したときに使う auth profile です。
- `live.profiles.default_gameplay`
  adapter ごとの明示 map が無いときの既定 gameplay profile です。
- `live.profiles.gameplay_map`
  adapter ごとに gameplay profile を上書きします。
- `live.admin.surfaces.<instance>`
  0 個以上の admin surface instance を有効化します。`profile` は surface plugin profile、`config` は plugin-owned config path です。
- `static.bootstrap.storage_profile`
  storage profile です。`static` なので restart-required です。

JE online auth や Bedrock XBL を有効化したいときは allowlist と profile selection の両方を揃えます。

- JE online auth
  `static.bootstrap.online_mode = true`
  `live.profiles.auth = "mojang-online-v1"`
  allowlist に `auth-mojang-online` を追加
- Bedrock XBL
  `live.profiles.bedrock_auth = "bedrock-xbl-v1"`
  allowlist に `auth-bedrock-xbl` を追加

## manual reload の使い分け

### `reload runtime artifacts`

active selection を固定したまま、managed plugin の artifact 差分だけを見て reload します。

見ないもの:

- allowlist の変更
- auth / gameplay / admin surface selection の変更
- `live.network.*`
- `live.topology.*`
- `live.plugins.buffer_limits.*`
- `live.plugins.failure_policy.*`
- admin principal の permission 変更
- surface plugin config path の変更

「同じ selection / topology / core のまま、新しい shared library だけを差し替えたい」ときの操作です。

### `reload runtime topology`

最新 config を読みますが、active config に反映するのは `network` と `topology` だけです。現在の plugin selection、allowlist、profile map、buffer limit、failure policy、admin principal map、core state は維持します。

主に反映される key:

- `live.network.server_ip`
- `live.network.server_port`
- `live.network.motd`
- `live.network.max_players`
- `live.topology.be_enabled`
- `live.topology.default_adapter`
- `live.topology.enabled_adapters`
- `live.topology.default_bedrock_adapter`
- `live.topology.enabled_bedrock_adapters`
- `live.topology.reload_watch`
- `live.topology.drain_grace_secs`

reload 後は新規接続が新 generation に入り、旧 generation の session は `drain_grace_secs` のあいだ継続します。

### `reload runtime core`

最新 config を読み、`ServerCore` 向けに投影される差分だけを反映しつつ core migration を行います。selection / topology / admin surface は変えません。

期待する性質:

- play 中の接続は切れない
- `player_id` / `entity_id` は維持される
- open window、`cursor`、keepalive、dropped item、active mining、view/chunk state を維持する
- `LoginAccepted` は再送しない

主に反映される key:

- `static.bootstrap.level_name`
- `static.bootstrap.game_mode`
- `static.bootstrap.difficulty`
- `static.bootstrap.view_distance`
- `live.network.max_players`

反映しないもの:

- `static.bootstrap.online_mode`
- `static.bootstrap.level_type`
- `static.bootstrap.world_dir`
- `static.bootstrap.storage_profile`
- `static.plugins.*`
- `live.network.server_ip`
- `live.network.server_port`
- `live.network.motd`
- `live.topology.*`
- `live.plugins.*`
- `live.profiles.*`
- `static.admin.principals.*`
- `live.admin.surfaces.*`

つまり `reload runtime core` は「core-only swap」ではなく、「config 再読込 + core config への投影 + live state migration」です。

### `reload runtime full`

最新 config を読み、restart-required な差分が無いことを確認したうえで runtime selection、topology generation、core migration をまとめて再評価します。

反映対象:

- allowlist
- auth / bedrock auth / gameplay / admin surface selection
- buffer limits
- failure policy
- admin principal の permission
- `static.bootstrap.level_name`
- `static.bootstrap.game_mode`
- `static.bootstrap.difficulty`
- `static.bootstrap.view_distance`
- `live.network.max_players`
- `live.network.*`
- `live.topology.*`
- core runtime state migration

`full` は単なる config reload ではなく、artifact / topology / core をひとまとめにした reload mode です。途中で core migration が失敗した場合は、selection / topology も commit しません。

## watch reload

`live.plugins.reload_watch = true` または `live.topology.reload_watch = true` が有効な場合、runtime loop は定期的に config source を読み直し、`reload runtime full` 相当の処理を試みます。

watch reload の重要な点は次の 2 つです。

- artifact 差分だけではなく selection / topology / core migration をまとめて再評価します。
- loaded config か active config のどちらかで watch flag が有効なら、次回の watch tick が継続されます。

custom boot path で reload host を持たない supervisor を作る場合、watch flag は使えません。`server-bootstrap` から通常起動する限りは reload-capable な boot になります。

## failure policy

kind ごとの既定値は次です。

- `protocol = quarantine`
- `gameplay = quarantine`
- `storage = fail-fast`
- `auth = skip`
- `admin_surface = skip`

許可される action は kind ごとに違います。

- protocol / gameplay / admin-surface
  `quarantine` / `skip` / `fail-fast`
- storage / auth
  `skip` / `fail-fast`

運用上の読み方:

- `skip`
  壊れた候補だけ見送り、旧世代を維持します。
- `quarantine`
  壊れた artifact や active plugin を隔離し、同じ失敗を繰り返しにくくします。
- `fail-fast`
  runtime 全体の重大障害として扱い、graceful stop へ入ります。

`core` migration failure は plugin kind の failure policy ではなく rollback-first の runtime policy で扱います。通常の candidate failure では旧 core を維持し、rollback 不可能な不整合だけを fail-fast 条件とします。

## admin surfaces

### console surface

`[live.admin.surfaces.console]` のように `console-v1` surface を有効化すると、console surface plugin が stdio resource を取得して line-oriented operator loop を起動します。command の parse / render は plugin-owned です。

sample の `console-v1` surface で使える command は次です。

- `help`
- `status`
- `sessions`
- `reload runtime artifacts`
- `reload runtime topology`
- `reload runtime core`
- `reload runtime full`
- `upgrade runtime executable <path>`
- `shutdown`

permission は `static.admin.principals."console:<instance>"` で制御します。`console` instance なら principal id は `console:console` です。

### gRPC surface

`[live.admin.surfaces.<instance>]` で `profile = "grpc-v1"` を指定すると、gRPC admin surface plugin が unary gRPC control plane を起動します。host が理解するのは surface profile と opaque config path だけで、token file や bind policy は plugin-owned surface config に閉じます。

- transport は plaintext h2 のみです。
- bind policy は plugin-owned config の `bind_addr` / `allow_non_loopback` で決まります。
- `static.admin.principals.<id>` が host 側 permission policy です。
- `principals.<id>.token_file` は gRPC surface config file 基準で relative 解決されます。
- token は trim した結果が non-empty である必要があります。
- principal 間で同じ token を使うことはできません。
- TLS と public exposure は reverse proxy / ingress 側で扱う前提です。

`reload runtime full` 後は admin surface selection、surface config path、principal permission が次の request から新設定へ切り替わります。

## stdin EOF と終了条件

console surface の stdin EOF は、その console surface の入力 loop を閉じるだけです。server process 自体は継続し、surface 0 件の headless 状態も許可されます。

- stdin EOF
  console surface の入力 loop だけが終了します。
- `Ctrl-C`
  常に shutdown を要求します。
