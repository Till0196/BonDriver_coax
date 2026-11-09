# BonDriver_coax

[coax](https://github.com/Till0196/coax)の`coaxd`のチューナーを、TVTestやEDCBからBonDriverとして使うためのものです。
BonDriver本体、デバイス設定やチャンネル定義を作成するTVTestプラグイン、カードリーダー用の`winscard.dll`の3つでできています。

## ダウンロード

[BonDriver_coaxのリリース](https://github.com/Till0196/BonDriver_coax/releases/latest)と、[coaxのリリース](https://github.com/Till0196/coax/releases/latest)の両方が必要です。
BonDriver_coaxは、読み込むソフト（TVTestやEDCB）と同じアーキテクチャのものを選びます。

| 使うソフト | ファイル | 中身 |
|---|---|---|
| x64のTVTest、EDCB | `BonDriver_coax-x86_64-pc-windows-gnullvm.zip` | `BonDriver_coax.dll`、`winscard.dll`、`Plugins\coax.tvtp` |
| 32bitのTVTest、EDCB | `BonDriver_coax-i686-pc-windows-gnullvm.zip` | 同上 |
| Linux版のEDCB（x86_64） | `BonDriver_coax-x86_64-unknown-linux-gnu.tar.gz` | `BonDriver_coax.so` |
| Linux版のEDCB（arm64） | `BonDriver_coax-aarch64-unknown-linux-gnu.tar.gz` | 同上 |
| Linux版のEDCB（armhf） | `BonDriver_coax-armv7-unknown-linux-gnueabihf.tar.gz` | 同上 |
| Linux版のEDCB（32bit x86） | `BonDriver_coax-i686-unknown-linux-gnu.tar.gz` | 同上 |

`coaxd`は、OSに合ったものを選びます。

| OS | ファイル |
|---|---|
| Windows（x64） | `coax_Windows_amd64.zip` |
| Windows（arm64） | `coax_Windows_arm64.zip` |
| Windows（32bit） | `coax_Windows_i386.zip` |
| Linux | `coax_Linux_amd64.tar.gz`、`coax_Linux_arm64.tar.gz`、`coax_Linux_armv7.tar.gz`、`coax_Linux_i386.tar.gz`（`.deb`、`.rpm`、`.apk`もあります） |

アーキテクチャを混ぜて使うときの注意は[アーキテクチャの選び方](docs/architecture.md)にあります。

## 入れ方（TVTest）

TVTestのフォルダーに次のように置きます。

```
TVTest.exe
BonDriver_coax.dll
winscard.dll
coaxd.exe              ← coax の coaxd。ここに置けば必要なときに自動で起動します
firmware\              ← coax の利用時にファームウェアが必要なデバイスはこのフォルダに設置します
Plugins\
  coax.tvtp            ← coax_plugin.dll の名前を変えたもの
```

`coaxd`はcoaxのリリースから入手してください。
ファームウェアの入手と置き方は[coaxの説明](https://github.com/Till0196/coax/blob/main/docs/setup/firmware.md)にあります。

あとはプラグインに従ってください。

1. TVTestでBonDriverに`BonDriver_coax.dll`を選びます。
2. 「プラグイン」メニューの「coax」でウィンドウを開きます。
3. 「経路と空間の設定...」で、どのチューナーで何（地デジ、BS/CS、ケーブルなど）を受けるかを決めます。
4. 「チャンネルスキャン」を押します。受かったチャンネルが設定ファイルに書かれます。
5. 最後にTVTestのチャンネルスキャンをします。

設定はDLLの隣の`BonDriver_coax.ini`に入ります。
手動で編集する必要はなく、「設定ファイルを開く」で内容を確認できます。

`winscard.dll`は、チューナーに内蔵のカードスロットをTVTestからカードリーダーとして使えるようにします。
外付けのカードリーダーもそのまま使えます。
内蔵スロットが無い機器なら置かなくても構いません。

BonDriverと`coaxd.exe`をサブフォルダーに置く場合は、`winscard.ini`で`coaxd.exe`の場所を指定してください。
設定の項目とログの取り方は[設定](docs/settings.md)にあります。

## EDCBで使う

`BonDriver_coax.dll`（Linux版のEDCBなら`BonDriver_coax.so`）をEDCBのBonDriverフォルダーに置きます。
EDCBは`BonDriver_coax(…).ChSet4.txt`が無いとチューナーとして一覧に表示しないので、最初に`EpgDataCap_Bon`でチャンネルスキャンをしてください。
設定ファイルはTVTestで作ったものをそのまま使えます。
Linux版のEDCBでの設定とカードの使い方は[Linux版のEDCB](docs/settings.md#linux版のedcb)にあります。
1チューナーに1つのDLLを割り当てる方法は[DLLを複製して使う](docs/settings.md#dllを複製して使う)にあります。

## ドキュメント

| | |
|---|---|
| [アーキテクチャの選び方](docs/architecture.md) | BonDriver_coaxと`coaxd`のアーキテクチャを混ぜて使うときの注意 |
| [設定](docs/settings.md) | 設定ファイルの項目、DLLの複製、Linux版のEDCB、ログの取り方 |
| [ビルド](docs/build.md) | ソースからの組み方と`bondriver-probe` |

## ライセンス

Apache-2.0です。
全文は[LICENSE](LICENSE)にあります。
