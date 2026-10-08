# Binary releases

All release targets are **AMD64 / x86_64**:

| Target | Archive | Compatibility |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | `.tar.gz` | Built on Ubuntu 22.04; use on compatible glibc systems (baseline glibc 2.35). |
| `x86_64-unknown-linux-musl` | `.tar.gz` | Static musl runtime; CI rejects ELF interpreter/shared-library dependencies. |
| `x86_64-pc-windows-msvc` | `.zip` | Windows x64 with the MSVC C runtime statically linked. |

Download from [GitHub Releases](https://github.com/irrit-us/Audit-Alchemist-Agent/releases).
Each archive contains the optimized `audit-harness` binary (`.exe` on Windows),
a minimal `node.toml`, quick-start instructions, licenses/skill attribution, and
`build-info.json` recording version, source commit, target, and Rust version.
Every archive has a sibling `.sha256` checksum file. Verify before extracting:

```sh
sha256sum -c audit-harness-v0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256
tar -xzf audit-harness-v0.1.0-x86_64-unknown-linux-musl.tar.gz
```

On Windows, compare `Get-FileHash -Algorithm SHA256 <archive.zip>` with the
checksum file, then use `Expand-Archive <archive.zip>`. Run the extracted CLI
with `./audit-harness` or `.\audit-harness.exe`, or add its directory to PATH.
Edit the included configuration for your provider, root, and target. Rust and
Python are not required to run the binary. Bash, optional debuggers, and MCP
server programs are separate host dependencies; Windows Bash can come from Git
for Windows or an explicit `AUDIT_BASH` path.

## CI and publication

[Release binaries](../.github/workflows/release.yml) runs on `v*` tag pushes and
manual dispatch. The tag must exactly match `v` plus `Cargo.toml`'s package
version. All three release builds run in parallel using `Cargo.lock`. The shared
CI workflow runs Linux/Windows Rust checks and debugger checks. Packaging smoke
tests run the **extracted release binary**, checking version, embedded skills,
TOML validation, and a credential-free audit preview from an independent directory.

Archives are uploaded as CI artifacts for 14 days. Only tag-push runs publish to
GitHub Releases, after every check and build succeeds and all three archive
checksums verify. Manual runs build artifacts without publishing. Tags containing
a SemVer prerelease suffix publish as prereleases. Only the publish job has
repository write permission; no personal access token is needed.

To release a new version, update `Cargo.toml`, regenerate and commit `Cargo.lock`,
record changes, and push a matching annotated tag:

```sh
git tag -a v0.1.0 -m 'Release v0.1.0'
git push origin v0.1.0
```

The workflow refuses to overwrite existing release assets. A failed build can
be rerun before publication; an interrupted publication that leaves a draft
requires inspecting that draft before retrying. Do not move published tags to
different source commits. SHA-256 checksums detect accidental corruption; they
are not a publisher signature or a reproducible-build guarantee.

Runtime linkage uses Rust's documented [CRT selection](https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes).
Publishing uses [`gh release create --verify-tag`](https://cli.github.com/manual/gh_release_create),
which uploads assets before publishing the release.
