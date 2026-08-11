#!/bin/bash

# 配置变量
KEYSTORE_FILE="./keys/x.jks"
ANDROID_KEYSTORE_BASE64=$(base64 -w 0 $KEYSTORE_FILE)

echo "正在设置 GitHub Secrets..."
gh secret --repo era-cloud/exvlan set --env-file .env
gh secret --repo era-cloud/exvlan set --env-file .env.secret
gh secret --repo era-cloud/exvlan set ANDROID_KEYSTORE_BASE64 --body "$ANDROID_KEYSTORE_BASE64"

gh secret --repo std-v/exvlan set --env-file .env
gh secret --repo std-v/exvlan set --env-file .env.secret
gh secret --repo std-v/exvlan set ANDROID_KEYSTORE_BASE64 --body "$ANDROID_KEYSTORE_BASE64"

gh secret --repo exvlan/exvlan set --env-file .env
gh secret --repo exvlan/exvlan set --env-file .env.secret
gh secret --repo exvlan/exvlan set ANDROID_KEYSTORE_BASE64 --body "$ANDROID_KEYSTORE_BASE64"

# PrimeUI license：CI 从 PRIMEUI_LICENSE secret 读取（token 存于 .env.secret 的 VITE_PRIMEUI_LICENSE）
PRIMEUI_LICENSE=$(grep '^VITE_PRIMEUI_LICENSE=' .env.secret | head -1 | cut -d= -f2-)
if [ -n "$PRIMEUI_LICENSE" ]; then
  for repo in era-cloud/exvlan std-v/exvlan exvlan/exvlan; do
    gh secret --repo "$repo" set PRIMEUI_LICENSE --body "$PRIMEUI_LICENSE"
  done
fi

cat .env .env.secret > .secrets
tee -a .secrets <<EOF
ANDROID_KEYSTORE_BASE64=$ANDROID_KEYSTORE_BASE64
EOF

# gh auth refresh --scopes admin:ssh_signing_key,repo,delete_repo,gist,read:gpg_key,write:org,write:packages,read:packages
