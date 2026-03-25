# 設定と reload 運用

この文書は、`runtime/server.toml` の解釈、relative path 解決、reload scope、admin surface の正本です。package / 起動 / release bundle の入口は [`getting-started.md`](getting-started.md) を参照してください。

## config file の選ばれ方

`server-bootstrap` は次の順で config path を決めます。

1. `REVY_SERVER_CONFIG`
2. `runtime/server.toml`

選ばれた path が存在しない場合は warning を出し、`ServerConfig::default()` を使って boot します。これは「空の TOML を読む」のではなく、構造体 default をそのまま使う挙動です。

`package-plugins` と `build-release-bundles` は別の既定 path を持ちます。そちらは [`getting-started.md`](getting-started.md) を参照してください。

## relative path 解決

TOML file が存在して読み込まれる場合、relative path はその TOML file の親 directory を基準に解決されます。現在の実装でこの挙動になる主な key は次です。

- `static.bootstrap.world_dir`
- `static.plugins.plugins_dir`
- `static.admin.grpc.principals.<id>.token_file`

補足:

- `static.bootstrap.world_dir` を省略すると、`<config-dir>/<level_name>` が使われます。`level_name` の既定値は `"world"` です。
- `static.plugins.plugins_dir` を省略すると、`<config-dir>/plugins` が使われます。
- `token_file` は non-empty token へ解決される必要があります。空 token、重複 token、空 permission list は config error です。

一方で、選ばれた config path が存在せず built-in default config で boot する場合は、relative 解決は発生しません。このときの built-in default は `runtime/world` と `runtime/plugins` を使います。

## 設定セクションと restart 境界

| セクション | 主な役割 | 反映方法 |
| --- | --- | --- |
| `static.bootstrap` | world、online mode、level type、game mode、difficulty、view distance、storage profile | restart-required |
| `static.plugins` | plugins dir、plugin ABI range | restart-required |
| `static.admin.grpc` の transport 部分 | `enabled`、`bind_addr`、`allow_non_loopback` | restart-required |
| `static.admin.grpc.principals.*` | token file、permissions | `reload config` |
| `live.network` | `server_ip`、`server_port`、`motd`、`max_players` | `reload generation` または `reload config` |
| `live.topology` | adapter 有効化、Bedrock 有効化、watch flag、drain | `reload generation` または `reload config` |
| `live.plugins` | allowlist、buffer limits、failure policy、watch flag | `reload config` |
| `live.profiles` | auth / bedrock auth / gameplay profile selection | `reload config` |
| `live.admin` | admin-ui profile、local console permissions | `reload config` |

`static.bootstrap.level_type` は現在 `"flat"` のみ対応です。`static.admin.grpc.principals.*` は TOML 上では `static` 配下ですが、transport 設定とは別に `reload config` で更新されます。

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
- `live.admin.ui_profile`
  local console の parse / render を担当する admin-ui profile です。
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

### `reload plugins`

`reload plugins` は config file を再読込しません。現在 active な selection config を固定したまま、managed plugin の artifact 差分だけを見て reload します。

見ないもの:

- allowlist の変更
- auth / gameplay / admin-ui profile の変更
- `live.network.*`
- `live.topology.*`
- `live.plugins.buffer_limits.*`
- `live.plugins.failure_policy.*`
- remote admin principal の token / permission 変更

「同じ selection のまま、新しい shared library だけを差し替えたい」ときの操作です。

### `reload generation`

`reload generation` は最新 config を読みますが、active config に反映するのは `network` と `topology` だけです。現在の plugin selection、allowlist、profile map、buffer limit、failure policy、admin principal map は維持します。

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

### `reload config`

`reload config` は最新 config を読み、restart-required な差分が無いことを確認したうえで runtime selection と topology generation をまとめて再評価します。

反映対象:

- allowlist
- auth / bedrock auth / gameplay / admin-ui profile selection
- buffer limits
- failure policy
- remote admin principal の token / permission
- `live.network.*`
- `live.topology.*`

失敗しやすい例:

- `static.bootstrap`、`static.plugins`、`static.admin.grpc` transport を変更した
- candidate selection が validation を通らない
- gameplay profile の切替が live session 条件を満たさない
- candidate topology の materialize に失敗した

## watch reload

`live.plugins.reload_watch = true` または `live.topology.reload_watch = true` が有効な場合、runtime loop は定期的に config source を読み直し、`reload config` 相当の処理を試みます。

watch reload の重要な点は次の 2 つです。

- artifact 差分だけではなく config と topology も再評価します。
- loaded config か active config のどちらかで watch flag が有効なら、次回の watch tick が継続されます。

custom boot path で reload host を持たない supervisor を作る場合、watch flag は使えません。`server-bootstrap` から通常起動する限りは reload-capable な boot になります。

## failure policy

kind ごとの既定値は次です。

- `protocol = quarantine`
- `gameplay = quarantine`
- `storage = fail-fast`
- `auth = skip`
- `admin_ui = skip`

許可される action は kind ごとに違います。

- protocol / gameplay / admin-ui
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

## admin console と gRPC

### local console

`live.admin.ui_profile` で有効な admin-ui profile が解決できた場合、stdio 上に line-oriented operator loop が起動します。line の parse / render は active admin-ui plugin に委譲されます。

sample の `console-v1` profile で使える command は次です。

- `help`
- `status`
- `sessions`
- `reload config`
- `reload plugins`
- `reload generation`
- `shutdown`

実行可否は `live.admin.local_console_permissions` で制御します。permission 名は `status`、`sessions`、`reload-config`、`reload-plugins`、`reload-generation`、`shutdown` です。

### built-in gRPC

`static.admin.grpc.enabled = true` のとき、同じ process に unary gRPC control plane が起動します。

- transport は plaintext h2 のみです。
- 既定では loopback bind だけを許可します。
- non-loopback bind には `allow_non_loopback = true` が必要です。
- principal を少なくとも 1 つ定義しないと boot できません。
- `token_file` は config file 基準で relative 解決されます。
- token は trim した結果が non-empty である必要があります。
- principal 間で同じ token を使うことはできません。
- TLS と public exposure は reverse proxy / ingress 側で扱う前提です。

`reload config` 後は remote principal の token / permission が次の request から新設定へ切り替わります。

## stdin EOF と終了条件

console loop の終了条件は stdin の種類と gRPC の有無で変わります。

- terminal stdin で EOF
  shutdown を要求します。
- non-terminal stdin で EOF、かつ gRPC あり
  console だけ detach し、server は継続します。
- non-terminal stdin で EOF、かつ他の admin surface なし
  headless 実行を避けるため warning を出して shutdown します。
- `Ctrl-C`
  常に shutdown を要求します。

このため、pipe 経由で起動する運用では gRPC を併用するか、stdin EOF が来ない構成にしておくのが安全です。
