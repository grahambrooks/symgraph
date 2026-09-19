#!/usr/bin/env python3
"""Release helper for grahambrooks Rust tools (release-kit v2).

Byte-identical in every repo; repo specifics come from .release.env:

    BINS=colab                  binaries to ship, comma-separated; the first names the archive
    PACKAGE=colab-cli           cargo package that owns them (optional; needed in workspaces)
    CRATE_DIR=.                 directory holding the Cargo.toml to build (default: repo root)
    FEATURES=                   cargo features for the release build (optional)
    EXTRA_TARGETS=              extra targets, e.g. x86_64-pc-windows-msvc (optional)
    PUBLISH_CRATE=false         publish PACKAGE to crates.io after a successful release
    FORMULA_NAME=colab          Homebrew formula file/name
    FORMULA_CLASS=Colab         Ruby class name
    FORMULA_DESC=...            one-line description
    FORMULA_LICENSE=MIT         SPDX id

Subcommands (all run by .github/workflows/release.yml):

    plan TAG                    validate the tag and config; write GitHub step outputs
    set-version VERSION         write VERSION into the Cargo.toml(s) that own BINS
    sums DIR                    print SHA256SUMS for the archives in DIR
    formula TAG SHA256SUMS      write Formula/<FORMULA_NAME>.rb
    publish-crate VERSION       cargo publish PACKAGE unless VERSION is already on crates.io
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

ENV_FILE = Path(".release.env")
TAG_RE = re.compile(r"^v(\d{4})\.(\d{1,2})\.(\d+)$")

# Every tool ships these four; bx and the formula both rely on it.
BASE_TARGETS = [
    ("aarch64-apple-darwin", "macos-14"),
    # Native Intel runner: cross-compiling from Apple Silicon fails for any tool
    # that links a -sys crate (openssl-sys via git2/libssh2), since the runner
    # only has arm64 libraries.
    ("x86_64-apple-darwin", "macos-15-intel"),
    ("x86_64-unknown-linux-gnu", "ubuntu-latest"),
    ("aarch64-unknown-linux-gnu", None),          # native arm runner, or cross (see plan)
]
EXTRA_RUNNERS = {
    "x86_64-pc-windows-msvc": "windows-latest",
    "aarch64-pc-windows-msvc": "windows-latest",
    "x86_64-unknown-linux-musl": "ubuntu-latest",
}
FORMULA_TARGETS = {
    "darwin_arm": "aarch64-apple-darwin",
    "darwin_intel": "x86_64-apple-darwin",
    "linux_arm": "aarch64-unknown-linux-gnu",
    "linux_intel": "x86_64-unknown-linux-gnu",
}


def fail(msg: str) -> None:
    print(f"::error::{msg}", file=sys.stderr)
    sys.exit(1)


def load_config() -> dict[str, str]:
    if not ENV_FILE.exists():
        fail(".release.env not found — stamp the repo with release-kit/stamp.sh")
    cfg = {}
    for line in ENV_FILE.read_text().splitlines():
        m = re.match(r"^([A-Za-z_][A-Za-z0-9_]*)=(.*)$", line.strip())
        if m:
            cfg[m.group(1)] = m.group(2).strip().strip("'\"")
    cfg.setdefault("CRATE_DIR", ".")
    cfg.setdefault("FORMULA_LICENSE", "MIT")
    # v1 .release.env files named a single BIN_NAME.
    if not cfg.get("BINS") and cfg.get("BIN_NAME"):
        cfg["BINS"] = cfg["BIN_NAME"]
    missing = [k for k in ("BINS", "FORMULA_NAME", "FORMULA_CLASS", "FORMULA_DESC") if not cfg.get(k)]
    if missing:
        fail(f".release.env is missing {', '.join(missing)}")
    return cfg


def bins(cfg: dict[str, str]) -> list[str]:
    return [b.strip() for b in cfg["BINS"].split(",") if b.strip()]


def output(**values: str) -> None:
    out = os.environ.get("GITHUB_OUTPUT")
    lines = [f"{k}={v}" for k, v in values.items()]
    if out:
        with open(out, "a") as f:
            f.write("\n".join(lines) + "\n")
    print("\n".join(lines))


def cmd_plan(tag: str) -> None:
    m = TAG_RE.match(tag)
    if not m:
        fail(f"tag {tag!r} is not CalVer vYYYY.M.N")
    cfg = load_config()
    crate_dir = cfg["CRATE_DIR"]
    if not (Path(crate_dir) / "Cargo.toml").exists():
        fail(f"CRATE_DIR={crate_dir} has no Cargo.toml")
    # Public repos get the free native arm runner; private repos cross-compile.
    arm_linux = "ubuntu-latest" if os.environ.get("REPO_PRIVATE") == "true" else "ubuntu-24.04-arm"
    include = [{"target": t, "os": os_ or arm_linux, "workspace": crate_dir} for t, os_ in BASE_TARGETS]
    for t in filter(None, (x.strip() for x in cfg.get("EXTRA_TARGETS", "").split(","))):
        if t not in EXTRA_RUNNERS:
            fail(f"EXTRA_TARGETS: no runner known for {t}")
        include.append({"target": t, "os": EXTRA_RUNNERS[t], "workspace": crate_dir})
    output(
        version=tag[1:],
        matrix=json.dumps({"include": include}, separators=(",", ":")),
        archive=f"{bins(cfg)[0]}-$tag-$target",
        bins=",".join(bins(cfg)),
        package=cfg.get("PACKAGE", ""),
        features=cfg.get("FEATURES", ""),
        manifest_path="" if crate_dir == "." else f"{crate_dir}/Cargo.toml",
        publish_crate="true" if cfg.get("PUBLISH_CRATE", "").lower() == "true" else "false",
    )


def set_toml_version(path: Path, section: str, version: str) -> bool:
    """Set `version = "..."` inside [section] of a Cargo.toml; False if it has none there."""
    text = path.read_text()
    head = re.search(rf"^\[{re.escape(section)}\][ \t]*$", text, re.M)
    if not head:
        return False
    nxt = re.search(r"^\[", text[head.end():], re.M)
    end = head.end() + (nxt.start() if nxt else len(text) - head.end())
    body = text[head.end():end]
    new_body, n = re.subn(r'^(version\s*=\s*)"[^"]*"', rf'\g<1>"{version}"', body, count=1, flags=re.M)
    if n:
        path.write_text(text[:head.end()] + new_body + text[end:])
    return bool(n)


def cmd_set_version(version: str) -> None:
    cfg = load_config()
    crate_dir = Path(cfg["CRATE_DIR"])
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=crate_dir, check=True, capture_output=True, text=True).stdout)
    wanted = set(bins(cfg))
    owners = [p for p in meta["packages"]
              if (cfg.get("PACKAGE") and p["name"] == cfg["PACKAGE"])
              or (not cfg.get("PACKAGE") and any("bin" in t["kind"] and t["name"] in wanted for t in p["targets"]))]
    if not owners:
        fail(f"no package in {crate_dir} builds {', '.join(wanted)}")
    changed = []
    root = Path(meta["workspace_root"]) / "Cargo.toml"
    if set_toml_version(root, "workspace.package", version):
        changed.append(f"{root} [workspace.package]")
    for p in owners:
        manifest = Path(p["manifest_path"])
        if set_toml_version(manifest, "package", version):
            changed.append(str(manifest))
    if not changed:
        fail("found no literal version to set (neither [workspace.package] nor the owning [package])")
    # Refresh Cargo.lock's entries for the workspace's own packages.
    subprocess.run(["cargo", "metadata", "--format-version", "1"], cwd=crate_dir, check=True,
                   stdout=subprocess.DEVNULL)
    print(f"set version {version} in: {', '.join(changed)}")


def cmd_sums(directory: str) -> None:
    files = sorted(p for p in Path(directory).iterdir()
                   if p.name.endswith((".tar.gz", ".zip")))
    if not files:
        fail(f"no archives in {directory}")
    for p in files:
        print(f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}")


FORMULA = """\
# Generated by scripts/release.py (release-kit) from the {tag} release — do not edit.
class {cls} < Formula
  desc "{desc}"
  homepage "https://github.com/{repo}"
  version "{version}"
  license "{license}"

  on_macos do
    on_arm do
      url "{base}/{archive_darwin_arm}"
      sha256 "{sha_darwin_arm}"
    end
    on_intel do
      url "{base}/{archive_darwin_intel}"
      sha256 "{sha_darwin_intel}"
    end
  end

  on_linux do
    on_arm do
      url "{base}/{archive_linux_arm}"
      sha256 "{sha_linux_arm}"
    end
    on_intel do
      url "{base}/{archive_linux_intel}"
      sha256 "{sha_linux_intel}"
    end
  end

  def install
{install}
  end

  test do
{test}
  end
