#!/bin/zsh

# 1. 核心修复：完美兼容 Bash 和 Zsh 的当前脚本绝对路径获取逻辑
if [ -n "$BASH_SOURCE" ]; then
    CURRENT_SCRIPT="${BASH_SOURCE[0]}"
elif [ -n "$ZSH_VERSION" ]; then
    CURRENT_SCRIPT="${(%):-%x}"
else
    CURRENT_SCRIPT="$0"
fi

SCRIPT_DIR="$(cd "$(dirname "$CURRENT_SCRIPT")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# 2. 锁定并导出密钥库的绝对路径
export KEYSTORE_PATH="${ROOT}/keys/x.jks"

# 3. 动态读取并载入同目录下的 .env 文件
ENV_FILE="${ROOT}/.env"

if [ -f "$ENV_FILE" ]; then
    # 逐行读取 .env，自动忽略注释(#)和空行
    while IFS='=' read -r key value || [ -n "$key" ]; do
        # 跳过空行和注释
        [[ "$key" =~ ^[[:space:]]*# || -z "$key" ]] && continue
        
        # 去除 value 两侧的引号 (单引号或双引号)
        value=$(echo "$value" | sed -e 's/^"//' -e 's/"$//' -e "s/^'//" -e "s/'$//")
        
        # 核心：使用 export 强制将其注入全局环境变量空间
        export "$key"="$value"
    done < "$ENV_FILE"
    echo "✅ 成功从 $ENV_FILE 载入并导出了签名环境变量。"
else
    echo "❌ 未找到环境配置文件: $ENV_FILE"
fi
