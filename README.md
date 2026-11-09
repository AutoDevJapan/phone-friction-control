# 利用制限

成人が自分で選んだアプリの利用上限と休憩時間を管理する、Rust製のAndroid試作です。別製品 `phone-friction` のランチャーとは別に開発しています。画面の基準は `mockups/usage-break-ui.png` です。

## Androidの設定

1. 利用時間の説明を読み、「使用状況へのアクセス」をAndroid設定で許可します。
2. 対象アプリ、1日の上限、休憩時間を選びます。
3. 使用中の中断を使う場合は、設定画面の説明を読んで「ユーザー補助」で利用制限を有効にします。上限を超えた対象アプリが前面にある場合、休憩画面へ切り替えます。
4. 再生時間だけを数える場合は、そのモードを選び、「通知へのアクセス」も許可します。

ユーザー補助の許可がなければ、ホームにこのアプリを設定し、ホームへ戻った時に上限を確認する方式です。許可した場合も、システム設定からの権限解除、アプリの強制停止・削除を妨げません。非対象のアプリを止めません。

## 計測と中断の範囲

前面利用のモードは、AndroidのUsageEventsをRustで集計します。日付の境界、アプリの切替、複数Activity、画面消灯、端末再起動を扱います。履歴は初回に読み、その後は前回からの差分を取得します。遅れて追加される履歴のため直近10秒を繰り返し確認し、確定分だけを利用時間へ加えます。表示と発動には約10秒の遅れがあります。

再生時間のモードは、選んだアプリが前面にあり、MediaSessionがPLAYINGを通知した間だけを数えます。停止、読み込み待ち、画面消灯、別アプリへ切り替えた間は数えません。選んだアプリのセッションがない場合や状態が不明な場合は「取得できません」と表示し、利用時間で置き換えたり、その日をゼロで上書きしたりしません。複数の対象の一部だけ取得できる場合も、全体の時間は取得不能として前の記録を保持します。サービスは確認できた再生分だけを加え、その分だけで上限に達した場合は休憩を開始します。利用時間と再生時間の記録は分けて保存します。

再生状態は動画と音声を区別せず、本人が画面を見ていたことも証明できません。MediaSessionを公開しないアプリには対応しません。TikTokやYouTubeを含むすべてのアプリで動くとは説明しません。アプリごとの実際の対応状況は別途確認が必要です。

Android 37の専用仮想端末とYouTube 20.10.41では、公開動画の再生時間を計測し、試験用の1分上限で一時停止と休憩画面への切替を確認しました。休憩中にPiPの再生を再開しても、一時停止を要求して約1.7秒後にPAUSEDへ戻ることを観測しました。試験設定はADBで与えたもので、通常の画面では1分の上限を選べません。YouTubeのすべての版・動画・機種や、TikTokの対応を保証するものではありません。詳細は `docs/android-verification.md` に記録しています。

その日の上限で休憩を開始し、同じ日の休憩は一度だけです。日をまたぐ既存の休憩を短くしません。期限は端末の時刻を使います。時計の変更を阻止する製品ではありません。休憩中の選択アプリへの再入場も、サービスが前面のアプリを検出して休憩画面へ切り替えます。OSや機種によるサービスの再接続・背景起動の違いは実機検証が必要です。

通知アクセスがある場合、休憩中は選んだアプリのPLAYINGセッションだけに一時停止を要求します。画面消灯や背景再生でも要求します。非対象のセッションへは要求を送りません。対象が要求に応じない場合やセッションを公開しない場合は再生を止められず、休憩画面への切替だけです。画面を切り替えたことを再生停止の証拠として扱いません。

## 保存と広告

アプリ名、利用者の設定、日ごとの合計利用時間と合計再生時間、休憩期限を端末内だけに保存します。画面の文字・画像・操作内容、通知本文、動画の題名を読みません。通常版はインターネット権限や広告SDKを含みません。

利用時間の更新は10秒ごと、休憩期限や設定変更はすぐに保存します。サービスと画面は別の保存先を使い、画面の更新には同一プロセス内の共有状態を使います。保存に失敗した場合もメモリ上の休憩は続け、失敗を画面へ示します。読めないファイルを空の状態で上書きしません。状態を読めない間も、新しく観測した時間と設定に残った休憩はメモリ上で制御します。合計は取得不能として表示し、5秒ごとに読み直します。読み取りが回復した場合は、重複するOS利用時間と、新しく観測した再生時間を区別して前の記録へ統合します。

