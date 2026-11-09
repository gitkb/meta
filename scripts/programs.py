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

It reads the workspace it is in, from any directory; --manifest-path PATH
(before the command) reads another.
"""

import argparse
import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import tarfile
import zipfile


def fail(message):
    sys.exit(f"programs.py: {message}")


def metadata(manifest):
    argv = ["cargo", "metadata", "--no-deps", "--format-version", "1", "--manifest-path", manifest]
    run = subprocess.run(argv, stdout=subprocess.PIPE, text=True)
    if run.returncode != 0:
        fail(f"{' '.join(argv)} failed (exit {run.returncode})")
    data = json.loads(run.stdout)
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
    """(name, what is wrong with it as a program, or None) for each member."""
    if windows(asset):
        with zipfile.ZipFile(asset) as archive:
            return [(info.filename, zip_member(info)) for info in archive.infolist()]
    with tarfile.open(asset) as archive:
        return [
            (
                member.name,
                "a folder" if member.isdir()
                else "not a regular file" if not member.isreg()
                else "not executable" if not member.mode & 0o111
                else None,
            )
            for member in archive.getmembers()
        ]


def members(asset):
    """The programs ASSET holds, and what is wrong with any other member: a
    program is an executable file at the archive's top level, once. None
    for the programs when the members can't be read as names at all."""
    held = archived(asset)
    if any(name == "." or name.startswith("./") for name, _ in held):
        return None, [
            "its members are named ./NAME: name each program alone "
            "(tar -czf ARCHIVE -C DIR NAME..., not tar -C DIR .)"
        ]
    programs, problems = [], []
    for name, wrong in held:
        bare = name.replace("\\", "/").rstrip("/")
        if "/" in bare or bare == "..":
            problems.append(f"{name}: not at the archive's top level (name it {os.path.basename(bare)})")
        elif wrong:
            problems.append(f"{name}: {wrong}, not a program file")
        elif name in programs:
            problems.append(f"{name}: in it twice")
        else:
            programs.append(name)
    return programs, problems


def zip_member(info):
    """What is wrong with a zip entry as a program: by the file type in its
    Unix mode, when the zip records one (a Windows zip, or Python's
    writestr, records none)."""
    kind = stat.S_IFMT(info.external_attr >> 16)
    if info.is_dir() or kind == stat.S_IFDIR:
        return "a folder"
    if kind == stat.S_IFLNK:
        return "a link"
    if kind not in (0, stat.S_IFREG):
        return "not a regular file"
    return None


def check(asset, bins):
    """ASSET holds each program once, under its name, and nothing else."""
    (held, problems), wanted = members(asset), names(asset, bins)
    if held is None:
        fail(f"{asset}: {'; '.join(problems)}")
    missing = [name for name in wanted if name not in held]
    extra = [name for name in held if name not in wanted]
    if missing:
        problems.insert(0, f"missing {', '.join(missing)}")
    if extra:
        problems.append(f"not a declared program: {', '.join(extra)}")
    if problems:
        fail(f"{asset}: {'; '.join(problems)}")


def sha256(path):
    with open(path, "rb") as file:
        return hashlib.sha256(file.read()).hexdigest()


def package(source, dist, asset, bins):
    os.makedirs(dist, exist_ok=True)
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
    held = {}
    for name, path in by_name.items():
        held[name], problems = members(path)
        if problems:
            fail(f"{path}: {'; '.join(problems)}")
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
    parser.add_argument(
        "--manifest-path",
        default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "Cargo.toml"),
    )
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
