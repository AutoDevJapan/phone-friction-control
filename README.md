# 利用制限

成人が自分で決めたアプリ利用の上限と休憩時間を管理する、Android向けRust試作です。承認済みの画面案に合わせ、ホーム、記録、設定、休憩中の画面を用意しています。

## できること

- ホーム画面から起動できるアプリを選び、端末内で前面利用時間を集計する
- 1日の上限を超えた後、休憩画面を設定時間表示する
- Androidのホーム画面に設定した場合、ホームボタンで戻った時点で利用時間を確認する
- 選んだアプリ名と日ごとの合計時間、休憩時間を端末内へ保存する
- 広告枠は制御アプリ内だけに置く想定とし、休憩画面には広告を出さない

## いまの制御範囲

この試作は、Androidのホーム画面から対象アプリを開く使い方を前提にしています。ホームボタンで戻った時に上限を確認するため、上限を超えた瞬間に対象アプリの上へ画面を重ねる機能はありません。通知や外部リンクから直接開いたアプリも制御しません。

AndroidのUsageStats APIは利用時間を取得します。一般アプリから他社アプリを停止するDevicePolicyManager APIは使えないため、現段階ではホーム画面に戻った時点で休憩画面を表示します。Accessibility Serviceを使う強い制御は、Playの申告・開示要件を確認した別の実装段階が必要です。

iOSの画面時間制御、AppleのFamily Controls配布許可、広告配信SDKと広告契約は含みません。広告配信は接続しておらず、インターネット権限も宣言していません。

## Androidの準備

1. アプリの設定画面で利用時間の説明を確認し、Androidの「使用状況へのアクセス」を許可します。
2. 休憩をホームボタンから適用するには、Androidの「ホームアプリ」設定でこのアプリを選びます。
3. 制限するアプリと、1日の上限、休憩時間を設定します。

使用状況へのアクセスが未許可でも、広告表示や別の権限付与を要求せず、制御が動かないことを画面に示します。

設定、選択アプリ、日ごとの記録、休憩の終了時刻はアプリ専用の保存先へ書き込みます。一時ファイルの書き込みと同期が成功した後に置き換えるため、途中で終了しても以前の正常な記録を残します。休憩中に日付が変わっても、元の休憩時間を短くしません。

取得するのはAndroidが集計した前面利用時間です。動画を再生していた時間そのものではありません。OSが集計範囲を日単位へ広げる場合があるため、厳密な視聴時間の判定には別の計測実装が必要です。

## Androidの仕様資料

- [UsageStatsManager](https://developer.android.com/reference/android/app/usage/UsageStatsManager)
- [DevicePolicyManager.setPackagesSuspended](https://developer.android.com/reference/android/app/admin/DevicePolicyManager#setPackagesSuspended(android.content.ComponentName,java.lang.String[],boolean))
- [Google Play: AccessibilityService API の利用条件](https://support.google.com/googleplay/android-developer/answer/10964491?hl=en)
- [Apple: Family Controls entitlement の申請](https://developer.apple.com/documentation/familycontrols/requesting-the-family-controls-entitlement)

## プレビュー

PC上の画面プレビューは次のコマンドで起動します。

```powershell
cargo run --features desktop-preview --bin phone-friction-control-preview
```

Android APKはAndroid SDKのAAPT2、zipalign、apksignerとNDKで作成できます。配布用の署名鍵、広告SDK、APIキーはリポジトリへ置きません。

### arm64の試用APKを作る

Rustの `aarch64-linux-android` ターゲット、SDK Platform 36、Build Tools 36.1.0、NDK、JDKを用意します。以下はPowerShellでの一回の作成例です。`$out` は新しい作業用フォルダー、`$target` はCargoの生成先を指定してください。デバッグ鍵はAndroid Studioが作成した試験用のものを使用し、公開用の署名には使用しません。

```powershell
$ndk = "$env:ANDROID_HOME/ndk/30.0.14904198/toolchains/llvm/prebuilt/windows-x86_64/bin"
$tools = "$env:ANDROID_HOME/build-tools/36.1.0"
$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = "$ndk/aarch64-linux-android26-clang.cmd"
cargo build --locked --target aarch64-linux-android --lib
New-Item -ItemType Directory -Path "$out/staging/lib/arm64-v8a" -Force
Copy-Item "$target/aarch64-linux-android/debug/libphone_friction_control.so" "$out/staging/lib/arm64-v8a/libphone_friction_control.so"
& "$ndk/llvm-strip.exe" --strip-unneeded "$out/staging/lib/arm64-v8a/libphone_friction_control.so"
& "$tools/aapt2.exe" link -o "$out/unsigned.apk" --manifest android/AndroidManifest.xml -I "$env:ANDROID_HOME/platforms/android-36/android.jar" --version-code 1 --version-name 0.1.0 --debug-mode
& "$env:JAVA_HOME/bin/jar.exe" --update --file "$out/unsigned.apk" --no-compress -C "$out/staging" lib/arm64-v8a/libphone_friction_control.so
& "$tools/zipalign.exe" -P 16 -f 4 "$out/unsigned.apk" "$out/aligned.apk"
& "$env:JAVA_HOME/bin/java.exe" -jar "$tools/lib/apksigner.jar" sign --ks "$env:USERPROFILE/.android/debug.keystore" --ks-key-alias androiddebugkey --ks-pass pass:android --key-pass pass:android --out "$out/phone-friction-control-arm64-debug.apk" "$out/aligned.apk"
& "$env:JAVA_HOME/bin/java.exe" -jar "$tools/lib/apksigner.jar" verify --verbose "$out/phone-friction-control-arm64-debug.apk"
& "$tools/zipalign.exe" -c -P 16 4 "$out/phone-friction-control-arm64-debug.apk"
```

各コマンドの終了コードが0であることを確認してから次へ進みます。APKはデバッグ可能な試用版です。正式配布の署名、広告配信、iOS版、実端末の検証は別途必要です。

### 回帰検査

```powershell
cargo test --locked --features desktop-preview --lib
cargo clippy --locked --target aarch64-linux-android --lib --no-deps -- -D warnings
```

日付をまたぐ休憩、14日分の記録、権限解除時の記録保持、休憩終了後の再利用、保存内容の往復、途中の書き込み失敗と再保存を検査します。

## フォント

日本語表示にNoto Sans JPを使用します。ライセンスは `assets/OFL.txt` を参照してください。
