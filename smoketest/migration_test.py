import os, sys, json, time, subprocess, urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "release", "proxy-rs.exe")
LEGACY = os.path.join(ROOT, "smoketest", "legacy", "Proxy")   # 模拟旧 %LOCALAPPDATA%\Proxy
DATA = os.path.join(ROOT, "target", "release", "data")        # 默认便携数据目录 = exe_dir/data

def rm(p):
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

def setup_legacy():
    rm(os.path.join(ROOT, "smoketest", "legacy"))
    os.makedirs(LEGACY, exist_ok=True)
    # 旧数据：带标记，证明是“复制”而非新建默认配置
    json.dump({"web_port": 9527, "migrated_from_legacy": True,
               "core_binary": "", "data_dir": "", "clash_api_port": 9999,
               "proxy_port": 10020, "api_secret": "old", "enable_tun": False,
               "autostart": False}, open(os.path.join(LEGACY, "app_config.json"), "w"), indent=2)
    json.dump({"nodes": [{"id": "legacy1", "name": "Legacy Node", "type": "socks",
               "server": "9.9.9.9", "port": 1, "subscription_id": None}]},
              open(os.path.join(LEGACY, "profile.json"), "w"), indent=2)

def main():
    rm(DATA)
    setup_legacy()
    # 关键：设置 LOCALAPPDATA=模拟旧目录，且不设置 PROXY_RS_DATA_DIR（否则迁移被跳过）
    env = dict(os.environ)
    env["LOCALAPPDATA"] = os.path.join(ROOT, "smoketest", "legacy")
    if "PROXY_RS_DATA_DIR" in env:
        del env["PROXY_RS_DATA_DIR"]
    proc = subprocess.Popen([EXE], env=env)
    try:
        time.sleep(3.0)  # 等待启动 + 迁移
        migrated_cfg = os.path.join(DATA, "app_config.json")
        migrated_prof = os.path.join(DATA, "profile.json")
        ok_cfg = os.path.exists(migrated_cfg)
        ok_prof = os.path.exists(migrated_prof)
        marker = False
        if ok_cfg:
            marker = json.load(open(migrated_cfg)).get("migrated_from_legacy") is True
        legacy_untouched = os.path.exists(os.path.join(LEGACY, "app_config.json"))
        print(f"[{'PASS' if ok_cfg else 'FAIL'}] 便携目录生成 app_config.json")
        print(f"[{'PASS' if ok_prof else 'FAIL'}] 便携目录生成 profile.json")
        print(f"[{'PASS' if marker else 'FAIL'}] 内容为旧数据(含迁移标记)，非新建默认配置")
        print(f"[{'PASS' if legacy_untouched else 'FAIL'}] 旧目录未被删除(仅复制)")
        allok = ok_cfg and ok_prof and marker and legacy_untouched
    finally:
        try: proc.terminate()
        except: pass
        time.sleep(1)
        try: proc.kill()
        except: pass
    # 清理测试产物（不碰真实 LOCALAPPDATA）
    rm(DATA)
    rm(os.path.join(ROOT, "smoketest", "legacy"))
    print("\n=== migration test", "PASSED ===" if allok else "FAILED ===")
    sys.exit(0 if allok else 1)

if __name__ == "__main__":
    main()
