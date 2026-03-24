# 設定と reload 運用

## 概要

この文書は、`runtime/server.toml.example` を基準に RevyCraft の設定項目と reload 挙動を整理したものです。どの項目が restart-required で、どの項目が live reload できるかを運用者向けにまとめます。

## 対象読者

- `runtime/server.toml` を編集して運用する人
- manual reload と watch reload の違いを把握したい人
- admin console / gRPC の設定境界を確認したい人

## この文書でわかること

- `static` と `live` の役割分担
- reload できる項目 / できない項目
- `reload plugins` / `reload config` / `reload generation` の使い分け
- failure policy と operator 向け観測面

## 関連資料

- [`getting-started.md`](getting-started.md)
- [`../contributors/reload-semantics-and-boundaries.md`](../contributors/reload-semantics-and-boundaries.md)
- [`../../runtime/server.toml.example`](../../runtime/server.toml.example)

## 設定セクションの見取り図

| セクション | 主な役割 | reload 可否 |
| --- | --- | --- |
| `static.bootstrap` | world、online mode、level type、storage profile | restart-required |
| `static.plugins` | plugin directory、plugin ABI range | restart-required |
| `static.admin.grpc` | built-in gRPC transport の bind と exposure | transport 設定は restart-required |
| `live.network` | bind address、port、MOTD、max players | `reload_generation` または `reload_config` |
| `live.topology` | enabled adapters、Bedrock listener、drain、watch flag | `reload_generation` または `reload_config` |
| `live.plugins` | allowlist、buffer limits、failure policy、watch flag | `reload_config` |
| `live.profiles` | auth / bedrock auth / gameplay profile selection | `reload_config` |
| `live.admin` | active admin-ui profile、local console permissions | `reload_config` |

補足として、`static.admin.grpc.principals.<id>.token_file` と `permissions` は TOML 上は `static.admin.grpc` 配下ですが、transport とは別扱いです。`reload_config()` で更新されます。

## restart-required な項目

現行実装では次の変更は reload で受け付けません。

- `static.bootstrap` 全体
- `static.plugins` 全体
- `static.admin.grpc.enabled`
- `static.admin.grpc.bind_addr`
- `static.admin.grpc.allow_non_loopback`

特に `static.bootstrap.level_type` は `"flat"` のみ対応です。`storage_profile` や `online_mode` は `static.bootstrap`、plugin directory と ABI range は `static.plugins` に属します。どちらも restart 前提で扱います。

## live reload できる項目

### `live.network`

- `server_ip`
- `server_port`
- `motd`
- `max_players`

### `live.topology`

- `be_enabled`
- `default_adapter`
- `enabled_adapters`
- `default_bedrock_adapter`
- `enabled_bedrock_adapters`
- `reload_watch`
- `drain_grace_secs`

### `live.plugins`

- `allowlist`
- `reload_watch`
- `buffer_limits.protocol_response_bytes`
- `buffer_limits.gameplay_response_bytes`
- `buffer_limits.storage_response_bytes`
- `buffer_limits.auth_response_bytes`
- `buffer_limits.admin_ui_response_bytes`
- `buffer_limits.callback_payload_bytes`
- `buffer_limits.metadata_bytes`
- `failure_policy.protocol`
- `failure_policy.gameplay`
- `failure_policy.storage`
- `failure_policy.auth`
- `failure_policy.admin_ui`

`buffer_limits.*` は plugin ABI 境界で取り込む response / callback payload / metadata の上限です。`reload_config()` で更新され、`reload_generation()` では更新されません。

### `live.profiles`

- `auth`
- `bedrock_auth`
- `default_gameplay`
- `gameplay_map`

### `live.admin`

- `ui_profile`
- `local_console_permissions`
- remote principal の token / permission map

## profile と allowlist の基本

plugin package が `runtime/plugins/` に存在しても、active になるかどうかは allowlist と profile selection が決めます。

- allowlist
  runtime selection の候補に入れる plugin id の集合です。
- auth / gameplay / admin-ui profile
  実際に active にする profile id です。
- storage profile
  `static.bootstrap.storage_profile` なので restart-required です。

sample config は offline-first です。JE online auth や Bedrock XBL を使うときは `package-all-plugins` 実行後に allowlist と profile を明示してください。