広告枠は任意表示で、このアプリ内に限ります。休憩画面へ置かず、広告視聴・拒否・読み込み失敗によって休憩を解除しません。広告版はGoogle GMA Next-Gen 1.5.0とUMP 4.0.0を接続します。ホームで表示を選んだ後に同意状況を更新し、`canRequestAds` の確認後に広告を要求します。試験版は公式の試験IDだけを使用します。SDKはGoogleへ端末・通信情報などを送るため、操作前に説明します。選択アプリ名や利用時間はSDKへ渡しません。広告の個人向け最適化・SDKのクラッシュ報告・媒介初期化を無効にします。製品用AdMob登録は自動審査で拒否され、実際のIDは未作成です。製品用の広告ユニットによる実配信は未確認です。

iOS版、Family Controls配布許可、製品用広告IDでの配布版、署名付きAPKの一般配布は未完了です。0.4.2ではAPI26仮想端末の通常設定・30分上限から5分休憩・期限前の再入拒否・満了後の起動・HOME設定を確認しました。物理Galaxy S24（API36、arm64-v8a）では、私的なTest Lab検査で起動・設定変更・Back/Home後の復帰描画を確認しました。実端末でのUsage許可・サービス制御・回転は未検証です。条件と成果物は [Androidの検証記録](docs/android-verification.md) を参照してください。Androidの制御範囲をiOSへ当てはめず、製品全体は共有ToDo1614で継続します。iOSの調査結果と未達条件は `docs/ios-status.md` に記録しています。

## 検査とAPKの作成

PC上の画面:

```powershell
cargo run --locked --features desktop-preview --bin phone-friction-control-preview
```

検査:

```powershell
cargo nextest run --locked --workspace --features desktop-preview --test-threads 10
cargo test --locked --workspace --features desktop-preview --doc
cargo clippy --locked --workspace --all-targets --features desktop-preview -- -D warnings
cargo clippy --locked --target aarch64-linux-android --lib --no-deps -- -D warnings
```

Rustターゲット、SDK Platform36、Build Tools36.1.0、NDK30.0.14904198、JDKとAndroidの標準デバッグ鍵を用意します。`ANDROID_HOME` と `JAVA_HOME` を設定します。このPCではCargoの共有ゲートと作業先固有の生成先を使います。

```powershell
$env:CARGO_TARGET_DIR = Join-Path (Get-Location) 'target'
$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = "$env:ANDROID_HOME/ndk/30.0.14904198/toolchains/llvm/prebuilt/windows-x86_64/bin/aarch64-linux-android26-clang.cmd"
cargo build --locked --target aarch64-linux-android --lib
cargo run --locked -p android-bridge -- pack . target/aarch64-linux-android/debug/libphone_friction_control.so arm64-v8a <新しい出力フォルダー>
```

仮想端末用には `x86_64-linux-android` のライブラリとABI `x86_64` を使います。`android-bridge` はRustからJVMのサービス入口を生成し、d8でDEXに変換します。Java/Kotlinの製品ソースは追加していません。APKの署名と16KBのZIP配置を作成器が確認します。これはデバッグ可能な試用APKで、正式配布の鍵を使いません。

配布署名の経路は `android-bridge pack-release` です。Rustライブラリは `cargo build --locked --release --target aarch64-linux-android --lib` で作成します。

```text
android-bridge pack-release <プロジェクト> <ライブラリ.so> <ABI> <新しい出力フォルダー> <署名鍵のKeyStore> <鍵の別名> <versionCode> <versionName> <証明書のSHA-256>
```

鍵のパスワードは `PHONE_CONTROL_KS_PASS` と `PHONE_CONTROL_KEY_PASS` の環境変数から署名ツールへ渡します。コマンド引数やログへ値を書きません。指定するSHA-256は、保管している正しい署名証明書から先に確認した64桁の16進数です。生成されたAPKから後付けで採った値を基準にしません。証明書が一致しない場合とAndroidのデバッグ証明書では、完成品を出力しません。APKの署名と16KB配置の確認後にだけ `phone-control-<ABI>-release.apk` を出力します。更新には同じ署名鍵と増加したversionCodeを使います。

使い捨ての試験鍵で更新と拒否条件を確認した後、製品用署名鍵も作成しました。パスワードはこのPCのWindowsユーザーの暗号化機能で保存し、復元を確認しています。その鍵で広告なしのarm64配布APKを作成し、署名・証明書・デバッグ不可・16KB配置を確認しました。一般公開は行っていません。後日の物理端末での私的検査は上記の0.4.2受入の範囲です。`pack-release` は通常の広告なしライブラリ専用です。`ads-test` のライブラリは `pack-ads`、`ads-production` のライブラリは `pack-ads-release` だけが扱います。両機能を同時には指定できません。

