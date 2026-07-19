<div align="center">

<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://media.x.ai/v1/website/spacexai-symbol-white-transparent-0c31957f.png">
    <source media="(prefers-color-scheme: light)" srcset="https://media.x.ai/v1/website/spacexai-symbol-black-transparent-6435cf42.png">
    <img alt="SpaceXAI logo" src="https://media.x.ai/v1/website/spacexai-symbol-black-transparent-6435cf42.png" width="96">
  </picture>
  <br>
  Grox (<code>grox</code>)
</h1>

**Grox** is a community fork of
[SpaceXAI's Grok Build](https://github.com/xai-org/grok-build) that adds an
existing Codex/ChatGPT subscription as a coding-agent provider. The connector
uses the official `codex app-server` JSONL protocol, matching the
subscription-backed architecture used by
[t3code](https://github.com/pingdotgg/t3code); it does not copy browser tokens
or translate a subscription into an OpenAI API key.

Grok Build is SpaceXAI's terminal-based AI coding agent. It runs as a
full-screen TUI that understands your codebase, edits files, executes shell
commands, searches the web, and manages long-running tasks — interactively,
headlessly for scripting/CI, or embedded in editors via the Agent Client
Protocol (ACP).

[Installing Grox](#installing-grox) ·
[Using Codex](#using-codex) ·
[Building from source](#building-from-source) ·
[Documentation](#documentation) ·
[Repository layout](#repository-layout) ·
[Development](#development) ·
[Contributing](#contributing) ·
[License](#license)

![Grok Build TUI](https://media.x.ai/v1/website/universe-tui-screenshot-6f7a0837.png)

> Grox is not affiliated with or endorsed by xAI or OpenAI. Grok Build remains
> the upstream foundation and retains its original Apache-2.0 notices.

This repository contains the Rust source for the `grok` CLI/TUI and its agent
runtime. It is synced periodically from the SpaceXAI monorepo.

A small `SOURCE_REV` file at the root records the full monorepo commit SHA
for the version of the code present in this tree.

</div>

---

## Installing Grox

Grox currently installs from source. Rust is pinned by
[`rust-toolchain.toml`](rust-toolchain.toml). Install
[DotSlash](https://dotslash-cli.com) first because the upstream build uses it
for hermetic tools:

```sh
cargo install dotslash
git clone https://github.com/micic-mihajlo/grox.git
cd grox
cargo build -p xai-grok-pager-bin --release
mkdir -p ~/.local/bin
install -m 755 target/release/grox ~/.local/bin/grox
grox --version
```

The inherited xAI updater is disabled in Grox so it cannot overwrite the fork
with an official Grok binary. Update with `git pull` and rebuild.

## Using Codex

Install the official Codex CLI and authenticate once:

```sh
codex login
grox codex --status
```

Start an interactive subscription-backed session or run one prompt:

```sh
grox codex
grox codex -p "review this repository"
```

Grox discovers the models available to the current Codex account dynamically.
It runs Codex in its workspace-write sandbox by default. Host-wide access is an
explicit opt-in:

```sh
grox codex --full-access
```

Every run prints its Codex thread ID. Resume that thread later with:

```sh
grox codex --resume <THREAD_ID>
```

Use `--codex-binary /path/to/codex` when the CLI is not on `PATH`.

## Building from source

Requirements:

- **Rust** — the toolchain is pinned by [`rust-toolchain.toml`](rust-toolchain.toml);
  `rustup` installs it automatically on first build.
- **[DotSlash](https://dotslash-cli.com)** — required so hermetic tools under
  [`bin/`](bin/) (notably [`bin/protoc`](bin/protoc)) can download and run.
  Install it and ensure `dotslash` is on your `PATH` **before** building:

  ```sh
  cargo install dotslash
  # or: prebuilt packages — https://dotslash-cli.com/docs/installation/
  /usr/bin/env dotslash --help   # sanity check
  ```

- **protoc** — proto codegen resolves [`bin/protoc`](bin/protoc) via DotSlash,
  or falls back to a `protoc` on `PATH` / `$PROTOC`.
- macOS and Linux are supported build hosts; Windows builds are best-effort
  and not currently tested from this tree.

```sh
cargo run -p xai-grok-pager-bin              # build + launch the Grok Build TUI
cargo build -p xai-grok-pager-bin --release  # release binary: target/release/grox
cargo check -p xai-grok-pager-bin            # fast validation
```

Plain `grox` launches the inherited Grok Build TUI. On first launch it opens
your browser for Grok authentication — see the
[authentication guide](crates/codegen/xai-grok-pager/docs/user-guide/02-authentication.md).
`grox codex` instead uses the existing Codex CLI login.

## Documentation

Full online documentation is available at
[docs.x.ai/build/overview](https://docs.x.ai/build/overview).

The user guide ships with the pager crate:
[`crates/codegen/xai-grok-pager/docs/user-guide/`](crates/codegen/xai-grok-pager/docs/user-guide/)
— getting started, keyboard shortcuts, slash commands, configuration, theming,
MCP servers, skills, plugins, hooks, headless mode, sandboxing, and more.

## Repository layout

| Path | Contents |
|------|----------|
| `crates/codegen/xai-grok-pager-bin` | Composition-root package; builds the `xai-grok-pager` binary |
| `crates/codegen/xai-grok-pager` | The TUI: scrollback, prompt, modals, rendering |
| `crates/codegen/xai-grok-shell` | Agent runtime + leader/stdio/headless entry points |
| `crates/codegen/xai-grok-tools` | Tool implementations (terminal, file edit, search, ...) |
| `crates/codegen/xai-grok-workspace` | Host filesystem, VCS, execution, checkpoints |
| `crates/codegen/...` | The rest of the CLI crate closure (config, MCP, markdown, sandbox, ...) |
| `crates/common/`, `crates/build/`, `prod/mc/` | Small shared leaf crates pulled in by the closure |
| `third_party/` | Vendored upstream source (Mermaid diagram stack) — see below |

> [!IMPORTANT]
> The root `Cargo.toml` (workspace members, dependency versions, lints,
> profiles) is **generated** — treat it as read-only. Prefer editing per-crate
> `Cargo.toml` files.

## Development

```sh
cargo check -p <crate>        # always target specific crates; full-workspace builds are slow
cargo test -p xai-grok-config # per-crate tests
cargo clippy -p <crate>       # lint config: clippy.toml at the repo root
cargo fmt --all               # rustfmt.toml at the repo root
```

## Contributing

> [!NOTE]
> External contributions are not accepted. See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

First-party code in this repository is licensed under the **Apache License,
Version 2.0** — see [`LICENSE`](LICENSE).

Third-party and vendored code remains under its original licenses. See:

- [`THIRD-PARTY-NOTICES`](THIRD-PARTY-NOTICES) — crates.io / git dependencies,
  bundled UI themes, and **in-tree source ports** (including openai/codex and
  sst/opencode tool implementations)
- [`crates/codegen/xai-grok-tools/THIRD_PARTY_NOTICES.md`](crates/codegen/xai-grok-tools/THIRD_PARTY_NOTICES.md)
  — crate-local notice for the codex and opencode ports (license texts +
  Apache §4(b) change notice)
- [`third_party/NOTICE`](third_party/NOTICE) — vendored Mermaid-stack index
