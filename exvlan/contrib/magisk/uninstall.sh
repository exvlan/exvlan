MODDIR=${0%/*}
pkill -f "${MODDIR}/exvlan"

# 使用 ${MODDIR:?} 确保变量非空，避免执行 rm -rf /*
rm -rf "${MODDIR:?}/"*