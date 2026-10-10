# iOSの未達条件（2026-10-10確認）

iOSの許可・アプリ選択・計測・遮蔽は未実装です。Androidの計測結果や、フレームワークの存在確認をiOSの成功として扱いません。

## 計測の意味

Appleの[DeviceActivityEvent](https://developer.apple.com/documentation/deviceactivity/deviceactivityevent)は、対象アプリ・カテゴリ・Webドメインが画面の最前面にある時間を活動時間としています。動画の再生状態を確認する値ではありません。したがって、このAPIでしきい値を検出して遮蔽できたとしても、本製品の「実際に再生された時間」という条件を満たしたとは判定できません。iOSで他社アプリの再生時間を取得する経路は、まだ確認できていません。

## Rustからの接続

調査済みの `swift-runtime-sys` コミット `025a1477436cf591eed50a4f69d5c4e9e79ae757` のFamily Controls・Device Activity・Managed Settings候補は、利用可否の確認だけでした。許可・選択・しきい値の登録・遮蔽に使える実装として採用していません。

[objc2のSwift連携資料](https://docs.rs/objc2/latest/objc2/topics/swift/index.html)は、Swift専用フレームワークの自動マッピングを対象外とし、SwiftでC ABIまたはObjective-Cの入口を用意する方法を案内しています。この製品はSwiftの新規追加を許していないため、その案を実装済みの解決策とは扱いません。Rustだけで必要なSwiftの型・非同期呼出し・ピッカー・拡張を構成できないと証明したわけでもありません。

次の技術検証には、対象iOS SDKの実際の公開宣言とABI、利用できるmacOSビルド環境、署名済みアプリと拡張の起動確認が必要です。推測した呼出規約やモックの成功をOS接続の証拠にしません。

## アカウントと配布

Apple Developerのログインは完了しておらず、会員・既存署名・配布許可の状態は未確認です。保存済み自動入力は表示されず、指定表のApple検索結果は0件でした。iPhoneによるサインインは検証中の表示から先へ進まず、本人の承認画面へ到達したとは記録していません。新規有料契約は行っていません。

[Appleの配布許可手順](https://developer.apple.com/documentation/familycontrols/requesting-the-family-controls-entitlement)では、Account HolderによるFamily Controlsの配布許可申請が必要です。使うScreen Time拡張にも申請し、承認後の能力と対応する配布方法を確認します。開発用の能力が存在することを、配布許可の証拠にしません。

これらは共有ToDo1614で継続する条件です。
