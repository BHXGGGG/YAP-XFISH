import re
import shutil
import subprocess
import sys
import zipfile
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent
REL = ROOT / "release"
WEBUI = ROOT / "webui"
FILES = ["yap-xfish.exe", "sing-box.exe", "libcronet.dll", "README.txt"]
VERSION_RE = re.compile(r'^version\s*=\s*"(\d+)\.(\d+)\.(\d+)"\s*$', re.MULTILINE)


def read_version() -> str:
    """Read the package version from Cargo.toml (bump it there before running)."""
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = VERSION_RE.search(text)
    if not match:
        raise RuntimeError("Cargo.toml 中找不到 package version")
    return ".".join(match.groups())


def run(command: list[str], cwd: Path) -> None:
    print("+", " ".join(command))
    subprocess.run(command, cwd=cwd, check=True)


def main() -> None:
    version = read_version()
    readme = REL / "README.txt"
    original_readme = readme.read_text(encoding="utf-8")
    updated_readme = re.sub(r"YAP-XFISH v\d+\.\d+\.\d+", f"YAP-XFISH v{version}", original_readme)
    readme.write_text(updated_readme, encoding="utf-8")
    print(f"打包版本 v{version}")

    try:
        npm = "npm.cmd" if sys.platform == "win32" else "npm"
        run([npm, "run", "build"], WEBUI)
        run(["cargo", "build", "--release"], ROOT)

        exe = ROOT / "target" / "release" / "yap-xfish.exe"
        if not exe.exists():
            raise RuntimeError(f"缺少已构建主程序: {exe}")
        shutil.copy2(exe, REL / "yap-xfish.exe")

        for name in FILES:
            path = REL / name
            if not path.exists():
                raise RuntimeError(f"缺少 {path}")
            print(f"  {name:16} {path.stat().st_size:>10} bytes")

        zip_path = REL / f"yap-xfish-v{version}-windows-amd64.zip"
        with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as archive:
            for name in FILES:
                archive.write(REL / name, name)
        with zipfile.ZipFile(zip_path) as archive:
            if bad_file := archive.testzip():
                raise RuntimeError(f"压缩包校验失败: {bad_file}")
        print(f"已打包: {zip_path} ({zip_path.stat().st_size} bytes) @ {datetime.now():%H:%M:%S}")
    except Exception:
        readme.write_text(original_readme, encoding="utf-8")
        raise


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as exc:
        raise SystemExit(exc.returncode)
