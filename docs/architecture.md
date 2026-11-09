# アーキテクチャの選び方

ファイルの一覧は[README](../README.md#ダウンロード)にあります。

## BonDriver_coax

BonDriver_coaxは、読み込むソフト（TVTestやEDCB）と同じアーキテクチャのものを選びます。
64bitのWindowsでも32bitのTVTestなら32bit版、arm64のWindowsでもx64のTVTestならx64版です。

Linux版はglibc 2.17以降で動きます。

## coaxd

BonDriver_coaxと`coaxd`はソケットでつなぐので、アーキテクチャが違っても動きます。
64bitのWindowsで32bitのTVTestを使う場合も、arm64のWindowsでx64のTVTestを使う場合も、`coaxd`はTVTestにそろえても、OSに合わせても構いません。
`coaxd`は別のプロセスなので、動作中のOSに合ったものを使うのがおすすめです。

ただし、`coaxd`が読み込むメーカーのDLLは、`coaxd`と同じアーキテクチャのものが要ります。
PT1・PT2・PT3のSDKとWinDriverのDLL、BDAのチューナーのフィルターがこれにあたります。該当するチューナーを利用する場合、ベンダーが配っていないアーキテクチャの`coaxd`では、そのチューナーを利用できません（例: PT3はARM64 Windows向けドライバがないため利用できません）。
カーネルのドライバーはOSと同じアーキテクチャのものが要るので、x64用のドライバーしか無いチューナーはarm64のWindowsでは使えません。

`coaxd`がWinUSBで直接制御するチューナーは、メーカーのドライバーもDLLも使わないので、どのアーキテクチャの`coaxd`でも使えます。
