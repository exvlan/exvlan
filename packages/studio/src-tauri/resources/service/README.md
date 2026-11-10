# exvlan-service

`script/ci/stage-service.sh` 在打包前把 `exvlan-service`（及其 `.sig`）复制到这里。
本地开发时该目录只有占位文件，缺失不会导致构建失败——首次启动会提示
「安装包内未找到服务二进制」。

落点：macOS `/Library/PrivilegedHelperTools/era.exvlan.studio.service`，
Windows `C:\ProgramData\era.exvlan.studio\bin\exvlan-service.exe`。
