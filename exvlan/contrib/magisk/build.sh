#!/bin/sh

version=$(cat module.prop | grep 'version=' | awk -F '=' '{print $2}' | sed 's/ (.*//')

version='v'$(grep '^version =' ../../exvlan/Cargo.toml | cut -d '"' -f 2)

if [ -z "$version" ]; then
    echo "Error: 版本号不存在."
    exit 1
fi

filename="exvlan_magisk_${version}.zip"
echo $version  


if [ -f "./exvlan" ] && [ -f "./exvlan-cli" ] && [ -f "./exvlan-api" ]; then
    zip -r -o -X "$filename" ./ -x '.git/*' -x '.github/*' -x 'folder/*' -x 'build.sh' -x 'magisk_update.json'
else
    wget -O "exvlan_last.zip" https://github.com/littlezo/era-xvlan/releases/download/"$version"/exvlan-linux-aarch64-"$version".zip
    unzip -o exvlan_last.zip -d ./
    mv ./exvlan-linux-aarch64/* ./
    rm -rf ./exvlan_last.zip
    rm -rf ./exvlan-linux-aarch64
    zip -r -o -X "$filename" ./ -x '.git/*' -x '.github/*' -x 'folder/*' -x 'build.sh' -x 'magisk_update.json'
fi