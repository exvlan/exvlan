#!/usr/bin/env python3
"""模拟 systemd 的 NOTIFY_SOCKET：验证 sd_notify(READY=1 / STOPPING=1)。

用法：python3 script/lab/sd-notify-smoke.py [service-bin]
"""
import os
import signal
import socket
import subprocess
import sys
import tempfile
import time

bin_path = sys.argv[1] if len(sys.argv) > 1 else "./target/debug/exvlan-service"
tmp = tempfile.mkdtemp(prefix="sd-notify-")
notify = os.path.join(tmp, "notify.sock")
svc_sock = os.path.join(tmp, "service.sock")

listener = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
listener.bind(notify)
listener.settimeout(10)

env = dict(os.environ, NOTIFY_SOCKET=notify, EXVLAN_SERVICE_SOCKET=svc_sock)
proc = subprocess.Popen([bin_path, "--daemon"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

msgs = []
try:
    msgs.append(listener.recv(4096).decode())
    time.sleep(0.3)
    proc.send_signal(signal.SIGTERM)
    msgs.append(listener.recv(4096).decode())
    code = proc.wait(timeout=10)
except Exception as exc:  # noqa: BLE001
    proc.kill()
    print(f"[sd-notify] FAIL: {exc}")
    sys.exit(1)

print(f"[sd-notify] socket socket={svc_sock}")
for m in msgs:
    print(f"[sd-notify] 收到: {m!r}")
print(f"[sd-notify] 退出码: {code}")
ok = msgs[0] == "READY=1" and msgs[1] == "STOPPING=1" and code == 0
print("[sd-notify] 全部通过" if ok else "[sd-notify] 失败")
sys.exit(0 if ok else 1)
