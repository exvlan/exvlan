#!/bin/bash

mkdir -p ./artifacts/objects/

# windows is the only OS using a different convention for executable file name
if [[ $OS =~ ^windows.*$ ]]; then
    SUFFIX=.exe
    case $TARGET in
        x86_64*)  ARCH_DIR=x86_64 ;;
        i686*)    ARCH_DIR=i686 ;;
        aarch64*) ARCH_DIR=arm64 ;;
    esac
    if [[ -n "$ARCH_DIR" ]]; then
        find "exvlan/core/third_party/${ARCH_DIR}" -maxdepth 1 -type f \( -name "*.dll" -o -name "*.sys" \) -exec cp {} ./artifacts/objects/ \;
    fi
fi

if [[ $GITHUB_REF_TYPE =~ ^tag$ ]]; then
    TAG=$GITHUB_REF_NAME
else
    TAG=$GITHUB_SHA
fi

if [[ $OS =~ ^ubuntu.*$ && ! $TARGET =~ (loongarch|freebsd) ]]; then
HOST_ARCH=$(uname -m)
case $HOST_ARCH in
    x86_64)  UPX_ARCH="amd64" ;;
    aarch64) UPX_ARCH="arm64" ;;
    *)       UPX_ARCH="amd64" ;;
esac

UPX_VERSION=5.1.1
UPX_PKG="upx-${UPX_VERSION}-${UPX_ARCH}_linux"
curl -L "https://github.com/upx/upx/releases/download/v${UPX_VERSION}/${UPX_PKG}.tar.xz" -s | tar xJvf -
cp "${UPX_PKG}/upx" .
UPX_BIN=./upx
fi

for BIN in ./target/$TARGET/release/{exvlan,exvlan-cli,exvlan-api,exvlan-api-embed}"$SUFFIX"; do
if [[ -f "$BIN" ]]; then
    if [[ -n "$UPX_BIN" ]]; then
    $UPX_BIN --lzma --best "$BIN" || true
    fi

    mv "$BIN" ./artifacts/objects/
fi
done

mv ./artifacts/objects/* ./artifacts/
rm -rf ./artifacts/objects/
