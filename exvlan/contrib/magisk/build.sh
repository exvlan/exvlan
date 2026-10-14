#!/bin/sh
WEBUI_SOURCE="../../packages/generator/dist"

if [ -f "${WEBUI_SOURCE}/index.html" ]; then
    rm -rf ./webroot
    mkdir -p ./webroot
    cp -R "${WEBUI_SOURCE}/." ./webroot/
elif [ ! -f "./webroot/index.html" ]; then
    echo "Error: WebUI 构建产物不存在，请先运行 pnpm --dir ../../packages/generator build."
    exit 1
fi

version=$(grep '^version =' ../../exvlan/Cargo.toml | cut -d '"' -f 2)

if [ -z "$version" ]; then
    echo "Error: 版本号不存在."
    exit 1
fi
version="v${version}"

filename="exvlan_magisk_${version}.zip"
echo "${version}"

if [ ! -f "./exvlan-core" ] || [ ! -f "./exvlan-cli" ] || [ ! -f "./exvlan-api" ]; then
    wget -O "exvlan_last.zip" "https://github.com/exvlan/exvlan/releases/download/${version}/exvlan-linux-aarch64-${version}.zip"
    unzip -o exvlan_last.zip -d ./
    mv ./exvlan-linux-aarch64/* ./
    rm -rf ./exvlan_last.zip
    rm -rf ./exvlan-linux-aarch64
fi

zip -r -o -X "${filename}" ./ -x '.git/*' -x '.github/*' -x 'folder/*' -x 'build.sh' -x 'magisk_update.json'