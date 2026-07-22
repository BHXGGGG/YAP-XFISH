import os, sys, json, time, subprocess, urllib.request, urllib.error, socket, asyncio, signal

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "release", "proxy-rs.exe")
TMP = os.path.join(ROOT, "smoketest", "data")
WEB = 19091
CLASH = 19090
PROXY = 18990
BASE = f"http://127.0.0.1:{WEB}"

def rm_tree(p):
    if os.path.isdir(p):
        for root, dirs, files in os.walk(p, topdown=False):
            for f in files:
                try: os.remove(os.path.join(root, f))
                except: pass
            for d in dirs:
                try: os.rmdir(os.path.join(root, d))
                except: pass
        try: os.rmdir(p)
        except: pass

def http(method, path, body=None, timeout=5):
    url = BASE + path
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    if data: req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, json.loads(r.read().decode() or "null")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()
    except Exception as e:
        return None, str(e)

def seed():
    rm_tree(TMP)
    os.makedirs(TMP, exist_ok=True)
    cfg = {
        "web_port": WEB,
        "core_binary": os.path.join(ROOT, "target", "release", "sing-box.exe"),
        "data_dir": TMP,
        "clash_api_port": CLASH,
        "proxy_port": PROXY,
        "api_secret": "smoke",
        "enable_tun": False,
        "autostart": False,
    }
    profile = {
        "mode": "global",
        "selected_node": None,
        "rules": [],
        "nodes": [
            {"id": "n1", "name": "Manual A", "type": "socks", "server": "10.0.0.1", "port": 1080, "subscription_id": None},
            {"id": "n2", "name": "SubNode X", "type": "socks", "server": "10.0.0.2", "port": 1080, "subscription_id": "sub1"},
        ],
    }
    subs = [{
        "id": "sub1", "name": "TestSub", "url": "http://example.invalid/sub.yaml",
        "interval": "manual", "enabled": False, "user_agent": None,
        "last_updated": None, "last_status": "idle", "last_message": "", "node_count": 1,
    }]
    with open(os.path.join(TMP, "app_config.json"), "w") as f: json.dump(cfg, f, indent=2)
    with open(os.path.join(TMP, "profile.json"), "w") as f: json.dump(profile, f, indent=2)
    with open(os.path.join(TMP, "subscriptions.json"), "w") as f: json.dump(subs, f, indent=2)

async def ws_collect(uri, events, stop, send_actions):
    import websockets
    async with websockets.connect(uri, open_timeout=5) as ws:
        async def listen():
            while not stop.is_set():
                try:
                    msg = await asyncio.wait_for(ws.recv(), timeout=0.5)
                    events.append(json.loads(msg))
                except asyncio.TimeoutError:
                    continue
                except Exception:
                    break
        listener = asyncio.create_task(listen())
        for delay, action in send_actions:
            await asyncio.sleep(delay)
            action()
        await asyncio.sleep(1.0)
        stop.set()
        await listener

def pe_subsystem(path):
    # 读 PE 可选头 Subsystem：2=GUI(无控制台) 3=Console
    with open(path, "rb") as f:
        d = f.read()
    if d[:2] != b"MZ":
        return None
    e_lfanew = int.from_bytes(d[0x3C:0x40], "little")
    if d[e_lfanew:e_lfanew+4] != b"PE\x00\x00":
        return None
    magic = int.from_bytes(d[e_lfanew+24:e_lfanew+26], "little")
    # PE32+ (0x20b): Subsystem offset = e_lfanew + 24 + 0x44
    off = e_lfanew + 24 + 0x44 if magic == 0x20b else e_lfanew + 24 + 0x44
    sub = int.from_bytes(d[off:off+2], "little")
    return sub

def main():
    results = []
    def ok(name, cond, extra=""):
        results.append((name, cond, extra))
        print(f"[{'PASS' if cond else 'FAIL'}] {name} {extra}")

    # PE subsystem
    sub = pe_subsystem(EXE)
    ok("release 为 GUI 子系统(无黑框)", sub == 2, f"subsystem={sub} (2=GUI,3=Console)")

    seed()
    env = dict(os.environ)
    env["PROXY_RS_DATA_DIR"] = TMP
    proc = subprocess.Popen([EXE], env=env)
    try:
        # wait status
        up = False
        for _ in range(60):
            st, body = http("GET", "/api/status")
            if st == 200:
                up = True
                break
            time.sleep(0.5)
        ok("后台启动并响应 /api/status", up)

        # initial profile: only manual node n1 (sub1 disabled)
        st, prof = http("GET", "/api/profile")
        init_nodes = prof.get("nodes", []) if isinstance(prof, dict) else []
        ok("初始仅显示手动节点(停用订阅被过滤)",
           st == 200 and len(init_nodes) == 1 and init_nodes[0]["id"] == "n1",
           f"nodes={[n['id'] for n in init_nodes]}")

        # WS real-time test
        import websockets  # noqa
        events = []
        stop = asyncio.Event()
        def enable():
            http("PUT", "/api/subscriptions/sub1", {"enabled": True})
        def disable():
            http("PUT", "/api/subscriptions/sub1", {"enabled": False})
        actions = [(0.5, enable), (2.0, disable)]
        try:
            asyncio.run(ws_collect(f"ws://127.0.0.1:{WEB}/api/ws", events, stop, actions))
        except Exception as e:
            ok("WS 连接/监听", False, str(e))

        prof_events = [e for e in events if e.get("type") == "profile"]
        # after enable -> 2 nodes; after disable -> 1 node
        counts = [len(e["profile"]["nodes"]) for e in prof_events]
        ok("启用订阅后 WS 推送节点数变 2", 2 in counts, f"profile事件节点数序列={counts}")
        ok("停用订阅后 WS 推送节点数变 1", 1 in counts, f"profile事件节点数序列={counts}")
        ok("共收到 Profile 事件(无需刷新网页)", len(prof_events) >= 2, f"count={len(prof_events)}")

        # portable: data dir contains runtime files
        have = lambda f: os.path.exists(os.path.join(TMP, f))
        ok("便携数据目录 app_config.json", have("app_config.json"))
        ok("便携数据目录 profile.json", have("profile.json"))
        ok("便携数据目录 subscriptions.json", have("subscriptions.json"))

        # core start -> config.json written in portable dir + proxy binds
        st, _ = http("POST", "/api/core/start")
        core_ok = False
        for _ in range(20):
            st, body = http("GET", "/api/status")
            if isinstance(body, dict) and body.get("running"):
                core_ok = True
                break
            time.sleep(0.5)
        ok("核心启动并监听代理端口(便携目录)", core_ok)
        ok("config.json 写入便携目录", have("config.json"))
        if have("config.json"):
            with open(os.path.join(TMP, "config.json")) as f:
                cc = json.load(f)
            lp = cc.get("inbounds", [{}])[0].get("listen_port")
            ok("config.json 代理端口=18990(可配/便携)", lp == PROXY, f"listen_port={lp}")
        http("POST", "/api/core/stop")
    finally:
        try: proc.terminate()
        except: pass
        time.sleep(1)
        try: proc.kill()
        except: pass

    passed = sum(1 for _, c, _ in results if c)
    total = len(results)
    print(f"\n=== {passed}/{total} passed ===")
    sys.exit(0 if passed == total else 1)

if __name__ == "__main__":
    main()
