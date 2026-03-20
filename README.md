# RevyCraft

Minecraft JE `1.7.10` / `1.8.x` / `1.12.2` を同一ポートで同時に扱える Rust サーバー実装と、その runtime / plugin 群をまとめた server-only workspace です。

## Workspace

- `apps/server`: `server-bootstrap` の起動 binary
- `crates/core`: `mc-core`
- `crates/plugin`: `mc-plugin-api`, `mc-plugin-host`, `mc-plugin-sdk-rust`
- `crates/protocol`: `mc-proto-*`
- `crates/runtime`: `server-runtime`
- `plugins/auth`: `mc-plugin-auth-*`
- `plugins/gameplay`: `mc-plugin-gameplay-*`
- `plugins/protocol`: `mc-plugin-proto-*`
- `plugins/storage`: `mc-plugin-storage-*`
- `tools/xtask`: protocol / gameplay / storage / auth plugin packaging task
- `runtime/`: `server.properties.example` と実行時の plugin/world 配置

## Run

サーバー起動:

```bash
cargo run -p xtask -- package-plugins
cargo run -p server-bootstrap
```

`server-bootstrap` は `runtime/server.properties` を読みます。ファイルが無い場合はデフォルト設定で起動します。
サンプル設定は `runtime/server.properties.example` にあります。
現在のワールド生成は `level-type=FLAT` のみ対応です。
creative-style block editing を使う場合は `gamemode=1` にしてください。
`default-adapter=je-1_7_10`、`default-bedrock-adapter=be-26_3`、`storage-profile=je-anvil-1_7_10`、`auth-profile=offline-v1`、`bedrock-auth-profile=bedrock-offline-v1` で既定の JE / BE adapter、永続化 backend、JE / BE auth profile を明示できます。
`online-mode=true` で起動する場合は `auth-profile=mojang-online-v1` に切り替えてください。phase 5 時点では Mojang sessionserver 固定で、verification failure や HTTP error は fail closed します。
`enabled-adapters=je-1_7_10,je-1_8_x,je-1_12_2` を設定すると、同じ TCP ポートで複数 JE 版を同時に受け付けます。
`enabled-bedrock-adapters=be-26_3` を設定すると、同じ `server-port` で Bedrock baseline adapter を受け付けます。
`default-gameplay-profile=canonical` で既定 gameplay profile を選べます。
`gameplay-profile-map=je-1_7_10:readonly,je-1_12_2:canonical,be-26_3:canonical` のように adapter ごとに gameplay profile を固定できます。
`enabled-adapters` 未指定時は後方互換のため `default-adapter` だけが有効です。
`be-enabled=true` を指定すると、同じ `server-port` 番号で `TCP(JE)` と `UDP(BE)` を同時 bind します。phase 6 では Bedrock baseline `be-26_3` の login / move / creative-style block edit まで入っています。`be-placeholder` は probe helper と unsupported version clean reject 用です。
phase 6 以降は protocol / gameplay / storage / auth plugin が必須です。`server-bootstrap` は `runtime/plugins/<plugin-id>/plugin.toml` を読み、そこから shared library を解決します。runtime は `target/` を直接読みません。
`plugin-reload-watch=true` か `RunningServer::reload_plugins().await` を使うと、protocol / gameplay / storage / auth plugin の generation swap を有効化できます。protocol reload は active `status/login/play` session の export/import が全件成功したときだけ swap し、candidate は `adapter_id`, `transport`, `edition`, `protocol_number`, `wire_format`, `bedrock_listener_descriptor` を維持している必要があります。storage reload は host の in-memory snapshot を candidate backend に import し、auth reload は新規 login から新 generation を使います。
`topology-reload-watch=true` か `RunningServer::reload_topology().await` を使うと、config source と最新 protocol plugin 群から listener / route / default adapter の topology generation を再構成できます。same-port listener は可能な限り再利用し、旧 topology session は `topology-drain-grace-secs` の猶予後に退役します。
`RunningServer::status().await` は active/draining topology、listener、session summary、plugin health をまとめた read-only snapshot を返します。個別 session は `RunningServer::session_status().await`、plugin kind ごとの generation / quarantine / artifact 情報は `PluginHost::status()` で見られます。
`server-bootstrap` は起動時に human-readable runtime summary を 1 回表示し、watch-driven reload 成功時にも short summary を出します。plugin が active quarantine / artifact quarantine / fail-fast に入ったときも運用ログを出します。

## Server Features

- Minecraft JE `1.7.10 / protocol 5`、`1.8.x / protocol 47`、`1.12.2 / protocol 340` の handshake / status / login / play
- offline-mode / online-mode 認証
- superflat overworld 生成
- 初期 chunk 送信
- creative-style block break / place
- player inventory window 0 同期
- starter hotbar と held item 同期
- 1.12.2 offhand 永続化と旧版向け劣化変換
- 複数接続
- 他プレイヤー spawn / teleport / head rotation 同期
- block change 同期
- keepalive
- `level.dat`, `playerdata/*.dat`, `region/*.mca` の read/write
- 将来の multi-version 対応を見据えた core / protocol 分離
- edition-aware handshake routing, adapter registry, storage registry
- protocol plugin host / loader / quarantine / generation reload
- gameplay profile host / resolver / generation reload
- storage profile host / generation reload
- offline / online auth profile host / generation reload
- protocol plugin package layout `runtime/plugins/<plugin-id>/plugin.toml`
- gameplay profile package layout `runtime/plugins/<plugin-id>/plugin.toml`
- 同一プロセスでの shared-port `TCP(JE)` + `UDP(BE 26.3 baseline)` 待受
- read-only runtime / plugin introspection snapshot (`status()`, `session_status()`, `PluginHost::status()`)

