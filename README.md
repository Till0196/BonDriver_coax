# BonDriver_coax

[coax](https://github.com/Till0196/coax)の`coaxd`のチューナーを、TVTestやEDCBからBonDriverとして使うためのものです。
BonDriver本体、デバイス設定やチャンネル定義を作成するTVTestプラグイン、カードリーダー用の`winscard.dll`の3つでできています。

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

32bitのTVTestには32bit版、64bitには64bit版を使ってください。

## EDCBで使う

`BonDriver_coax.dll`（Linux版のEDCBなら`BonDriver_coax.so`）をEDCBのBonDriverフォルダーに置きます。
EDCBは`BonDriver_coax(…).ChSet4.txt`が無いとチューナーとして一覧に表示しないので、最初に`EpgDataCap_Bon`でチャンネルスキャンをしてください。
設定ファイルはTVTestで作ったものをそのまま使えます。

## ビルド

そのOSの上でそのOS向けに組むなら、rustupだけで組めます（WindowsはVisual StudioのC++ビルドツールも）。

```sh
cargo build --release                    # 64bit Windows（MSVC）
cargo build --release -p bondriver_coax  # Linux版のEDCB（組んだ機械のglibc以降で動く）
```

クロスビルドはzigを使います（32bit Windowsや、古いglibcのボード向けも）。PATHの通った`zig`と`cargo install cargo-zigbuild`があれば、どのOSからでも同じ手順で組めます。

```sh
cargo win          # 64bit Windows
cargo win32        # 32bit Windows
cargo linux        # Linux版のEDCB（glibc 2.17以降。arm64 は linux-arm64、armhf は linux-armhf）
```

成果物は`target/release/`（ターゲットを指定したときは`target/<ターゲット>/release/`）に出ます。
Linux版は`libBonDriver_coax.so`として出るので、`BonDriver_coax.so`に名前を変えて置いてください。

動作の確認には、ホストと同じ順でBonDriverを呼ぶ`bondriver-probe`が使えます。

```sh
bondriver-probe BonDriver_coax.dll              # チャンネルを列挙する
bondriver-probe BonDriver_coax.dll 0 3 out.ts   # 空間0のチャンネル3を受けて書き出す
```

## ライセンス

Apache-2.0です。
全文は[LICENSE](LICENSE)にあります。
