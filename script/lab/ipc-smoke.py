#!/usr/bin/env python3
"""服务 IPC 冒烟（协议 v1 / v2 都能跑）。

用途：每次改服务 IPC（协议、字段、错误码）后，不依赖 GUI 就能确认
「服务还活着 + 信封正确 + 关键 op 能答」。

用法：
    # 当前装机服务（v1 平铺信封）
    python3 script/lab/ipc-smoke.py

    # 协议 v2（`{proto:2,op,params}` / `{ok,proto,data:{kind}}`）
    python3 script/lab/ipc-smoke.py --proto 2

    # 指定 socket / 管道（默认按平台猜）
    python3 script/lab/ipc-smoke.py --addr /var/run/era.exvlan.studio/service.sock

退出码：0 全过；1 有失败项；2 连不上服务。
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import socket
import sys

APP_ID = "era.exvlan.studio"
DEFAULT_UNIX = f"/var/run/{APP_ID}/service.sock"
DEFAULT_PIPE = rf"\\.\pipe\{APP_ID}.service"


def default_addr() -> str:
    if platform.system() == "Windows":
        return DEFAULT_PIPE
    return DEFAULT_UNIX


class Conn:
    """一条 IPC 连接：行式 JSON，一问一答。"""

    def __init__(self, addr: str):
        self.addr = addr
        if addr.startswith("\\\\.\\pipe\\"):
            self.sock = open(addr, "r+b", buffering=0)  # type: ignore[arg-type]
            self.is_pipe = True
        else:
            self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            self.sock.settimeout(10)
            self.sock.connect(addr)
            self.is_pipe = False
        self.buf = b""

    def call(self, payload: dict) -> dict:
        line = json.dumps(payload, ensure_ascii=False) + "\n"
        if self.is_pipe:
            self.sock.write(line.encode())
        else:
            self.sock.sendall(line.encode())
        return json.loads(self._read_line())

    def _read_line(self) -> str:
        while b"\n" not in self.buf:
            chunk = self.sock.read(4096) if self.is_pipe else self.sock.recv(4096)
            if not chunk:
                raise RuntimeError("服务关闭了连接")
            self.buf += chunk
        line, self.buf = self.buf.split(b"\n", 1)
        return line.decode()

    def close(self) -> None:
        try:
            self.sock.close()
        except Exception:
            pass


def build(proto: int, op: str, params: dict | None = None) -> dict:
    """按协议版本构造请求。"""
    if proto >= 2:
        req: dict = {"proto": proto, "op": op}
        if params is not None:
            req["params"] = params
        return req
    # v1：字段平铺
    req = {"proto": proto, "op": op}
    if params:
        req.update(params)
    return req


def envelope(proto: int, resp: dict) -> tuple[str, dict]:
    """把响应拆成 (kind, payload)，兼容 v1 平铺 / v2 `data`。"""
    if not resp.get("ok"):
        return "error", resp
    if proto >= 2:
        data = resp.get("data") or {}
        return data.get("kind", "?"), data
    # v1：kind 未知，按字段猜
    return "legacy", resp


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--proto", type=int, default=1, choices=[1, 2])
    ap.add_argument("--addr", default=default_addr())
    args = ap.parse_args()

    try:
        conn = Conn(args.addr)
    except Exception as e:
        print(f"[smoke] 连不上服务 {args.addr}: {e}", file=sys.stderr)
        return 2

    failures: list[str] = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        print(f"  {'PASS' if ok else 'FAIL'}  {name}{(' — ' + detail) if detail else ''}")
        if not ok:
            failures.append(name)

    print(f"[smoke] addr={args.addr} proto={args.proto}")

    # 1) ping：服务活着
    try:
        resp = conn.call(build(args.proto, "ping"))
        check("ping", resp.get("ok") is True, json.dumps(resp, ensure_ascii=False)[:120])
    except Exception as e:
        print(f"[smoke] ping 失败: {e}", file=sys.stderr)
        return 2

    # 2) hello：版本串 + proto
    resp = conn.call(build(args.proto, "hello"))
    kind, data = envelope(args.proto, resp)
    check("hello.ok", resp.get("ok") is True)
    if args.proto >= 2:
        check("hello.kind", kind == "hello", kind)
        check("hello.proto", data.get("proto") == 2, str(data.get("proto")))
    check("hello.service", bool(data.get("service") or resp.get("service")), str(data.get("service") or resp.get("service")))

    # 3) status：mode / kernel
    resp = conn.call(build(args.proto, "status"))
    kind, data = envelope(args.proto, resp)
    check("status.ok", resp.get("ok") is True)
    if args.proto >= 2:
        check("status.kind", kind == "status", kind)
        check("status.mode", bool(data.get("mode")), str(data.get("mode")))
        check("status.kernel", isinstance(data.get("kernel"), dict), str(type(data.get("kernel"))))
    else:
        check("status.mode", bool(resp.get("mode")), str(resp.get("mode")))

    # 4) config.list：配置目录可读
    resp = conn.call(build(args.proto, "config.list"))
    kind, data = envelope(args.proto, resp)
    check("config.list.ok", resp.get("ok") is True, json.dumps(resp, ensure_ascii=False)[:120])

    # 5) whitelist.get：白名单可读
    resp = conn.call(build(args.proto, "whitelist.get"))
    kind, data = envelope(args.proto, resp)
    check("whitelist.get.ok", resp.get("ok") is True, json.dumps(resp, ensure_ascii=False)[:120])

    # 6) port.owner：探测（端口空闲或结构化占用都算通过）
    resp = conn.call(build(args.proto, "port.owner"))
    check("port.owner.ok", resp.get("ok") is True, json.dumps(resp, ensure_ascii=False)[:140])

    # 7) 未知 op：必须回 ok=false + code（不能崩、不能空响应）
    resp = conn.call(build(args.proto, "definitely.not.an.op"))
    check("unknown_op.rejected", resp.get("ok") is False and bool(resp.get("code")), json.dumps(resp, ensure_ascii=False)[:120])

    # 8) 错误信封：code 必须是字符串（v2 是枚举序列化后的 snake_case）
    if not resp.get("ok"):
        check("error.code_is_string", isinstance(resp.get("code"), str), str(resp.get("code")))

    conn.close()
    print(f"[smoke] {'全部通过' if not failures else '失败：' + ', '.join(failures)}")
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
