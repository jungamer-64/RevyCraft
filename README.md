# RevyCraft

Minecraft JE `1.7.10` / `1.8.x` / `1.12.2` を同一ポートで同時に扱える Rust サーバー実装と、既存の Bevy ローカルクライアントを同居させた workspace です。

## Workspace

- `revycraft-client`: 既存のローカル Bevy ボクセルクライアント
- `mc-core`: バージョン非依存のワールド、プレイヤー、イベント、サーバーコア
- `mc-plugin-api`: stable C ABI と binary envelope codec
- `mc-plugin-sdk-rust`: Rust plugin authoring helper
- `mc-plugin-proto-je-1_7_10`: JE 1.7.10 protocol plugin
- `mc-plugin-proto-je-1_8_x`: JE 1.8.x protocol plugin
- `mc-plugin-proto-je-1_12_2`: JE 1.12.2 protocol plugin
- `mc-plugin-proto-be-placeholder`: Bedrock placeholder protocol plugin
- `mc-plugin-gameplay-canonical`: canonical gameplay profile plugin
- `mc-plugin-gameplay-readonly`: readonly gameplay profile plugin
- `mc-plugin-storage-je-anvil-1_7_10`: JE 1.7.10 Anvil storage plugin
- `mc-plugin-auth-offline`: offline authentication plugin
- `mc-plugin-auth-mojang-online`: Mojang sessionserver-backed online authentication plugin
- `mc-proto-common`: protocol adapter 境界と wire codec 共通部
- `mc-proto-je-common`: Java Edition 向けの item/block/slot/chunk 共通 helper
- `mc-proto-je-1_7_10`: Minecraft JE 1.7.10 専用 codec と Anvil/NBT 変換
- `mc-proto-je-1_8_x`: Minecraft JE 1.8.x 専用 codec
- `mc-proto-je-1_12_2`: Minecraft JE 1.12.2 専用 codec
- `server-runtime`: protocol-agnostic な dedicated server runtime
- `server-bootstrap`: plugin host を束ねる起動 binary
- `xtask`: protocol / gameplay / storage / auth plugin packaging task

## Run

サーバー起動:

```bash
cargo run -p xtask -- package-plugins
cargo run -p server-bootstrap
```

Bevy クライアント起動:

```bash
cargo run -p revycraft-client
```

`server-bootstrap` はルートの `server.properties` を読みます。ファイルが無い場合はデフォルト設定で起動します。
現在のワールド生成は `level-type=FLAT` のみ対応です。
creative-style block editing を使う場合は `gamemode=1` にしてください。
`default-adapter=je-1_7_10`、`storage-profile=je-anvil-1_7_10`、`auth-profile=offline-v1` で既定 protocol adapter / 永続化 backend / offline auth profile を明示できます。
`online-mode=true` で起動する場合は `auth-profile=mojang-online-v1` に切り替えてください。phase 5 時点では Mojang sessionserver 固定で、verification failure や HTTP error は fail closed します。
`enabled-adapters=je-1_7_10,je-1_8_x,je-1_12_2` を設定すると、同じ TCP ポートで複数 JE 版を同時に受け付けます。
`default-gameplay-profile=canonical` で既定 gameplay profile を選べます。
`gameplay-profile-map=je-1_7_10:readonly,je-1_12_2:canonical` のように adapter ごとに gameplay profile を固定できます。
`enabled-adapters` 未指定時は後方互換のため `default-adapter` だけが有効です。
`be-enabled=true` を指定すると、同じ `server-port` 番号で `TCP(JE)` と `UDP(BE placeholder)` を同時 bind します。Bedrock 側は現段階では listener と検知だけで、クライアント向け応答や login/play はまだ未実装です。
phase 5 以降は protocol / gameplay / storage / auth plugin が必須です。`server-bootstrap` は `dist/plugins/<plugin-id>/plugin.toml` を読み、そこから shared library を解決します。runtime は `target/` を直接読みません。
`plugin-reload-watch=true` か `RunningServer::reload_plugins().await` を使うと、protocol / gameplay / storage / auth plugin の generation swap を有効化できます。storage reload は host の in-memory snapshot を candidate backend に import し、auth reload は新規 login から新 generation を使います。

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
- protocol plugin package layout `dist/plugins/<plugin-id>/plugin.toml`
- gameplay profile package layout `dist/plugins/<plugin-id>/plugin.toml`
- 同一プロセスでの shared-port `TCP(JE)` + `UDP(BE placeholder)` 待受

