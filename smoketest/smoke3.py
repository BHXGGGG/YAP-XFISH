#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
本轮（第五轮）冒烟测试：
  A. 延迟测试默认值：/api/config 返回 latency_test_url=gstatic、concurrency=50、timeout=5000
  B. 403 修复：订阅抓取使用浏览器 UA（本地测试服务器记录收到的 User-Agent，断言含 Mozilla）
  C. 订阅实时状态：通过 WS 接收 Subscription 事件，断言成功时携带 node_count>0（无需刷新列表）
  D. 延迟测速管线：触发 /api/nodes/latency，断言 WS 收到对应节点的 Latency 事件
全部走 PROXY_RS_DATA_DIR 隔离，不触碰真实数据。
"""
import asyncio
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import http.server
import urllib.request
import websockets  # type: ignore

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "release", "proxy-rs.exe")
VENV = "C:/Users/lo2/.workbuddy/binaries/python/envs/default"
PY = os.path.join(VENV, "Scripts", "python.exe")

passed = 0
failed = 0


def check(name, ok, detail=""):
    global passed, failed
    if ok:
        passed += 1
        print(f"  [PASS] {name}")
    else:
        failed += 1
        print(f"  [FAIL] {name}  {detail}")


def http_json(port, path, method="GET", body=None, timeout=10):
    url = f"http://127.0.0.1:{port}{path}"
    req = urllib.request.Request(url, data=json.dumps(body).encode() if body is not None else None,
                                 method=method)
    if body is not None:
        req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


def wait_web(port, timeout=20):
    t0 = time.time()
    while time.time() - t0 < timeout:
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{port}/api/status", timeout=2)
            return True
        except Exception:
            time.sleep(0.3)
    return False


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


# ---------- 本地订阅测试服务器：记录 UA，返回 Clash YAML ----------
UA_LOG = []


class Handler(http.server.BaseHTTPRequestHandler):  # type: ignore[name-defined]
    def do_GET(self):
        UA_LOG.append(self.headers.get("User-Agent", ""))
        yaml = """
proxies:
  - name: "fake-socks"
    type: socks5
    server: 127.0.0.1
    port: 1
  - name: "fake-http"
    type: http
    server: 127.0.0.1
    port: 2
"""
        data = yaml.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/yaml")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


def start_test_server(port):
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
    t = threading.Thread(target=srv.serve_forever, daemon=True)
    t.start()
    return srv


async def main():
    if not os.path.exists(EXE):
        print(f"EXE 不存在: {EXE}")
        sys.exit(1)

    web_port = free_port()
    test_port = free_port()
    srv = start_test_server(test_port)
    d = tempfile.mkdtemp(prefix="prs_s3_")
    env = dict(os.environ, PROXY_RS_DATA_DIR=d,
               # 让 web 端口避开占用
               )
    # 预置 app_config 指定 web 端口
    cfg = {"web_port": web_port, "clash_api_port": free_port(), "proxy_port": free_port()}
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, "app_config.json"), "w") as f:
        json.dump(cfg, f)

    proc = subprocess.Popen([EXE], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        if not wait_web(web_port):
            check("Web 服务启动", False)
            return
        check("Web 服务启动", True)

        # A. 延迟默认值
        c = http_json(web_port, "/api/config")
        check("延迟测试 URL 默认 gstatic", c.get("latency_test_url") == "https://www.gstatic.com/generate_204",
              str(c.get("latency_test_url")))
        check("并发数默认 50", c.get("latency_concurrency") == 50, str(c.get("latency_concurrency")))
        check("超时默认 5000", c.get("latency_timeout") == 5000, str(c.get("latency_timeout")))

        # 添加订阅（指向本地测试服务器）
        sub = {
            "name": "local-test",
            "url": f"http://127.0.0.1:{test_port}/sub",
            "interval": "manual",
            "enabled": True,
        }
        http_json(web_port, "/api/subscriptions", method="POST", body=sub)

        # 连接 WS，监听 Subscription / Latency 事件
        ws_url = f"ws://127.0.0.1:{web_port}/api/ws"
        sub_events = {}
        latency_events = {}

        async def ws_listen():
            async with websockets.connect(ws_url) as ws:  # type: ignore[attr-defined]
                while True:
                    try:
                        msg = await asyncio.wait_for(ws.recv(), timeout=25)
                    except Exception:
                        break
                    ev = json.loads(msg)
                    if ev.get("type") == "subscription":
                        sub_events[ev["id"]] = ev
                    elif ev.get("type") == "latency":
                        latency_events[ev["id"]] = ev

        task = asyncio.create_task(ws_listen())

        # 触发全部更新
        http_json(web_port, "/api/subscriptions/update-all", method="POST")
        await asyncio.sleep(6)

        # B. UA 修复：测试服务器应记录到浏览器 UA
        check("订阅抓取使用浏览器 UA（含 Mozilla）",
              any("Mozilla" in ua for ua in UA_LOG),
              f"UA_LOG={UA_LOG}")

        # C. 订阅实时状态：WS 收到 success 且 node_count>0
        subs = http_json(web_port, "/api/subscriptions")
        ok = False
        detail = ""
        for s in subs:
            ev = sub_events.get(s["id"])
            if ev and ev.get("status") == "success" and (ev.get("node_count") or 0) > 0:
                ok = True
            detail = f"sub={s.get('name')} last_status={s.get('last_status')} node_count={s.get('node_count')} ev={ev}"
        check("订阅更新成功且 WS 实时携带 node_count>0", ok, detail)

        # D. 延迟测速管线：触发后 WS 收到 Latency 事件
        http_json(web_port, "/api/nodes/latency", method="POST")
        await asyncio.sleep(6)
        check("延迟测速触发并收到 Latency 事件", len(latency_events) > 0,
              f"latency_events={list(latency_events.keys())}")

        task.cancel()
    finally:
        try:
            proc.terminate()
        except Exception:
            pass
        try:
            proc.wait(timeout=5)
        except Exception:
            subprocess.run(["taskkill", "/F", "/IM", "proxy-rs.exe"], capture_output=True)
        srv.shutdown()
        shutil.rmtree(d, ignore_errors=True)


if __name__ == "__main__":
    asyncio.run(main())
    print(f"\n=== 结果: PASS={passed} FAIL={failed} ===")
    sys.exit(1 if failed else 0)
