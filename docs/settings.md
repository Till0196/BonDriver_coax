# 設定

- [設定ファイル](#設定ファイル)
- [[coax]](#coax)
- [[Tuners]](#tuners)
- [[Spaces]](#spaces)
- [[TunerSettings]](#tunersettings)
- [[Channels 空間名]](#channels-空間名)
- [winscard.ini](#winscardini)
- [DLLを複製して使う](#dllを複製して使う)
- [Linux版のEDCB](#linux版のedcb)
- [環境変数](#環境変数)
- [困ったとき](#困ったとき)

## 設定ファイル

DLLは、自分と同じフォルダーにある同じ名前の`.ini`を読みます。
ファイルが無ければ既定の値で動きます。

| DLL | 設定ファイル |
|---|---|
| `BonDriver_coax.dll` | `BonDriver_coax.ini` |
| `BonDriver_coax-PT3-S.dll` | `BonDriver_coax-PT3-S.ini`、無ければ最後の`-`より前の`BonDriver_coax-PT3.ini` |
| `winscard.dll` | `winscard.ini` |

`BonDriver_coax.ini`のほとんどはTVTestプラグイン(coax.tvtp)から設定できます。

書式は次の通りです。

- 文字コードはUTF-8か、Windowsの既定のコードページ（日本語ならShift_JIS）です。
- `;`か`#`で始まる行はコメントです。値の後ろに書くときは、前に空白を入れます。
- セクション名とキー名は大文字・小文字を区別しません。
- 値の中の環境変数は展開しません。

## [coax]

`coaxd`への接続です。

| キー | 値 | 既定 |
|---|---|---|
| `Host` | `coaxd`のソケットのパス、または`ホスト:ポート` | [環境変数](#環境変数)の`COAX_HOST`、`COAXD_SOCKET`、無ければ`%LOCALAPPDATA%\coaxd\http.sock` |
| `Daemon` | 起動する`coaxd.exe`のパス。相対パスはDLLのフォルダーから | 下の順で探す |
| `StartDaemon` | `coaxd`が応答しないときに起動する（`1`/`0`） | `1` |
| `Tuner` | 使うチューナー。`[Tuners]`が無いときに使います | `coaxd`が選ぶ |
| `Priority` | チューナーを取り合うときの優先度。大きい方が取ります | `0` |

```ini
[coax]
Daemon = coaxd.exe
StartDaemon = 1
Priority = 10
```

`coaxd.exe`は次の順で探します。

1. `Daemon`
2. 環境変数`COAXD`（絶対パスのみ）
3. DLLと同じフォルダー
4. `PATH`

`Host`が`ホスト:ポート`のときは起動しません。

## [Tuners]

方式とチューナーの組に名前を付けます。

```ini
[Tuners]
isdbt = ISDBT, pt3:*, bands=UHF
isdbs = ISDBS, pt3:*
cable = ISDBC, model:TBS 6205*, bands=VHF,UHF,EXUHF,CATV
```

`名前 = 方式, チューナー..., bands=帯域...`

| 項目 | 値 | 既定 |
|---|---|---|
| 方式 | `ISDBT`、`ISDBS`、`ISDBS3`、`ISDBC`、`J83B_5_6`、`DVBS`、`DVBS2`など | 必須 |
| チューナー | チューナーの識別子。末尾の`*`で前方一致、`model:`で機種名。方式を受けられないチューナーは除きます | `coaxd`が選ぶ |
| `bands` | 地上波とケーブルで回す帯域。`UHF`、`EXUHF`、`CATV`、`VHF` | 地上波は`UHF,CATV,EXUHF`、ケーブルは`VHF`も |

BonDriver_coaxは、ここに書いたチューナーだけを使います。
`coaxd`の一覧に出ていても、書いていないチューナーは開きません。
そのため、同じPCで一部のチューナーをほかのBonDriverで使っていても競合しません。

ただし、チューナーを書かなかった場合と、`*`で書いたパターンに一致するチューナーが1つも無い場合は、空きチューナーを`coaxd`が自動で選択します。
**ほかのBonDriver実装と分けて使うときは、識別子をそのまま書くか、確実に一致するパターンで書いてください。**

## [Spaces]

チューニング空間を、並べる順に書きます。

```ini
[Spaces]
0 = 地デジ, isdbt, cable
1 = BS, isdbs, cable
2 = CS, isdbs, cable
```

`番号 = 名前, [Tuners]の名前..., group=種類`

- `[Tuners]`の名前は、書いた順に試します。
- `group`は、名前から種類（`BS`、`CS`、`BS4K`など）が分からないときに書きます。
- `[Spaces]`が無ければ、「地デジ」の1つになります。

## [TunerSettings]

チューナーごとの設定です。

```ini
[TunerSettings]
pt3:6-17-0-0:0    = lnb=high
model:TBS 6205se* = gain=auto
px3:p1-14.1:0     = loop_through=low
```

`チューナー = 項目=値, ...`

チューナーは`[Tuners]`と同じ書き方です。

| 項目 | 値 |
|---|---|
| `lnb` | `off`、`low`、`high` |
| `gain` | `auto`、`min`、`max`、`+13dB`など |
| `loop_through` | `max`、`high`、`mid`、`low` |

チューナーに無い項目は無視します。

## [Channels 空間名]

プラグインのチャンネルスキャンの結果です。
スキャンのたびに書き直されます。

この節が無い空間に出るチャンネルは、方式によって違います。

| 方式 | 節が無いとき |
|---|---|
| 地上波（`ISDBT`）、ケーブル（`ISDBC`）、光（`J83B_5_6`） | 内蔵のチャンネルマップの物理チャンネルを全部出します |
| 124/128度のCS（`DVBS`、`DVBS2`） | 内蔵のトランスポンダーの一覧を出します |
| BS・110度CS（`ISDBS`、`ISDBS3`） | 何も出ません |

BS・110度CSは、1つのトランスポンダーに複数のTSが載っていて、どのTSがあるかは受けてみないと分からないためです。
衛星の空間は、プラグインでチャンネルスキャンをするか、別のPCで作った`.ini`をコピーしてください。

## winscard.ini

`winscard.dll`は`[coax]`の`Host`、`Daemon`、`StartDaemon`だけを読みます。
`winscard.dll`と`coaxd.exe`が同じフォルダーにあれば、`winscard.ini`を用意する必要はありません。

BonDriverと`coaxd.exe`をサブフォルダーに置くときは、`Daemon`で場所を指定します。

```
C:\TVTest\
├─ TVTest.exe
├─ winscard.dll
├─ winscard.ini
└─ BonDriver\
   ├─ BonDriver_coax.dll
   └─ coaxd.exe
```

```ini
; winscard.ini
[coax]
Daemon = BonDriver\coaxd.exe
```

`Host`を書くときは、`BonDriver_coax.ini`と同じにしてください。

## DLLを複製して使う

`BonDriver_coax.dll`を`BonDriver_coax-名前.dll`の形で複製すると、複製ごとに別の設定を持てます。
地上波と衛星、チューナーごとなどにDLLを分けるときに使います。

複製するのは`BonDriver_coax.dll`だけです。
`coaxd.exe`と`winscard.dll`は名前を変えず、1つのままにします。

- `coaxd`は、PCのすべてのチューナーをまとめて扱います。どの複製も同じ`coaxd`につなぎ、`coaxd`が空いているチューナーを割り当てます。`coaxd`は1台のPCで1つしか動きません。
- `winscard.dll`は、TVTestやEpgDataCap_BonがWindowsの`winscard.dll`の代わりに読むものです。名前を変えると読まれません。どのBonDriverを使っていても、同じ`coaxd`からカードを読みます。

あらかじめTVTestのプラグインで設定してから、DLLと`.ini`をEDCBのBonDriverフォルダーにコピーすると楽です。

1. TVTestのフォルダーで`BonDriver_coax.dll`を複製し、同じ名前の空の`.ini`も作ります。
   `.ini`が無いと、名前の最後の`-`より前の`.ini`を読み書きします。
2. TVTestで複製したDLLを開き、プラグインの「経路と空間の設定...」とチャンネルスキャンをします。
3. DLLと`.ini`をEDCBのBonDriverフォルダーにコピーし、`EpgDataCap_Bon`でチャンネルスキャンをします。

プラグインの「設定ファイル」で、編集するファイルを選べます。

以下はPT3を2枚つないだ場合の例です。
ほかのチューナーでも、識別子を置き換えれば同じように書けます。
識別子はプラグインの「経路と空間の設定...」に出ます。
ここでの値は説明のためのもので、実際の値はPCによって違います。

PT3は1枚に衛星と地上波のチューナーが2つずつあります。
識別子は`pt3:バス-デバイス-ファンクション-番号:0`で、PCI上の位置から作ります。
番号は0・1が衛星、2・3が地上波です。
カードを挿すスロットを変えたり、PCIeのデバイスを増やしたりすると、変わることがあります。

```
pt3:6-17-0-0:0  ISDB-S  1枚目の衛星1
pt3:6-17-0-1:0  ISDB-S  1枚目の衛星2
pt3:6-17-0-2:0  ISDB-T  1枚目の地上波1
pt3:6-17-0-3:0  ISDB-T  1枚目の地上波2
pt3:7-0-0-0:0   ISDB-S  2枚目の衛星1
...
```

### 空いているチューナーを使う

| DLL | 使うチューナー |
|---|---|
| `BonDriver_coax-PT3-T.dll` | すべてのPT3の地上波 |
| `BonDriver_coax-PT3-S.dll` | すべてのPT3の衛星 |

```ini
; BonDriver_coax-PT3-T.ini
[Tuners]
isdbt = ISDBT, pt3:*

[Spaces]
0 = 地デジ, isdbt
```

```ini
; BonDriver_coax-PT3-S.ini
[Tuners]
isdbs = ISDBS, pt3:*

[Spaces]
0 = BS, isdbs
1 = CS, isdbs
```

`pt3:*`はすべてのPT3に一致し、そのうち経路の方式を受けられるものだけが使われます。
空いているものから順に使うので、EDCBのチューナー数は地上波・衛星それぞれの合計（PT3が2枚なら4）にします。

上の`.ini`は経路と空間だけです。
`BonDriver_coax-PT3-S`は、プラグインでチャンネルスキャンをして`[Channels BS]`などを書き足すまで、チャンネルが出ません（[[Channels 空間名]](#channels-空間名)）。

### チューナーを固定する

| DLL | 使うチューナー |
|---|---|
| `BonDriver_coax-PT3-T0.dll` | `pt3:6-17-0-2:0` |
| `BonDriver_coax-PT3-T1.dll` | `pt3:6-17-0-3:0` |
| `BonDriver_coax-PT3-T2.dll` | `pt3:7-0-0-2:0` |
| `BonDriver_coax-PT3-T3.dll` | `pt3:7-0-0-3:0` |
| `BonDriver_coax-PT3-S0.dll` | `pt3:6-17-0-0:0` |
| `BonDriver_coax-PT3-S1.dll` | `pt3:6-17-0-1:0` |
| `BonDriver_coax-PT3-S2.dll` | `pt3:7-0-0-0:0` |
| `BonDriver_coax-PT3-S3.dll` | `pt3:7-0-0-1:0` |

```ini
; BonDriver_coax-PT3-T0.ini
[Tuners]
isdbt = ISDBT, pt3:6-17-0-2:0

[Spaces]
0 = 地デジ, isdbt
```

```ini
; BonDriver_coax-PT3-S0.ini
[Tuners]
isdbs = ISDBS, pt3:6-17-0-0:0

[Spaces]
0 = BS, isdbs
1 = CS, isdbs
```

1枚の中は`pt3:6-17-0-`のように前の部分で書けます。

```ini
; 1枚目の地上波だけを使う場合
[Tuners]
isdbt = ISDBT, pt3:6-17-0-*
```

## Linux版のEDCB

`BonDriver_coax.so`をEDCBのBonDriverフォルダーに置きます。
設定ファイルは`.so`と同じフォルダーの同じ名前の`.ini`で、書き方はWindowsと同じです。
複製するときも、`BonDriver_coax-PT3-T.so`のように`.so`を複製します。

Windowsと違うのは次の点です。

| | Windows | Linux |
|---|---|---|
| 設定の作り方 | TVTestのプラグイン | 手動で書くか、Windowsで作った`.ini`をコピー |
| `coaxd`の置き場所 | DLLと同じフォルダー | `.so`と同じフォルダー、`PATH`、`Daemon`のどれか。常駐させても構いません |
| 既定のソケット | `%LOCALAPPDATA%\coaxd\http.sock` | rootなら`/run/coaxd/http.sock`、それ以外は`$XDG_RUNTIME_DIR/coaxd/http.sock` |
| カード | `winscard.dll` | `coax pcsc` |

Linuxにはプラグインが無いので、BS・110度CSのチャンネルはスキャンできません。
Windowsで作った`.ini`をコピーすると、`[Channels BS]`などもそのまま使えます。

チューナーの識別子はOSで違います。
Windowsの`.ini`をコピーしたら、`[Tuners]`と`[TunerSettings]`の識別子を、`coax list`に出るLinuxの識別子に書き換えてください。

```ini
; Linux（pt3ドライバー）の例
[Tuners]
isdbt = ISDBT, chardev:pt3video*
isdbs = ISDBS, chardev:pt3video*
```

EDCBを別のユーザーやsystemdのサービスで動かす場合は、ソケットの場所がそのユーザーのものになります。
`coaxd`を別に動かしているなら、`[coax]`の`Host`でそのソケットを指してください。

カードは`coax pcsc`がpcscdの代わりのソケットを作り、EDCBはpcsc-liteを通してそこにつなぎます。
`coax pcsc --idle 0`を常駐させ、EDCBの環境変数`PCSCLITE_CSOCK_NAME`にそのソケットを指定します。

```sh
coax pcsc --idle 0
# serving PC/SC on /run/user/1000/coaxd/pcscd.comm
```

```ini
# systemdのユニットの例
[Service]
Environment=PCSCLITE_CSOCK_NAME=/run/user/1000/coaxd/pcscd.comm
```

詳しくはcoaxの[カードリーダー](https://github.com/Till0196/coax/blob/main/docs/reference/card-reader.md)にあります。

## 環境変数

| 環境変数 | 意味 |
|---|---|
| `COAX_HOST` | `Host`の既定。ソケットのパスか`ホスト:ポート` |
| `COAXD_SOCKET` | `Host`の既定。ソケットのパス |
| `COAXD_SOCKET_DIR` | 既定のソケットを置くフォルダー |
| `COAXD` | `coaxd.exe`のパス（絶対パス） |
| `COAX_TRACE` | DLLとプラグインのログを書くファイル |
| `COAXD_LOG` | `coaxd`のログを書くファイル。DLLが起動した`coaxd`にも引き継がれます |

## 困ったとき

### 起動して最初だけカードが読めない

`winscard.dll`が`coaxd.exe`を見つけられていません。
カードを開くプラグインはBonDriverより先に`winscard.dll`を呼ぶので、そこで「カードリーダーが無い」になります。
その後でBonDriverが`coaxd`を起動するので、2回目からは読めます。

[winscard.ini](#winscardini)の`Daemon`を設定してください。
`COAX_TRACE`のログには`no coaxd.exe to start`と出ます。

### ログを取る

`coaxd.exe`を終了してから、環境変数を付けてTVTestを起動します。

```bat
@echo off
set COAX_TRACE=%USERPROFILE%\Desktop\coax-trace.log
set COAXD_LOG=%USERPROFILE%\Desktop\coaxd.log
start "" "%~dp0TVTest.exe"
```

| ファイル | 中身 |
|---|---|
| `coax-trace.log` | `coaxd`の起動、カードリーダーの一覧、通信の失敗など |
| `coaxd.log` | デバイスを開いた結果、カードのリセット、返したエラーなど |
