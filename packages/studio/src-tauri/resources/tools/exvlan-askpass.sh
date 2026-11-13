#!/bin/sh
# sudo -A 的 askpass 助手：把用户输入的密码写到 stdout，然后由 sudo 读走。
#
# 约定（sudo 侧）：
#   - 只在 GUI 进程里被调用（`SUDO_ASKPASS` 由提权代码设置）；
#   - 密码只在本进程与 sudo 之间通过管道流动 —— 不写临时文件、不进日志；
#   - 用户取消时以非 0 退出，sudo 会报「no password was provided」。
#
# 弹窗顺序：zenity（GNOME/大多数桌面）→ kdialog（KDE）→ xterm（最小化环境）。
# 三者都没有时直接失败，由 GUI 展示「可复制命令」让用户手动执行。

set -u

title="Exvlan 需要系统权限"

if command -v zenity >/dev/null 2>&1; then
    exec zenity --password --title="$title"
fi

if command -v kdialog >/dev/null 2>&1; then
    exec kdialog --title "$title" --password
fi

if command -v xterm >/dev/null 2>&1; then
    # xterm 的 stdout 就是 sudo 给的管道：窗口里读到密码后写到 stdout。
    exec xterm -title "$title" -e sh -c '
        printf "%s" "请输入密码（不会回显）：" >&2
        stty -echo
        IFS= read -r password
        stty echo
        printf "\n" >&2
        printf "%s\n" "$password"
    '
fi

echo "exvlan-askpass: 没有可用的密码对话框（zenity / kdialog / xterm 都未安装）" >&2
exit 1
