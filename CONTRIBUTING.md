# Contributing to MQKit

Thanks for considering a contribution. MQKit is a free file converter for
Windows that works offline: a Tauri 2 app (Rust and Svelte 5) that runs each
tool in a separate worker process. See the [README](README.md) for how the
pieces fit together.

## Contributor License Agreement

Contributions are accepted under the terms of [CLA.md](CLA.md). Opening a
pull request constitutes acceptance; you keep the copyright to your work.
Please read it once before your first PR. It is short and written to be
readable.

## Dev setup

Needs Rust (the version in `rust-toolchain.toml`), Node.js, the .NET Framework
4.8 SDK and Visual Studio Build Tools with CMake.

```bash
npm install && npm --prefix ui install
npm run dev
```

The engines some tools need (FFmpeg, LibreOffice, a portable Python, the AI
models) are downloaded by the app the first time a tool asks for them.

## Before opening a PR

```bash
npm --prefix ui run check
node scripts/build-workers.mjs
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo deny check
```

This is what CI runs. If these pass locally, CI should too.

## Making changes

- Keep PRs focused. A bug fix doesn't need an accompanying refactor.
- Every tool has a permanent number (ID:1 to ID:44, `ui/src/lib/numbers.ts`).
  People quote them; a number is never reused or moved.
- The interface speaks several languages, all equal. A new text gets every
  language in `ui/src/lib/i18n.svelte.ts`, not only English.
- Nothing leaves the PC. A change that makes the app contact a server, other
  than the ones SECURITY.md lists, will not be accepted.
- A new dependency must pass `cargo deny check`: permissive licences, or a
  copyleft one the AGPL can take in, named in `deny.toml`. After adding one,
  run `npm run notices` and commit `THIRD-PARTY-NOTICES.txt`.
- Comments explain *why*, not *what*: an invariant, a workaround, a
  constraint the code cannot express.

## Reporting security issues

Please don't open a public issue for a security vulnerability. See
[SECURITY.md](SECURITY.md).
