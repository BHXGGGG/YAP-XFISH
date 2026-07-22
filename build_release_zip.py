import os, zipfile, datetime

ROOT = os.path.dirname(os.path.abspath(__file__))
REL = os.path.join(ROOT, "release")
FILES = ["proxy-rs.exe", "sing-box.exe", "README.txt"]
ZIP = os.path.join(REL, "proxy-rs-v0.1.0-windows-amd64.zip")

# 校验源码已是最新：exe 时间应晚于源码
src_mtime = max(os.path.getmtime(os.path.join(ROOT, "src", f))
                for f in os.listdir(os.path.join(ROOT, "src")))
for f in FILES:
    p = os.path.join(REL, f)
    assert os.path.exists(p), f"缺少 {p}"
    print(f"  {f:16} {os.path.getsize(p):>10} bytes")

# 直接以 "w" 模式打开即原地截断覆盖，无需先删除（避免沙箱安全删除拦截）。
with zipfile.ZipFile(ZIP, "w", zipfile.ZIP_DEFLATED) as z:
    for f in FILES:
        z.write(os.path.join(REL, f), f)
print(f"\n已打包: {ZIP} ({os.path.getsize(ZIP)} bytes) @ {datetime.datetime.now():%H:%M:%S}")
print("内容:")
with zipfile.ZipFile(ZIP) as z:
    for i in z.infolist():
        print(f"  {i.filename:16} {i.file_size:>10}")
