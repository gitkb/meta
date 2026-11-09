#!/usr/bin/env python3
"""meta's programs: the [[bin]] targets of the Cargo workspace, as `cargo
metadata` lists them. Every list of them (make install, the release
archives, the Homebrew formula, the tests) comes from here.

  programs.py bins                    the program names, one per line
  programs.py packages [--dirs]       the packages that own them (their folders,
                                      from the workspace root)
  programs.py package FROM DIST ASSET stage FROM's programs in DIST and archive
                                      them as ASSET (.tar.gz, or .zip with .exe
                                      names), with ASSET.sha256
  programs.py check ASSET             ASSET holds exactly the programs
  programs.py formula VERSION ASSET.. the Homebrew formula for these archives,
                                      installing what they hold

--manifest-path PATH (before the command) reads another workspace.
"""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
import zipfile


def fail(message):
    sys.exit(f"programs.py: {message}")


def metadata(manifest):
    argv = ["cargo", "metadata", "--no-deps", "--format-version", "1"]
    if manifest:
        argv += ["--manifest-path", manifest]
    out = subprocess.run(argv, check=True, stdout=subprocess.PIPE, text=True).stdout
    data = json.loads(out)
    members = set(data["workspace_members"])
    return data["workspace_root"], [p for p in data["packages"] if p["id"] in members]


def programs(packages):
    """(package, program) for every bin target, in package order."""
    found = [
        (package, target["name"])
        for package in packages
        for target in package["targets"]
        if "bin" in target["kind"]
    ]
    if not found:
        fail("the workspace declares no [[bin]] target")
    return found


def windows(asset):
    return asset.endswith(".zip")


def names(asset, bins):
    suffix = ".exe" if windows(asset) else ""
    return [name + suffix for name in bins]


def archived(asset):
    if windows(asset):
        with zipfile.ZipFile(asset) as archive:
            return archive.namelist()
    with tarfile.open(asset) as archive:
        return [member.name for member in archive.getmembers()]


def check(asset, bins):
    held, wanted = archived(asset), names(asset, bins)
    missing = [name for name in wanted if name not in held]
    extra = [name for name in held if name not in wanted]
    problems = []
    if missing:
        problems.append(f"missing {', '.join(missing)}")
    if extra:
        problems.append(f"not a declared program: {', '.join(extra)}")
    if problems:
        fail(f"{asset}: {'; '.join(problems)}")


def sha256(path):
    with open(path, "rb") as file:
        return hashlib.sha256(file.read()).hexdigest()


def package(source, dist, asset, bins):
    os.makedirs(dist)
    wanted = names(asset, bins)
    for name in wanted:
        path = os.path.join(source, name)
        if not os.path.isfile(path):
            fail(f"{name} is not in {source}: build it first (cargo build --workspace --bins)")
        shutil.copy2(path, dist)
    if windows(asset):
        with zipfile.ZipFile(asset, "w", zipfile.ZIP_DEFLATED) as archive:
            for name in wanted:
                archive.write(os.path.join(dist, name), name)
    else:
        with tarfile.open(asset, "w:gz") as archive:
            for name in wanted:
                archive.add(os.path.join(dist, name), name)
    check(asset, bins)
    with open(asset + ".sha256", "w") as file:
        file.write(f"{sha256(asset)}  {os.path.basename(asset)}\n")


PLATFORMS = {
    "meta-darwin-arm64.tar.gz": ("on_macos", "on_arm"),
    "meta-darwin-x64.tar.gz": ("on_macos", "on_intel"),
    "meta-linux-arm64.tar.gz": ("on_linux", "on_arm"),
    "meta-linux-x64.tar.gz": ("on_linux", "on_intel"),
}


def formula(version, assets):
    by_name = {os.path.basename(asset): asset for asset in assets}
    if sorted(by_name) != sorted(PLATFORMS):
        fail(f"the formula needs exactly {', '.join(PLATFORMS)}; got {', '.join(sorted(by_name))}")
    held = {name: archived(path) for name, path in by_name.items()}
    first = held["meta-linux-x64.tar.gz"]
    for name, files in held.items():
        if sorted(files) != sorted(first):
            fail(f"{name} holds {', '.join(files)}, unlike meta-linux-x64.tar.gz")
    if "meta" not in first:
        fail("the archives hold no meta")
    blocks = {}
    for name, (os_block, arch_block) in PLATFORMS.items():
        blocks.setdefault(os_block, []).append(
            f"""    {arch_block} do
      url "https://github.com/gitkb/meta/releases/download/v#{{version}}/{name}"
      sha256 "{sha256(by_name[name])}"
    end"""
        )
    platforms = "\n\n".join(
        f"  {os_block} do\n" + "\n".join(arches) + "\n  end" for os_block, arches in blocks.items()
    )
    installs = "\n".join(f'    bin.install "{name}"' for name in first)
    return f"""# typed: false
# frozen_string_literal: true

class MetaCli < Formula
  desc "Multi-repo management CLI with AI integration"
  homepage "https://github.com/gitkb/meta"
  version "{version}"
  license "MIT"

{platforms}

  def install
{installs}
  end

  test do
    system "#{{bin}}/meta", "--version"
  end
end
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--manifest-path")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("bins")
    packages = commands.add_parser("packages")
    packages.add_argument("--dirs", action="store_true")
    pack = commands.add_parser("package")
    pack.add_argument("source")
    pack.add_argument("dist")
    pack.add_argument("asset")
    checked = commands.add_parser("check")
    checked.add_argument("asset")
    brew = commands.add_parser("formula")
    brew.add_argument("version")
    brew.add_argument("assets", nargs="+")
    args = parser.parse_args()

    if args.command == "formula":
        print(formula(args.version, args.assets), end="")
        return
    root, packages = metadata(args.manifest_path)
    found = programs(packages)
    bins = [name for _, name in found]
    if args.command == "bins":
        print("\n".join(bins))
    elif args.command == "packages":
        owners = {package["manifest_path"]: package["name"] for package, _ in found}
        for path, name in owners.items():
            print(os.path.relpath(os.path.dirname(path), root) if args.dirs else name)
    elif args.command == "package":
        package(args.source, args.dist, args.asset, bins)
    elif args.command == "check":
        check(args.asset, bins)


if __name__ == "__main__":
    main()