## Tests

```bash
cargo test --workspace
```

`server-runtime` の integration test は shared test harness 経由で protocol / gameplay / storage / auth plugin を package/load します。

## Client Controls

- `WASD`: move
- `Space`: jump
- `Mouse`: look
- `Left Click`: remove block
- `Right Click`: place selected block
- `1` / `2` / `3`: select `Grass` / `Dirt` / `Stone`
- `Esc`: release cursor
- `Left Click` after release: recapture cursor

## Notes

- `level-type` は `FLAT` のみ対応です。その他の値は起動時に reject します。
- `server-runtime` 自体は concrete protocol crate や built-in storage/auth 実装を直接組み込みません。`server-bootstrap` は plugin host だけを束ね、protocol / gameplay / storage / auth は plugin から読み込みます。
- 登録済み adapter は `je-1_7_10` / `je-1_8_x` / `je-1_12_2` / `be-placeholder` です。ストレージ profile は `je-anvil-1_7_10`、auth profile は `offline-v1` / `mojang-online-v1` です。
- 登録済み gameplay profile は `canonical` / `readonly` です。profile は `default-gameplay-profile` と `gameplay-profile-map` で固定され、接続中の reassignment はしません。
- `enabled-adapters` には重複や未知の adapter を指定できません。`default-adapter` はその中に含まれている必要があります。
- plugin host config は `plugins-dir`, `plugin-allowlist`, `plugin-failure-policy`, `plugin-reload-watch`, `plugin-abi-min`, `plugin-abi-max`, `storage-profile`, `auth-profile`, `default-gameplay-profile`, `gameplay-profile-map` です。現状の failure policy は `quarantine` のみです。
- `online-mode=true` は `auth-profile=mojang-online-v1` を要求し、`online-mode=false` は `auth-profile=offline-v1` を要求します。auth reload は新規 login からだけ新 generation を使い、challenge 発行済み session や既存 play session は旧 generation のまま完了します。
- online-mode の JE login は RSA-1024 challenge と AES-128-CFB8 transport encryption を使います。compression negotiation と `hasJoined` の `ip` parameter はまだ未対応です。
- Linux では package した `.so` を使った `dlopen + reload` テストまで入っています。manifest / artifact key 自体は cross-platform です。
- `be-enabled=true` にすると `be-placeholder` adapter を持つ UDP listener も起動しますが、Bedrock は operator-visible placeholder のみで、検知時に `not implemented` を stderr に出して datagram を破棄します。
- handshake probe が edition を認識して未対応 protocol 番号だった場合、status は `default-adapter` で応答し、login はその adapter の codec で disconnect を返します。
- `enabled-adapters` で無効化した JE 版も probe 自体は残るため、status/login の fallback は維持されます。
- どの handshake probe にも一致しない接続には誤った codec で応答せず、そのまま切断します。
- BE 向けの registry / edition / transport 識別に加えて UDP listener までは入りましたが、RakNet/BE status/login/play packet 自体はまだ未実装です。
- 現在の network editing は creative 前提です。survival の採掘時間、消費、drop は未実装です。
- player inventory window 0 だけを扱います。containers、crafting、一般 window 操作は未実装です。`1.12.2` の offhand は core で保持し、`1.7.10` / `1.8.x` には送出しません。
- whitelist block/item: `stone`, `dirt`, `grass_block`, `cobblestone`, `oak_planks`, `sand`, `sandstone`, `glass`, `bricks`
- 初期対象外: chat, mobs, combat, Nether/End
- 既存配布ワールドの広範互換ではなく、サーバーが生成した semantic world の保存・再読込を優先しています。
