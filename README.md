# MQKit

**Convert files on your own PC.** MQKit (Morph Quick Kit) is a free file converter for Windows:
44 tools for PDFs, images, video, audio, documents and more. It works offline, so your files never
leave your PC. No ads, no account, no size limit.

Made by [Software Hive S.R.L.](https://softwarehive.ro), Romania.
Download, the list of tools, questions and privacy: **[mqkit.com](https://mqkit.com)**.

## What it does

Each tool has a permanent number (ID:1 to ID:44), shown in the app and on the site.

- **PDF:** compress, to Word, merge, split, pages, password, stamp, OCR, to Excel, to PowerPoint, to images.
- **Images:** compress, convert, resize, HEIC to JPG, AI upscale, remove background, RAW, AVIF, SVG, icons,
  design files, images to PDF.
- **Video and audio:** compress, convert, to GIF, trim, merge, subtitles, video to MP3, audio convert,
  speech to text, text to speech.
- **Documents:** Word and other Office files to PDF, HTML to PDF, Markdown, tables, e-books, mail.
- **Other:** archives, fonts, remove metadata, CAD and 3D.

The heavy engines (LibreOffice, FFmpeg, the AI models, a portable Python) are downloaded only when a tool
first needs them, from their official sources, and checked by SHA-256.

## Privacy

Conversion happens on your PC, and your files are not uploaded anywhere. What the app asks of the internet,
and when, is listed in [SECURITY.md](SECURITY.md): a daily version check (it can be turned off), the components a
tool needs, a link you paste, and the pictures a web page you print links to.

## How it is built

- `src-tauri`: the app (Tauri 2, Rust): tools, jobs, file naming, engines downloaded on demand
  (`engines.toml`, `engines.python.toml`), the Explorer right-click menu, the new-version notice.
- `ui`: the interface (Svelte 5).
- `crates/contract`: the plugin contract's types and conditions.
- `crates/worker-core`: the tools written in Rust.
- `crates/whisper`, `crates/hb-subset`: speech to text (whisper.cpp) and font subsetting (HarfBuzz), vendored.
- `workers/office-worker`: Word, Excel and PowerPoint through COM (C#, .NET Framework 4.8).
- `workers/python`: tools that run on the portable Python.
- `workers/keywords.json`: search words for each tool, in several languages.

## Build

Needs Rust, Node.js, the .NET Framework 4.8 SDK and Visual Studio Build Tools (with CMake).

```bash
npm install && npm --prefix ui install
npm run dev                       # the workers and the app, with hot reload
python scripts/conformance.py     # the tools' conformance tests
```

```powershell
.\scripts\package.ps1             # test installer, unsigned: dist\dev\
.\scripts\package.ps1 -Release    # signed release, from a clean tree tagged v<version>
```

Releases are signed with Software Hive's code-signing certificate (`scripts/sign-windows.ps1`).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Contributions are accepted under the [CLA](CLA.md); you keep the
copyright to your work. Security issues: [SECURITY.md](SECURITY.md).

## License

Copyright (C) 2026 Software Hive S.R.L. MQKit is free software under the GNU Affero General Public License,
version 3 or later ([LICENSE](LICENSE)). What it is built from, and the licences of the engines it downloads:
[THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt). The MQKit name and logo belong to Software Hive S.R.L.