- JE online auth
  `auth-mojang-online` を allowlist に追加し、`static.bootstrap.online_mode = true` と `live.profiles.auth = "mojang-online-v1"` を設定します。
- Bedrock XBL
  `auth-bedrock-xbl` を allowlist に追加し、`live.profiles.bedrock_auth = "bedrock-xbl-v1"` を設定します。

## admin console と gRPC

### stdio console

`live.admin.ui_profile` で有効な admin-ui profile が解決できた場合、stdio 上に line-oriented operator loop を起動します。parse / render は admin-ui plugin に委譲されます。

sample console では次の command が使えます。

- `status`
- `sessions`
- `reload config`
- `reload plugins`
- `reload generation`
- `shutdown`

実行可否は `live.admin.local_console_permissions` で permission 単位に制御します。

### built-in gRPC

`static.admin.grpc.enabled = true` のとき、同じ process に unary gRPC control plane が起動します。built-in transport は plaintext h2 のみです。

- 既定では loopback bind のみ許可します。
- non-loopback bind には `allow_non_loopback = true` が必要です。
- public exposure と TLS は reverse proxy / ingress 側で終端してください。
- 有効化時は `static.admin.grpc.principals.<id>` を少なくとも 1 つ定義する必要があります。

`reload_config()` 後は remote principal token と permission が次の request から新設定へ切り替わります。

## manual reload の使い分け

### `reload plugins`

artifact が更新された managed plugin だけを差し替えます。selection config は変えません。次の変更は見ません。

- allowlist の変更
- auth / gameplay / admin-ui profile の変更
- listener / port / MOTD の変更
- failure policy の変更

「同じ selection のまま新しい plugin artifact を入れ替えたい」ときに使います。

`live.plugins.buffer_limits` の変更もこの操作では反映しません。

### `reload config`

config source を再読込し、plugin selection と topology generation の両方を再評価します。allowlist、profile map、buffer limits、failure policy、admin principal の変更をまとめて反映したいときはこれを使います。

ただし、次のような条件では失敗します。

- restart-required な static 設定を変えた
- まだ session が使っている gameplay profile を config から外そうとした
- candidate selection や candidate topology が validation を通らない

### `reload generation`

最新 config を読みますが、反映するのは `network` と `topology` だけです。plugin selection や `live.plugins.buffer_limits` を変えたくないまま、listener / routing / default adapter / MOTD などを更新したいときに使います。

reload 後は新規接続が新 generation に入り、旧 generation の session は `drain_grace_secs` の間だけ継続します。

## watch reload

`live.plugins.reload_watch = true` または `live.topology.reload_watch = true` を有効にすると、runtime loop が定期的に config source を読み直し、`reload_config` 相当の処理を試みます。

watch reload でも artifact 差分だけではなく config と topology を再評価します。sample config の既定値は両方 `false` です。

## config path override

`server-bootstrap` は既定で `runtime/server.toml` を読みます。別 path を明示したいときは `REVY_SERVER_CONFIG` を使います。

```bash
REVY_SERVER_CONFIG=/srv/revy/server.toml cargo run -p server-bootstrap
```

指定 path が見つからない場合は warning を出し、default config fallback で起動を試みます。

## failure policy の読み方

kind ごとの既定値は次のとおりです。

- protocol = `quarantine`
- gameplay = `quarantine`
- storage = `fail-fast`
- auth = `skip`
- admin-ui = `skip`

運用上の意味は次のように読むとわかりやすいです。

- `skip`
  壊れた候補だけを見送り、旧世代を維持します。
- `quarantine`
  壊れた candidate artifact や active plugin を隔離し、同じ失敗を繰り返しにくくします。
- `fail-fast`
  その失敗を runtime 全体の重大障害として扱い、graceful stop に入ります。

## 観測面

operator がまず使う観測面は次の 3 つです。

- `status`
  active / draining generation、listener、session summary、plugin host status を見ます。
- `sessions`
  connection ごとの詳細 session 状態を見ます。
- plugin host snapshot
  active quarantine、artifact quarantine、pending fatal の有無を見ます。

plugin runtime failure や quarantine 発生時は plugin host 側でも短いログが出ます。