## Tests

```bash
cargo test --workspace
```

`server-runtime` の integration test は shared test harness 経由で protocol / gameplay / storage / auth plugin を package/load します。

## Notes

- `level-type` は `FLAT` のみ対応です。その他の値は起動時に reject します。
- 既定の実行時データは `runtime/` 配下に集約されます。plugin package は `runtime/plugins/`、ワールドは `runtime/world/`、設定は `runtime/server.properties` です。
- `server-runtime` 自体は concrete protocol crate や built-in storage/auth 実装を直接組み込みません。`server-bootstrap` は plugin host だけを束ね、protocol / gameplay / storage / auth は plugin から読み込みます。
- 登録済み adapter は `je-1_7_10` / `je-1_8_x` / `je-1_12_2` / `be-26_3` / `be-placeholder` です。ストレージ profile は `je-anvil-1_7_10`、auth profile は `offline-v1` / `mojang-online-v1` / `bedrock-offline-v1` / `bedrock-xbl-v1` です。
- 登録済み gameplay profile は `canonical` / `readonly` です。profile は `default-gameplay-profile` と `gameplay-profile-map` で固定され、接続中の reassignment はしません。
- `enabled-adapters` には重複や未知の adapter を指定できません。`default-adapter` はその中に含まれている必要があります。
- plugin host config は `plugins-dir`, `plugin-allowlist`, `plugin-failure-policy-protocol`, `plugin-failure-policy-gameplay`, `plugin-failure-policy-storage`, `plugin-failure-policy-auth`, `plugin-reload-watch`, `topology-reload-watch`, `topology-drain-grace-secs`, `plugin-abi-min`, `plugin-abi-max`, `storage-profile`, `auth-profile`, `default-gameplay-profile`, `gameplay-profile-map` です。既定値は `protocol=quarantine`, `gameplay=quarantine`, `storage=fail-fast`, `auth=skip` です。
- protocol plugin reload は manifest に `runtime.reload.protocol` capability を要求し、plugin ABI `2.0` の `ProtocolSessionSnapshot { connection_id, phase, player_id, entity_id }` を使って active session migration を行います。
- topology reload は `ServerConfigSource::{Inline, Properties}` を入口にし、`server-ip`, `server-port`, `be-enabled`, `motd`, `max-players`, `default-adapter`, `enabled-adapters`, `default-bedrock-adapter`, `enabled-bedrock-adapters` だけを live 反映します。plugin root / allowlist / auth / storage / gameplay profile 設定は process-static のままです。
- introspection snapshot は `RunningServer::status()` を主入口にし、active/draining topology、listener binding、session summary、dirty flag、plugin host status を返します。詳細 session 一覧は `RunningServer::session_status()` に分かれています。
- `PluginHost::status()` は active runtime view だけを返し、protocol / gameplay / storage / auth の generation、failure action、current artifact、active quarantine、artifact quarantine、pending fatal を kind ごとに expose します。`quarantine_reason()` は互換用の convenience API として残っています。
- `online-mode=true` は `auth-profile=mojang-online-v1` を要求し、`online-mode=false` は `auth-profile=offline-v1` を要求します。Bedrock は `bedrock-auth-profile=bedrock-offline-v1` か `bedrock-auth-profile=bedrock-xbl-v1` を使います。auth reload は新規 login からだけ新 generation を使い、challenge 発行済み session や既存 play session は旧 generation のまま完了します。
- online-mode の JE login は RSA-1024 challenge と AES-128-CFB8 transport encryption を使います。compression negotiation と `hasJoined` の `ip` parameter はまだ未対応です。
- Linux では package した `.so` を使った `dlopen + reload` テストまで入っています。manifest / artifact key 自体は cross-platform です。
- `be-enabled=true` にすると `default-bedrock-adapter` と `enabled-bedrock-adapters` が必須になります。phase 6 では `be-26_3` を baseline として login / move / creative-style block edit を扱い、unsupported Bedrock version には clean reject を返します。
- handshake probe が edition を認識して未対応 protocol 番号だった場合、status は `default-adapter` で応答し、login はその adapter の codec で disconnect を返します。
- `enabled-adapters` で無効化した JE 版も probe 自体は残るため、status/login の fallback は維持されます。
- どの handshake probe にも一致しない接続には誤った codec で応答せず、そのまま切断します。
- Bedrock phase 6 は creative-style world interaction までで、chat、containers、combat、mobs、Nether/End はまだ未実装です。
- 現在の network editing は creative 前提です。survival の採掘時間、消費、drop は未実装です。
- player inventory window 0 だけを扱います。containers、crafting、一般 window 操作は未実装です。`1.12.2` の offhand は core で保持し、`1.7.10` / `1.8.x` には送出しません。
- whitelist block/item: `stone`, `dirt`, `grass_block`, `cobblestone`, `oak_planks`, `sand`, `sandstone`, `glass`, `bricks`
- 初期対象外: chat, mobs, combat, Nether/End
- 既存配布ワールドの広範互換ではなく、サーバーが生成した semantic world の保存・再読込を優先しています。