### 署名付き広告版の作成

```text
cargo build --locked --release --target <Androidターゲット> --lib --features ads-production
android-bridge pack-ads-release <プロジェクト> <ライブラリ.so> <ABI> <新しい出力フォルダー> <Gradle実行ファイル> <署名鍵のKeyStore> <鍵の別名> <versionCode> <versionName> <証明書のSHA-256> <AdMob AppID> <バナーユニットID>
```

既存の署名鍵・事前に確認した証明書SHA-256と、登録済みの製品IDを指定します。新しい製品鍵やIDを作成する処理はありません。AppIDは `ca-app-pub-<16桁>~<10桁>`、バナーユニットIDは同じpublisherの `ca-app-pub-<16桁>/<10桁>` に限ります。Googleのサンプルpublisherは拒否します。IDはコンパイル時の環境変数から使わず、APKのapplication meta-dataにまとめて配置します。RustはActivityから取得したApplicationInfoのmetaDataを読み、UMP・GMA・バナーに使います。起動後のID未設定・不正は広告だけを停止し、利用制限は継続します。

Gradleが未署名release APKを作り、通常版と共有する署名処理がデバッグ不可・証明書の単一一致・Android Debug証明書の拒否・16KB配置を確認します。パスワードは上記の2つの秘密環境変数を使い、署名診断を表示しません。すべて通った後だけ `phone-control-<ABI>-ads-release.apk` を出力します。既存の出力フォルダーは上書きしません。試験と製品のSOの取り違えも出力フォルダーの作成前に拒否します。

CIの非sample IDは構文検査・APK作成専用のfixtureです。AdMobに登録された実物IDではなく、そのAPKを起動・インストール・実配信に使用しません。実IDの取得、既存製品署名鍵による配布、実SDK・実端末の動作確認は別途必要です。広告は製品版でも任意で、休憩解除の条件にはしません。

## 一次資料

試験広告版は `cargo build --locked --target <Androidターゲット> --lib --features ads-test` の後、`android-bridge pack-ads <プロジェクト> <ライブラリ.so> <ABI> <新しい出力フォルダー> <Gradle実行ファイル>` で作ります。Gradle 8.14.3、AGP 8.13.2を使用し、AARの依存・DEX・リソースを統合します。作成器はSOのコピーだけをstripし、署名と16KB配置を確認します。Googleの試験IDを組み込むデバッグ版のため、製品配布には使いません。

- [UsageStatsManager](https://developer.android.com/reference/android/app/usage/UsageStatsManager)
- [GMA Next-Genのバナー広告](https://developers.google.com/admob/android/next-gen/banner)
- [UsageEvents.Event](https://developer.android.com/reference/android/app/usage/UsageEvents.Event)
- [MediaSessionManager](https://developer.android.com/reference/android/media/session/MediaSessionManager)
- [AccessibilityService](https://developer.android.com/reference/android/accessibilityservice/AccessibilityService)
- [Google PlayのAccessibilityService API利用条件](https://support.google.com/googleplay/android-developer/answer/10964491?hl=en)
- [AppleのFamily Controls配布許可](https://developer.apple.com/documentation/familycontrols/requesting-the-family-controls-entitlement)
- [Android APKの署名と環境変数によるパスワード指定](https://developer.android.com/tools/apksigner)
- [Androidアプリの署名と更新時の鍵の管理](https://developer.android.com/studio/publish/app-signing)

日本語フォントはNoto Sans JPです。ライセンスは `assets/OFL.txt` を参照してください。

## ライセンスと配布範囲

AutoDevJapanが権利を持つ自作コードは [Apache License 2.0](LICENSE) で提供します。著作権表示は [NOTICE](NOTICE) を参照してください。これは「Apache-2.0」であり、将来版への自動変更を含みません。

Rust依存ライブラリ、Android/iOS SDK、Google GMA・UMP、フォント、画像などの第三者著作物には、それぞれの権利者のライセンスが適用されます。このライセンスはSDKの再配布、Google Playへの掲載、AppleのFamily Controls配布許可、広告配信の承認を与えるものではありません。

製品ソースは承認されたOSS公開範囲として、この公開リポジトリで提供しています。ソースの公開と、署名付きAPK・ストア配布の受入条件は別です。OSS公開はストア掲載、有料販売、広告の実課金を承認するものではありません。署名鍵・認証情報を配布物へ含めません。既存素材の削除や履歴書換えは行っていません。
