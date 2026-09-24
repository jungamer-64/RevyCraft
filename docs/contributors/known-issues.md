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

2026-09-08 の Windows local run では、`cargo test --workspace --all-targets --quiet` が成功しました。runtime library test は 168 件、executable upgrade の functional test は 16 件すべて成功しています。

同日の Ubuntu-26.04 / WSL の `cargo test --locked --release -p revy-server-runtime --lib --quiet` も、169 件すべて成功しました。

これらの functional baseline は、1000接続の latency acceptance を代替しません。freeze の数値だけでなく、規定の負荷条件、全sessionの継続、正常終了、abort / rollback の測定はそれぞれ確認が必要です。

2026-09-07 の Ubuntu-26.04 / WSL の `REVY_CUTOVER_LATENCY_OPERATION=reload-core REVY_CUTOVER_LATENCY_TIER=acceptance cargo test --locked --release -p revy-server --test upgrade_runtime latency_cases::production_cutover_latency_mixed_500_500 -- --exact --ignored --nocapture` で、5回の freeze 測定と全接続の最終 gameplay 確認後、shutdown の親process終了待ちが10秒でtimeoutする試行がありました。再試行は正常終了しましたが、原因は未確定です。timeoutした試行は性能job成功には数えません。

2026-09-09 の Windows local run でも、`REVY_CUTOVER_LATENCY_OPERATION=reload-topology REVY_CUTOVER_LATENCY_TIER=acceptance cargo test --release -p revy-server --test upgrade_runtime latency_cases::production_cutover_latency_mixed_500_500 -- --exact --ignored --nocapture` で同じ終了待ちtimeoutが発生しました。追加の4試行では、保存・session終了・listener停止・admin停止がすべて完了し、原因は再現できていません。Linuxの事象と同一原因とは断定していません。

同日の Windows / Java 1000接続の `REVY_CUTOVER_LATENCY_OPERATION=executable-upgrade REVY_CUTOVER_LATENCY_TIER=acceptance cargo test --release -p revy-server --test upgrade_runtime latency_cases::production_cutover_latency_java_1000 -- --exact --ignored --nocapture` で、freeze 719,057 µsによりHard Limitで失敗した試行があります。起動時status集計を `Committed` 送信後へ移した後の20回測定は基準内ですが、元の超過原因を確定できておらず、再現性の確認を継続します。

## この baseline の使い方

- baseline と変更後の failure を比較し、既存 failure と新しい regression を区別します。
- runtime / gameplay / storage の責務整理で failure が増えた場合は、「既知だから放置」ではなく、この文書に追記して drift を見える化します。
- architecture の意図を確認したいときは [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)、reload と migration の意味論を追いたいときは [`core-reload-runtime-design.md`](core-reload-runtime-design.md) を参照します。
