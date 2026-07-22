#!/usr/bin/env python3
"""验证第四轮修复：
1) 全新配置默认 proxy_port == 10020（不再是 7890）。
2) 核心路径随文件夹移动：即使 app_config.json 里 core_binary 指向一个不存在的旧绝对路径，
   核心仍能启动（resolve_core_binary 回退到 exe 同目录的 sing-box.exe）。
"""
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
import asyncio

import websockets  # noqa: F401  (venv 已装)

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "release", "proxy-rs.exe")
SINGBOX = os.path.join(ROOT, "target", "release", "sing-box.exe")

results = []


def check(name, ok, detail=""):
    results.append((name, ok, detail))
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" :: {detail}" if detail else ""))


def wait_web(port, timeout=20):
    end = time.time() + timeout
    while time.time() < end:
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{port}/api/status", timeout=1)
            return True
        except Exception:
            time.sleep(0.3)
    return False


def http_json(port, path, method="GET", data=None):
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}{path}",
        data=json.dumps(data).encode() if data is not None else None,
        headers={"Content-Type": "application/json"} if data is not None else {},
        method=method,
    )
    with urllib.request.urlopen(req, timeout=5) as r:
        return json.loads(r.read().decode())


def port_listening(port):
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=1):
            return True
    except Exception:
        return False


async def capture_ws_logs(port, secs=4):
    try:
        async with websockets.connect(f"ws://127.0.0.1:{port}/api/ws") as ws:
            end = time.time() + secs
            while time.time() < end:
                try:
                    msg = await asyncio.wait_for(ws.recv(), timeout=1.0)
                except asyncio.TimeoutError:
                    continue
                try:
                    obj = json.loads(msg)
                except Exception:
                    continue
                if obj.get("type") == "log":
                    print(f"  [ws-log] {obj.get('level')}: {obj.get('message')}")
    except Exception as e:
        print(f"  [ws-log] connect err {e}")


def stop_exe(proc):
    try:
        proc.terminate()
    except Exception:
        pass
    try:
        proc.wait(timeout=5)
    except Exception:
        pass
    # 始终清理核心子进程，避免占用端口影响后续场景
    subprocess.run(["taskkill", "/F", "/IM", "proxy-rs.exe"], capture_output=True)
    subprocess.run(["taskkill", "/F", "/IM", "sing-box.exe"], capture_output=True)


