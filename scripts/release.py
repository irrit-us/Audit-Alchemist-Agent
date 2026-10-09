"""Build metadata and portable release archives; Python is needed only in CI."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
DIST = ROOT / "target/release-dist"
TARGETS = ("x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl", "x86_64-pc-windows-msvc")


def metadata():
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", version):
        raise ValueError("release version must be a portable SemVer without build metadata")
    if os.environ.get("GITHUB_REF_TYPE") == "tag" and os.environ["GITHUB_REF_NAME"] != f"v{version}":
        raise ValueError("release tag must equal v + Cargo.toml package.version")
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    return version, commit


def archive_name(version, target):
    extension = ".zip" if target.endswith("windows-msvc") else ".tar.gz"
    return f"alchemist-v{version}-{target}{extension}"


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def smoke(binary, workspace, version):
    def run(*args):
        return subprocess.check_output([str(binary), *args], cwd=workspace, text=True, encoding="utf-8", timeout=30)

    if run("--version").strip() != f"alchemist {version}":
        raise ValueError("packaged binary version does not match manifest")
    if not json.loads(run("skills")):
        raise ValueError("packaged binary is missing built-in skills")
    (workspace / "sample.py").write_text("print('release smoke')\n", encoding="utf-8")
    config = workspace / "node.toml"
    if not json.loads(run("check-config", "--config", str(config)))["valid"]:
        raise ValueError("packaged config is invalid")
    preview = json.loads(run("audit", "--config", str(config), "--dry-run", "--target", "sample.py"))
    if preview["files"] != 1 or not preview["dry_run"]:
        raise ValueError("packaged CLI cannot run a configured audit")


def package(target):
    version, commit = metadata()
    executable = "alchemist.exe" if target.endswith("windows-msvc") else "alchemist"
    source = ROOT / "target" / target / "release" / executable
    if not source.is_file():
        raise ValueError(f"missing release binary: {source}")
    DIST.mkdir(parents=True, exist_ok=True)
    archive = DIST / archive_name(version, target)
    if archive.exists() or archive.with_name(archive.name + ".sha256").exists():
        raise ValueError("release archive already exists; use a clean output directory")
    directory_name = f"alchemist-v{version}-{target}"
    with tempfile.TemporaryDirectory() as temporary:
        staging = Path(temporary) / directory_name
        staging.mkdir()
        shutil.copy2(source, staging / executable)
        (staging / executable).chmod(0o755)
        shutil.copy2(ROOT / "LICENSE", staging / "LICENSE")
        shutil.copy2(ROOT / "skills/LICENSE.codex-auditor", staging / "LICENSE.codex-auditor")
        shutil.copy2(ROOT / "skills/UPSTREAM.md", staging / "UPSTREAM.md")
        (staging / "node.toml").write_text('schema_version = 1\n\n[cli]\nmodel = "YOUR-MODEL"\nendpoint = "https://YOUR-PROVIDER/v1/chat/completions"\nroot = "."\ntarget = "."\nformat = "quiet"\n', encoding="utf-8")
        (staging / "README.md").write_text(
            f"# Audit Alchemist {version}\n\nTarget: `{target}`\n\n"
            "Edit `node.toml` for your provider and target, then run:\n\n"
            "```sh\nalchemist check-config --config node.toml\nalchemist audit --config node.toml --dry-run\nalchemist audit --config node.toml\n```\n\n"
            "Set AUDIT_API_KEY for a real audit. Bash and optional debugger/MCP executables are separate host tools. On Windows install Git Bash or set AUDIT_BASH. Python and Rust are not required to run this binary.\n\n"
            f"[Configuration reference](https://github.com/irrit-us/Audit-Alchemist-Agent/blob/{commit}/docs/configuration.md)\n",
            encoding="utf-8")
        (staging / "build-info.json").write_text(json.dumps({"version": version, "commit": commit, "target": target,
            "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip()}, indent=2) + "\n", encoding="utf-8")
        if archive.suffix == ".zip":
            with zipfile.ZipFile(archive, "x", compression=zipfile.ZIP_DEFLATED) as output:
                for path in sorted(staging.iterdir()):
                    output.write(path, f"{directory_name}/{path.name}")
        else:
            with tarfile.open(archive, "x:gz") as output:
                output.add(staging, arcname=directory_name)
        extracted = Path(temporary) / "extracted"
        extracted.mkdir()
        # Only extract the archive produced above from the explicit staging allowlist.
        if archive.suffix == ".zip":
            with zipfile.ZipFile(archive) as source_archive:
                source_archive.extractall(extracted)
        else:
            with tarfile.open(archive) as source_archive:
                source_archive.extractall(extracted, filter="data")
        workspace = extracted / directory_name
        smoke(workspace / executable, workspace, version)
    archive.with_name(archive.name + ".sha256").write_text(f"{digest(archive)}  {archive.name}\n", encoding="ascii")
    print(f"Packaged and smoke-tested {archive.name}")


def verify():
    version, _ = metadata()
    expected = {archive_name(version, target) for target in TARGETS}
    expected |= {name + ".sha256" for name in list(expected)}
    if {p.name for p in DIST.iterdir()} != expected:
        raise ValueError("release must contain exactly three archives and their checksums")
    for target in TARGETS:
        archive = DIST / archive_name(version, target)
        expected_line = f"{digest(archive)}  {archive.name}\n"
        if archive.with_name(archive.name + ".sha256").read_text(encoding="ascii") != expected_line:
            raise ValueError(f"checksum mismatch: {archive.name}")
    print("Verified all three release archives and SHA-256 checksums")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="command", required=True)
    subcommands.add_parser("metadata")
    subcommands.add_parser("verify")
    subcommands.add_parser("package").add_argument("--target", choices=TARGETS, required=True)
    args = parser.parse_args()
    if args.command == "metadata":
        version, commit = metadata()
        if output := os.environ.get("GITHUB_OUTPUT"):
            with open(output, "a", encoding="utf-8") as stream:
                stream.write(f"version={version}\n")
        print(json.dumps({"version": version, "commit": commit}))
    elif args.command == "package":
        package(args.target)
    else:
        verify()


if __name__ == "__main__":
    main()
