# RevyCraft

Minecraft JE 1.7.10 互換サーバーの初期実装と、既存の Bevy ローカルクライアントを同居させた Rust workspace です。

## Workspace

- `revycraft-client`: 既存のローカル Bevy ボクセルクライアント
- `mc-core`: バージョン非依存のワールド、プレイヤー、イベント、サーバーコア
- `mc-proto-common`: protocol adapter 境界と wire codec 共通部
- `mc-proto-je-1_7_10`: Minecraft JE 1.7.10 専用 codec と Anvil/NBT 変換
- `server-runtime`: protocol-agnostic な dedicated server runtime

## Run

サーバー起動:

```bash
cargo run -p server-runtime
```

Bevy クライアント起動:

```bash
cargo run -p revycraft-client
```

`server-runtime` はルートの `server.properties` を読みます。ファイルが無い場合はデフォルト設定で起動します。
現在のワールド生成は `level-type=FLAT` のみ対応です。
creative-style block editing を使う場合は `gamemode=1` にしてください。
`default-adapter=je-1_7_10` と `storage-profile=je-anvil-1_7_10` で既定 protocol adapter / 永続化 backend を明示できます。
`be-enabled=true` を指定すると、同じ `server-port` 番号で `TCP(JE)` と `UDP(BE placeholder)` を同時 bind します。Bedrock 側は現段階では listener と検知だけで、クライアント向け応答や login/play はまだ未実装です。

## Server Features

- Minecraft JE 1.7.10 / protocol 5 handshake, status ping, login, play
- offline-mode 認証
- superflat overworld 生成
- 初期 chunk bulk 送信
- creative-style block break / place
- player inventory window 0 同期
- starter hotbar と held item 同期
- 複数接続
- 他プレイヤー spawn / teleport / head rotation 同期
- block change 同期
- keepalive
- `level.dat`, `playerdata/*.dat`, `region/*.mca` の read/write
- 将来の multi-version 対応を見据えた core / protocol 分離
- edition-aware handshake routing, adapter registry, storage registry
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
- 現在の登録 adapter / storage profile は `je-1_7_10` / `je-anvil-1_7_10` のみです。
- `be-enabled=true` にすると `be-placeholder` adapter を持つ UDP listener も起動しますが、Bedrock は operator-visible placeholder のみで、検知時に `not implemented` を stderr に出して datagram を破棄します。
- handshake probe が edition を認識して未対応 protocol 番号だった場合、status は `default-adapter` で応答し、login はその adapter の codec で disconnect を返します。
- どの handshake probe にも一致しない接続には誤った codec で応答せず、そのまま切断します。
- BE 向けの registry / edition / transport 識別に加えて UDP listener までは入りましたが、RakNet/BE status/login/play packet 自体はまだ未実装です。
- 現在の network editing は creative 前提です。survival の採掘時間、消費、drop は未実装です。
- player inventory window 0 だけを扱います。containers、crafting、一般 window 操作は未実装です。
- whitelist block/item: `stone`, `dirt`, `grass_block`, `cobblestone`, `oak_planks`, `sand`, `sandstone`, `glass`, `bricks`
- 初期対象外: chat, mobs, combat, Nether/End
- 既存配布ワールドの広範互換ではなく、サーバーが生成した 1.7.10 world の保存・再読込を優先しています。
