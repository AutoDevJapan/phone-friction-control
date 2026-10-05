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

Android APKの作成にはAndroid NDKと `cargo-apk2` が必要です。配布用の署名鍵、広告SDK、APIキーはリポジトリへ置きません。

## フォント

日本語表示にNoto Sans JPを使用します。ライセンスは `assets/OFL.txt` を参照してください。
