# iOSの未達条件（2026-10-10確認）

iOSの許可・アプリ選択・計測・遮蔽は未実装です。Androidの計測結果や、フレームワークの存在確認をiOSの成功として扱いません。

## 計測の意味

Appleの[DeviceActivityEvent](https://developer.apple.com/documentation/deviceactivity/deviceactivityevent)は、対象アプリ・カテゴリ・Webドメインが画面の最前面にある時間を活動時間としています。動画の再生状態を確認する値ではありません。したがって、このAPIでしきい値を検出して遮蔽できたとしても、本製品の「実際に再生された時間」という条件を満たしたとは判定できません。iOSで他社アプリの再生時間を取得する経路は、まだ確認できていません。

## Rustからの接続

調査済みの `swift-runtime-sys` コミット `025a1477436cf591eed50a4f69d5c4e9e79ae757` のFamily Controls・Device Activity・Managed Settings候補は、利用可否の確認だけでした。許可・選択・しきい値の登録・遮蔽に使える実装として採用していません。

[objc2のSwift連携資料](https://docs.rs/objc2/latest/objc2/topics/swift/index.html)は、Swift専用フレームワークの自動マッピングを対象外とし、SwiftでC ABIまたはObjective-Cの入口を用意する方法を案内しています。この製品はSwiftの新規追加を許していないため、その案を実装済みの解決策とは扱いません。Rustだけで必要なSwiftの型・非同期呼出し・ピッカー・拡張を構成できないと証明したわけでもありません。

次の技術検証には、対象iOS SDKの実際の公開宣言とABI、利用できるmacOSビルド環境、署名済みアプリと拡張の起動確認が必要です。推測した呼出規約やモックの成功をOS接続の証拠にしません。

## Rustコンパイラーで確認した境界（2026-10-10追加）

Rustの[Swift呼出規約の実験](https://github.com/rust-lang/rust/issues/156481)を、インストール済みのnightlyで追加検証しました。`tools/ios-abi-probe.rs` はApple APIを呼ばない、未定義の外部関数を使う小さなコンパイラー実験です。製品ライブラリの一部ではありません。

`rustc 1.99.0-nightly (eff8269f7 2026-07-18)` で、`aarch64-apple-ios` と `aarch64-apple-ios-sim` のLLVM IR生成は両方終了コード0でした。実際のIRには `call swiftcc void @abi_probe_external()` と `declare swiftcc void @abi_probe_external()` がありました。同じコードをWindowsのターゲットでコンパイルすると、E0570（対象ターゲットでSwift ABIをサポートしない）でした。この違いは、RustからSwift呼出規約を出力する経路が存在する証拠です。

これはSDKの型レイアウト、SwiftUIのgeneric metadataやprotocol witness table、非同期コンテキスト、Appleフレームワークのリンク、実行を確認した結果ではありません。特に、空の外部関数の呼出規約を、許可要求・アプリ選択・監視・遮蔽の接続成功へ読み替えません。Swiftコードの生成や追加も行っていません。

実SDKの確認用に、手動起動だけの `Manual iOS SDK audit` を準備しました。XcodeのSDKから対象APIの識別子を収集し、同じRust実験のIR・assembly・未解決シンボルと、実施範囲を保存します。SDK全体の宣言は成果物へ含めません。OS実行や署名は行わない検査です。このmacOSジョブはまだ実行していません。リポジトリは非公開で、[GitHubの標準macOS runner](https://docs.github.com/en/actions/concepts/billing-and-usage)を無料と決めつけず、追加費用または検査コードだけの公開を許可する判断を待ちます。

## アカウントと配布

Apple Developerのログインは完了しておらず、会員・既存署名・配布許可の状態は未確認です。保存済み自動入力は表示されず、指定表のApple検索結果は0件でした。iPhoneによるサインインは検証中の表示から先へ進まず、本人の承認画面へ到達したとは記録していません。新規有料契約は行っていません。

[Appleの配布許可手順](https://developer.apple.com/documentation/familycontrols/requesting-the-family-controls-entitlement)では、Account HolderによるFamily Controlsの配布許可申請が必要です。使うScreen Time拡張にも申請し、承認後の能力と対応する配布方法を確認します。開発用の能力が存在することを、配布許可の証拠にしません。

これらは共有ToDo1614で継続する条件です。
