# ビルド

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