# ---------- 场景 1：全新配置默认 proxy_port = 10020 ----------
def test_default_port():
    d = tempfile.mkdtemp(prefix="prs_def_")
    port = 19527
    # 写入完整配置，但【故意省略 proxy_port】，验证 serde 默认值生效（应为 10020）。
    cfg = {
        "web_port": port,
        "core_binary": r"C:\__proxy_rs_nonexistent__\sing-box.exe",
        "data_dir": d,
        "clash_api_port": 19099,
        "api_secret": "testsecret",
        "enable_tun": False,
        "autostart": False,
    }
    with open(os.path.join(d, "app_config.json"), "w") as f:
        json.dump(cfg, f, indent=2)
    # 最小 profile，确保核心能渲染并启动
    profile = {"mode": "rule", "selected_node": None, "nodes": [], "rules": []}
    with open(os.path.join(d, "profile.json"), "w") as f:
        json.dump(profile, f, indent=2)
    with open(os.path.join(d, "subscriptions.json"), "w") as f:
        json.dump([], f)

    env = dict(os.environ, PROXY_RS_DATA_DIR=d)
    proc = subprocess.Popen([EXE], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        if not wait_web(port):
            check("场景1: Web 服务启动", False, "web 未启动")
            return
        check("场景1: Web 服务启动", True)
        # 启动核心，让渲染出的 config.json 反映内存中的 proxy_port（默认值 10020）。
        try:
            http_json(port, "/api/core/start", method="POST")
            time.sleep(3.0)
        except Exception as e:
            print(f"  [diag] core/start err: {e}")
        cfg_json = os.path.join(d, "config.json")
        if os.path.exists(cfg_json):
            with open(cfg_json) as f:
                rc = json.load(f)
            lp = [i.get("listen_port") for i in rc.get("inbounds", [])
                  if i.get("type") == "mixed"]
            check("场景1: 默认 proxy_port == 10020（渲染入站端口）",
                  lp and lp[0] == 10020, f"mixed listen_port={lp}")
        else:
            check("场景1: 渲染 config.json 存在", False)
    finally:
        stop_exe(proc)


# ---------- 场景 2：core_binary 指向不存在的旧路径，核心仍能启动 ----------
def test_stale_core_binary():
    d = tempfile.mkdtemp(prefix="prs_stale_")
    port = 19528
    pproxy = 18990
    pclash = 19099
    cfg = {
        "web_port": port,
        "core_binary": r"C:\__proxy_rs_nonexistent_old_location__\sing-box.exe",
        "data_dir": d,
        "clash_api_port": pclash,
        "proxy_port": pproxy,
        "api_secret": "testsecret",
        "enable_tun": False,
        "autostart": False,
    }
    with open(os.path.join(d, "app_config.json"), "w") as f:
        json.dump(cfg, f, indent=2)
    # 最小合法 profile（无节点即可渲染 direct/block 出站并正常绑定 mixed 入站）
    profile = {
        "mode": "rule",
        "selected_node": None,
        "nodes": [],
        "rules": [],
    }
    with open(os.path.join(d, "profile.json"), "w") as f:
        json.dump(profile, f, indent=2)
    with open(os.path.join(d, "subscriptions.json"), "w") as f:
        json.dump([], f)

    env = dict(os.environ, PROXY_RS_DATA_DIR=d)
    proc = subprocess.Popen([EXE], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        if not wait_web(port):
            check("场景2: Web 服务启动", False)
            return
        check("场景2: Web 服务启动", True)
        # 关键：core_binary 指向不存在路径，核心应回退到 exe 同目录并启动
        r = http_json(port, "/api/core/start", method="POST")
        time.sleep(2.0)
        # 捕获 sing-box 实际运行日志（确认是否绑定成功或有 FATAL）
        asyncio.run(capture_ws_logs(port, 5))
        st = http_json(port, "/api/status")
        check("场景2: 核心启动成功（绕过失效路径）", st.get("running") is True,
              f"running={st.get('running')}")
        # 重试探测端口（sing-box 绑定可能需要几秒）
        listening = False
        for _ in range(24):
            if port_listening(pproxy):
                listening = True
                break
            time.sleep(0.5)
        check("场景2: 代理端口监听 (无黑框启动)", listening,
              f"port {pproxy} listening={listening}")
        # 诊断：sing-box check 校验渲染配置是否合法
        cj = os.path.join(d, "config.json")
        if os.path.exists(cj):
            try:
                chk = subprocess.run([SINGBOX, "check", "-c", cj],
                                     capture_output=True, encoding="oem",
                                     errors="ignore", timeout=15)
                print(f"  [diag] sing-box check rc={chk.returncode}")
                print(f"  [diag] check stdout: {chk.stdout.strip()[-500:]}")
                print(f"  [diag] check stderr: {chk.stderr.strip()[-500:]}")
            except Exception as e:
                print(f"  [diag] check err {e}")
        # 诊断：netstat 看 18990 实际绑定情况
        try:
            out = subprocess.run(
                ["netstat", "-ano"], capture_output=True, encoding="oem",
                errors="ignore", timeout=10
            ).stdout
            lines = [l for l in out.splitlines() if f":{pproxy}" in l]
            print(f"  [diag] netstat :{pproxy} -> {lines}")
        except Exception as e:
            print(f"  [diag] netstat err {e}")
        cfg_json = os.path.join(d, "config.json")
        if os.path.exists(cfg_json):
            with open(cfg_json) as f:
                rc = json.load(f)
            ins = rc.get("inbounds", [])
            for i in ins:
                if i.get("type") == "mixed":
                    print(f"  [diag] mixed inbound listen={i.get('listen')!r} "
                          f"listen_port={i.get('listen_port')}")
    finally:
        stop_exe(proc)


if __name__ == "__main__":
    if not os.path.exists(EXE):
        print("EXE 不存在，请先 cargo build --release")
        sys.exit(2)
    if not os.path.exists(SINGBOX):
        print("sing-box.exe 不在 target/release，请先放置")
        sys.exit(2)
    test_default_port()
    test_stale_core_binary()
    fails = [n for n, ok, _ in results if not ok]
    print(f"\n==== 共 {len(results)} 项，失败 {len(fails)} ====")
    sys.exit(1 if fails else 0)
