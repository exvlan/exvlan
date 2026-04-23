#!/bin/bash

# 配置变量
KEYSTORE_FILE="keys/x.jks"
CN="era.xvlan.studio" # 证书持有者名称

if [ -f .env ]; then
    echo "加载 .env 配置文件..."
    # 过滤注释并导出变量
    export $(grep -v '^#' .env | xargs)
else
    echo "❌ 错误: 未找到 .env 文件，请确保它存在并包含必要的变量"
    exit 1
fi


PASSWORD="$KEYSTORE_PASSWORD"
RELEASE_ALIAS="$KEYSTORE_ALIAS"
UPLOADED_ALIAS="$UPLOADED_KEYSTORE_ALIAS"
echo "RELEASE_ALIAS: $RELEASE_ALIAS"
echo "UPLOADED_ALIAS: $UPLOADED_ALIAS"

UNSIGNED_APK="./packages/studio/src-tauri/gen/android/app/build/outputs/apk/arm64/release/app-arm64-release-unsigned.apk"
SIGNED_APK="./release/exvlan-release-latest.apk"
mkdir -p ./release ./artifacts
echo "🚀 开始处理 APK 签名..."

# 1. 检查并生成 Keystore (如果不存在)
if [ ! -f "$KEYSTORE_FILE" ]; then
    # 首先必须关闭 NTP 自动同步，否则无法手动改时间
    timedatectl set-ntp false
    # 手动设置具体时间 (格式: YYYY-MM-DD HH:MM:SS)
    timedatectl set-time "2026-01-01 00:00:00"
    echo "$(date) 🔑 未找到 Keystore，正在生成新密钥... "
    keytool -genkey -v \
      -keystore "$KEYSTORE_FILE" \
      -alias "$RELEASE_ALIAS" \
      -keyalg EC \
      -groupname secp521r1 \
      -sigalg SHA512withECDSA \
      -validity 20090 \
      -storepass "$PASSWORD" \
      -keypass "$PASSWORD" \
      -storetype PKCS12 \
      -dname "CN=$CN, OU=Era X Development, O=Era X Team, L=Gui Yang, S=Gui Zhou, C=CN"

    # 改完后重新开启 NTP 自动同步
    sudo timedatectl set-ntp true
    echo "$(date) ✅ Keystore 已生成: $KEYSTORE_FILE"
fi
export KEYSTORE_ALIAS=$RELEASE_ALIAS
export KEYSTORE_PASSWORD=$PASSWORD
export ANDROID_KEYSTORE="$(pwd)/$KEYSTORE_FILE"

echo export KEYSTORE_ALIAS=$RELEASE_ALIAS
echo export KEYSTORE_PASSWORD=$PASSWORD
echo export ANDROID_KEYSTORE="$KEYSTORE_FILE"
echo $ANDROID_KEYSTORE $KEYSTORE_PASSWORD $KEYSTORE_ALIAS
# 2. 检查源文件是否存在
if [ ! -f "$UNSIGNED_APK" ]; then
    echo "❌ 错误: 找不到未签名文件 $UNSIGNED_APK"
    exit 1
fi

# 3. 寻找 Android SDK 中的 build-tools (支持环境路径)
if [ -z "$ANDROID_HOME" ]; then
    # 默认路径尝试（Linux/GitHub Actions）
    ANDROID_HOME="/usr/local/lib/android/sdk"
fi

BUILD_TOOLS_DIR=$(ls -d $ANDROID_HOME/build-tools/*/ | sort -V | tail -n 1)
ZIPALIGN="$BUILD_TOOLS_DIR/zipalign"
APKSIGNER="$BUILD_TOOLS_DIR/apksigner"

echo "🔍 使用 Build-tools 路径: $BUILD_TOOLS_DIR"

# 4. 对 APK 进行对齐优化 (Zipalign)
# 注意：签名之前必须先对齐
echo "📦 正在进行 Zipalign 对齐..."
$ZIPALIGN -f 4 "$UNSIGNED_APK" "./artifacts/aligned.apk"

# 5. 执行签名 (Apksigner)
echo "✍️ 正在进行 APK 签名..."
$APKSIGNER sign --ks "$KEYSTORE_FILE" \
  --ks-pass "pass:$PASSWORD" \
  --ks-key-alias "$RELEASE_ALIAS" \
  --key-pass "pass:$PASSWORD" \
  --out "$SIGNED_APK" \
  "./artifacts/aligned.apk"

# 6. 验证签名
echo "🧪 验证签名状态..."
$APKSIGNER verify "$SIGNED_APK"

if [ $? -eq 0 ]; then
    echo "🎉 签名成功! 文件位于: $SIGNED_APK"
else
    echo "❌ 签名验证失败"
    exit 1
fi
# 清理临时文件
rm -r "./artifacts"