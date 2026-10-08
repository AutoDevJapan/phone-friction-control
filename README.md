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

その日の上限で休憩を開始し、同じ日の休憩は一度だけです。日をまたぐ既存の休憩を短くしません。期限は端末の時刻を使います。時計の変更を阻止する製品ではありません。休憩中の選択アプリへの再入場も、サービスが前面のアプリを検出して休憩画面へ切り替えます。OSや機種によるサービスの再接続・背景起動の違いは実機検証が必要です。

通知アクセスがある場合、休憩中は選んだアプリのPLAYINGセッションだけに一時停止を要求します。画面消灯や背景再生でも要求します。非対象のセッションへは要求を送りません。対象が要求に応じない場合やセッションを公開しない場合は再生を止められず、休憩画面への切替だけです。画面を切り替えたことを再生停止の証拠として扱いません。

## 保存と広告

アプリ名、利用者の設定、日ごとの合計利用時間と合計再生時間、休憩期限を端末内だけに保存します。画面の文字・画像・操作内容、通知本文、動画の題名を読みません。インターネット権限や広告SDKは追加していません。

利用時間の更新は10秒ごと、休憩期限や設定変更はすぐに保存します。サービスと画面は別の保存先を使い、画面の更新には同一プロセス内の共有状態を使います。保存に失敗した場合もメモリ上の休憩は続け、失敗を画面へ示します。読めないファイルを空の状態で上書きしません。状態を読めない間も、新しく観測した時間と設定に残った休憩はメモリ上で制御します。合計は取得不能として表示し、5秒ごとに読み直します。読み取りが回復した場合は、重複するOS利用時間と、新しく観測した再生時間を区別して前の記録へ統合します。

広告枠は任意表示で、このアプリ内に限ります。休憩画面へ置かず、広告視聴・拒否・読み込み失敗によって休憩を解除しません。広告配信・広告事業者との接続は未実装です。

iOS版、Family Controls配布許可、正式配布の署名、実端末での検証も未完了です。Androidの制御範囲をiOSへ当てはめません。これらは共有ToDo1614の未達条件として保持しています。

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

## 一次資料

- [UsageStatsManager](https://developer.android.com/reference/android/app/usage/UsageStatsManager)
- [UsageEvents.Event](https://developer.android.com/reference/android/app/usage/UsageEvents.Event)
- [MediaSessionManager](https://developer.android.com/reference/android/media/session/MediaSessionManager)
- [AccessibilityService](https://developer.android.com/reference/android/accessibilityservice/AccessibilityService)
- [Google PlayのAccessibilityService API利用条件](https://support.google.com/googleplay/android-developer/answer/10964491?hl=en)
- [AppleのFamily Controls配布許可](https://developer.apple.com/documentation/familycontrols/requesting-the-family-controls-entitlement)

日本語フォントはNoto Sans JPです。ライセンスは `assets/OFL.txt` を参照してください。
