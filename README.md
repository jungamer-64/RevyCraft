# RevyCraft

Minecraft JE `1.7.10` / `1.8.x` / `1.12.2` を同一ポートで同時に扱える Rust サーバー実装と、既存の Bevy ローカルクライアントを同居させた workspace です。

## Workspace

- `revycraft-client`: 既存のローカル Bevy ボクセルクライアント
- `mc-core`: バージョン非依存のワールド、プレイヤー、イベント、サーバーコア
- `mc-plugin-api`: stable C ABI と plugin payload 定義
- `mc-plugin-sdk-rust`: Rust plugin authoring helper
- `mc-plugin-proto-je-1_7_10`: phase 1 の JE 1.7.10 protocol plugin
- `mc-proto-common`: protocol adapter 境界と wire codec 共通部
- `mc-proto-je-common`: Java Edition 向けの item/block/slot/chunk 共通 helper
- `mc-proto-je-1_7_10`: Minecraft JE 1.7.10 専用 codec と Anvil/NBT 変換
- `mc-proto-je-1_8_x`: Minecraft JE 1.8.x 専用 codec
- `mc-proto-je-1_12_2`: Minecraft JE 1.12.2 専用 codec
- `server-runtime`: protocol-agnostic な dedicated server runtime
- `server-bootstrap`: built-in storage / legacy adapters と plugin host を束ねる起動 binary

## Run

サーバー起動:

```bash
cargo build -p mc-plugin-proto-je-1_7_10
cargo run -p server-bootstrap
```

Bevy クライアント起動:

```bash
cargo run -p revycraft-client
```

`server-bootstrap` はルートの `server.properties` を読みます。ファイルが無い場合はデフォルト設定で起動します。
現在のワールド生成は `level-type=FLAT` のみ対応です。
creative-style block editing を使う場合は `gamemode=1` にしてください。
`default-adapter=je-1_7_10` と `storage-profile=je-anvil-1_7_10` で既定 protocol adapter / 永続化 backend を明示できます。
`enabled-adapters=je-1_7_10,je-1_8_x,je-1_12_2` を設定すると、同じ TCP ポートで複数 JE 版を同時に受け付けます。
`enabled-adapters` 未指定時は後方互換のため `default-adapter` だけが有効です。
`be-enabled=true` を指定すると、同じ `server-port` 番号で `TCP(JE)` と `UDP(BE placeholder)` を同時 bind します。Bedrock 側は現段階では listener と検知だけで、クライアント向け応答や login/play はまだ未実装です。
`plugins-dir=plugins` 配下の `plugin.toml` を読むと protocol plugin host が有効になります。phase 1 では `plugins/je-1_7_10/plugin.toml` から JE `1.7.10` protocol plugin を読み、`plugin-reload-watch=true` で modified plugin の generation swap を有効化できます。

## Server Features

- Minecraft JE `1.7.10 / protocol 5`、`1.8.x / protocol 47`、`1.12.2 / protocol 340` の handshake / status / login / play
- offline-mode 認証
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
- protocol plugin host / loader / quarantine / generation reload 骨格
- 同一プロセスでの shared-port `TCP(JE)` + `UDP(BE placeholder)` 待受

## Tests

```bash
cargo test --workspace
```

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

- `online-mode=true` は未実装です。現在は fail fast します。
- `level-type` は `FLAT` のみ対応です。その他の値は起動時に reject します。
- `server-runtime` 自体は concrete protocol crate を直接組み込みません。`server-bootstrap` が legacy built-ins と plugin host を束ねます。
- phase 1 で shared library plugin 化されているのは JE `1.7.10` protocol だけです。`1.8.x` / `1.12.2` / `be-placeholder` は bootstrap 側の built-in registration を継続しています。
- 登録済み adapter は `je-1_7_10` / `je-1_8_x` / `je-1_12_2` / `be-placeholder` です。ストレージ profile は `je-anvil-1_7_10` のみです。
- `enabled-adapters` には重複や未知の adapter を指定できません。`default-adapter` はその中に含まれている必要があります。
- plugin host config は `plugins-dir`, `plugin-allowlist`, `plugin-failure-policy`, `plugin-reload-watch`, `plugin-abi-min`, `plugin-abi-max` です。現状の failure policy は `quarantine` のみです。
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
