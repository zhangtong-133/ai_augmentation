"""Install pinned official llama.cpp and Qwen artifacts inside this project only."""
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tarfile

ROOT = Path(__file__).resolve().parent.parent
DIRECTORY = ROOT / ".local-model"
CONFIG = json.loads((ROOT / "infra/local-model-runtime.json").read_text())


def download(filename, url, expected):
  archive = DIRECTORY / filename
  if not archive.exists():
    partial = archive.with_suffix(archive.suffix + ".part")
    subprocess.run(["curl", "-fL", "--retry", "2", "--connect-timeout", "15", "--max-time", "600", "-o", str(partial), url], check=True)
    partial.rename(archive)
  digest = hashlib.sha256()
  with archive.open("rb") as source:
    for chunk in iter(lambda: source.read(1024 * 1024), b""):
      digest.update(chunk)
  if digest.hexdigest() != expected:
    raise RuntimeError(f"SHA-256 不匹配，拒绝使用：{filename}")
  return archive


def main():
  if sys.platform != "linux" or os.uname().machine != "x86_64":
    raise RuntimeError("此入口仅支持 Linux/WSL2 x86_64")
  DIRECTORY.mkdir(exist_ok=True)
  for item in CONFIG["archives"]:
    archive = download(item["filename"], item["url"], item["sha256"])
    destination = DIRECTORY / item["directory"]
    destination.mkdir(exist_ok=True)
    with tarfile.open(archive, "r:gz") as content:
      content.extractall(destination, filter="data")
  # Official Ubuntu libraries are extracted, never installed into the host.
  compatibility = DIRECTORY / "compat"
  compatibility.mkdir(exist_ok=True)
  for item in CONFIG["compatibility_packages"]:
    package = download(Path(item["Filename"]).name, "https://archive.ubuntu.com/ubuntu/" + item["Filename"], item["SHA256"])
    subprocess.run(["dpkg-deb", "--extract", str(package), str(compatibility)], check=True)
  model = CONFIG["model"]
  download(model["filename"], f'https://huggingface.co/{model["repo"]}/resolve/{model["revision"]}/{model["filename"]}', model["sha256"])
  library = compatibility / "usr/lib/x86_64-linux-gnu"
  loader = library / "ld-linux-x86-64.so.2"
  server = DIRECTORY / f'llama-runtime/llama-{CONFIG["version"]}/llama-server'
  cuda = DIRECTORY / f'cuda-runtime/cudart-llama-{CONFIG["version"]}-bin-ubuntu-cuda-12.8-x64'
  libraries = f"{library}:{server.parent}:{cuda}:/usr/lib/wsl/lib:/usr/lib/x86_64-linux-gnu"
  launcher = DIRECTORY / "llama-server"
  launcher.write_text(f'#!/bin/sh\ncd {shlex.quote(str(server.parent))} || exit 1\nexec {shlex.quote(str(loader))} --library-path {shlex.quote(libraries)} {shlex.quote(str(server))} "$@"\n')
  launcher.chmod(0o700)
  subprocess.run([str(launcher), "--version"], check=True)
  print(f'llama.cpp {CONFIG["version"]} 与 Qwen 官方 GGUF 已就绪；未启动模型。')


if __name__ == "__main__":
  try:
    main()
  except (RuntimeError, subprocess.CalledProcessError) as error:
    print(error, file=sys.stderr)
    sys.exit(1)
