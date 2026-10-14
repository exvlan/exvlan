# GUI for Exvlan

this is a GUI implementation for Exvlan, based on Tauri2.

## Compile

### Install prerequisites

```
apt install npm
npm install -g pnpm
```

### For Desktop (Win/Mac/Linux)

```
cd ../tauri-plugin-vpnservice
pnpm install
pnpm build

cd ../std
pnpm install
pnpm build

cd ../../exvlan-gui
pnpm install
pnpm tauri build
```

### For Android

Need to install android SDK / emulator / NDK / Java (easy with android studio)

```
# For ArchLinux
sudo pacman -Sy sdkmanager
sudo sdkmanager --install platform-tools platforms\;android-34 ndk\;r26 build-tools
export PATH=/opt/android-sdk/platform-tools:$PATH
export ANDROID_HOME=/opt/android-sdk/
export NDK_HOME=/opt/android-sdk/ndk/26.0.10792818/
rustup target add aarch64-linux-android

install java 20
```

Java version depend on gradle version specified in (exvlan-gui\src-tauri\gen\android\build.gradle.kts)

See [Gradle compatibility matrix](https://docs.gradle.org/current/userguide/compatibility.html) for detail .

```
pnpm install
pnpm tauri android init
pnpm tauri android build
```
