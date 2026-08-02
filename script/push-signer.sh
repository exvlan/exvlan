#!/bin/bash

# 配置变量
KEYSTORE_FILE="./keys/x.jks"
ANDROID_KEYSTORE_BASE64=$(base64 -w 0 $KEYSTORE_FILE)

echo "正在设置 GitHub Secrets..."
gh secret --repo meta-era/exvlan set --env-file .env
gh secret --repo meta-era/exvlan set ANDROID_KEYSTORE_BASE64 --body "$ANDROID_KEYSTORE_BASE64"

gh secret --repo era-cloud/exvlan set --env-file .env
gh secret --repo era-cloud/exvlan set ANDROID_KEYSTORE_BASE64 --body "$ANDROID_KEYSTORE_BASE64"

gh secret --repo meta-club/exvlan set --env-file .env
gh secret --repo meta-club/exvlan set ANDROID_KEYSTORE_BASE64 --body "$ANDROID_KEYSTORE_BASE64"

gh secret --repo excloudfs/exvlan set --env-file .env
gh secret --repo excloudfs/exvlan set ANDROID_KEYSTORE_BASE64 --body "$ANDROID_KEYSTORE_BASE64"

cat .env > .secrets
tee -a .secrets <<EOF
ANDROID_KEYSTORE_BASE64=$ANDROID_KEYSTORE_BASE64
EOF

# gh secret --repo exvlan/exvlan set --env-file .env
# gh secret --repo exvlan/exvlan set ANDROID_KEYSTORE_BASE64 --body "$ANDROID_KEYSTORE_BASE64"

# gh secret --repo unixse/exvlan set KEYSTORE_ALIAS --body "$KEY_ALIAS"
# gh secret --repo unixse/exvlan set KEYSTORE_PASSWORD --body "$PASSWORD"
# gh secret --repo unixse/exvlan set ANDROID_KEYSTORE_BASE64 --body "$ANDROID_KEYSTORE_BASE64"

# gh auth refresh --scopes admin:ssh_signing_key,repo,delete_repo,gist,read:gpg_key,write:org,write:packages,read:packages