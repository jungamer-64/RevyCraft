# contributors 向け既知課題

- 対象読者: boundary redesign や runtime まわりの code motion を始める前に、現行 baseline と既知の failing test を確認したい contributors
- この文書で扱う範囲: local test baseline、既知の failing test、更新時の書き方
- この文書で扱わないこと: 個々の failure の根本原因分析、operator 向け障害対応、issue tracker の運用ルール
- 次に読む文書: [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)

この文書は architecture 正本から切り離した、contributors 向けの鮮度管理用メモです。設計文書に一時的な failing baseline を埋め込まず、作業前提だけをここで更新します。

## 更新ルール

- baseline には必ず実行日と command を書きます。
- 失敗一覧は「いま code motion の前提として意識すべきもの」だけを残します。
- failure が解消されたらこの文書から消し、別の failure を追加したときは日付と command も更新します。

## 現在の local baseline

2026-03-29 に `cargo test -p revy-server-runtime --lib --quiet` を再実行した local baseline では、次の 5 件が failure しました。

- `runtime::tests::gameplay::container_windows::world_backed_crafting_table_opens_and_crafts_chest_via_protocol`
- `runtime::tests::gameplay::furnace::world_backed_furnace_opens_smelts_and_closes_via_protocol`
- `runtime::tests::gameplay::furnace::world_backed_furnace_output_persists_across_restart`
- `runtime::tests::gameplay::world_chest::world_backed_chest_place_open_and_persist_across_restart`
- `runtime::tests::gameplay::world_chest::world_backed_chest_syncs_slot_updates_to_other_viewers`

## この baseline の使い方

- boundary redesign の code motion は、この baseline を green に戻すか、明示的に quarantine してから始めます。
- runtime / gameplay / storage の責務整理で failure が増えた場合は、「既知だから放置」ではなく、この文書に追記して drift を見える化します。
- architecture の意図を確認したいときは [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)、reload と migration の意味論を追いたいときは [`core-reload-runtime-design.md`](core-reload-runtime-design.md) を参照します。