end
"""


def cmd_formula(tag: str, sums_file: str) -> None:
    cfg = load_config()
    repo = os.environ.get("GITHUB_REPOSITORY") or fail("GITHUB_REPOSITORY is not set")
    sums = {}
    for line in Path(sums_file).read_text().splitlines():
        if line.strip():
            digest, name = line.split(maxsplit=1)
            sums[name.strip()] = digest
    names = bins(cfg)
    values = {}
    for key, target in FORMULA_TARGETS.items():
        archive = f"{names[0]}-{tag}-{target}.tar.gz"
        if archive not in sums:
            fail(f"{archive} is missing from SHA256SUMS")
        values[f"archive_{key}"] = archive
        values[f"sha_{key}"] = sums[archive]
    Path("Formula").mkdir(exist_ok=True)
    out = Path("Formula") / f"{cfg['FORMULA_NAME']}.rb"
    out.write_text(FORMULA.format(
        tag=tag, cls=cfg["FORMULA_CLASS"], desc=cfg["FORMULA_DESC"].replace('"', '\\"'),
        repo=repo, version=tag[1:], license=cfg["FORMULA_LICENSE"],
        base=f"https://github.com/{repo}/releases/download/{tag}",
        install="\n".join(f'    bin.install "{b}"' for b in names),
        test="\n".join(f'    assert_path_exists bin/"{b}"' for b in names),
        **values,
    ))
    print(f"wrote {out}")


def cmd_publish_crate(version: str) -> None:
    cfg = load_config()
    package = cfg.get("PACKAGE") or bins(cfg)[0]
    req = urllib.request.Request(
        f"https://crates.io/api/v1/crates/{package}/{version}",
        headers={"User-Agent": f"release-kit ({os.environ.get('GITHUB_REPOSITORY', 'grahambrooks')})"})
    try:
        urllib.request.urlopen(req)
        print(f"{package} {version} is already on crates.io — skipping")
        return
    except urllib.error.HTTPError as e:
        if e.code != 404:
            fail(f"crates.io returned {e.code} for {package} {version}")
    # --allow-dirty: the version was stamped from the tag just before this.
    subprocess.run(["cargo", "publish", "-p", package, "--allow-dirty"], cwd=cfg["CRATE_DIR"], check=True)


def main(argv: list[str]) -> None:
    commands = {
        "plan": (cmd_plan, 1), "set-version": (cmd_set_version, 1), "sums": (cmd_sums, 1),
        "formula": (cmd_formula, 2), "publish-crate": (cmd_publish_crate, 1),
    }
    if len(argv) < 2 or argv[1] not in commands or len(argv) - 2 != commands[argv[1]][1]:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    fn, _ = commands[argv[1]]
    fn(*argv[2:])


if __name__ == "__main__":
    main(sys.argv)
