# Security Policy

## Reporting a vulnerability

Report vulnerabilities privately to **contact@softwarehive.ro**, or with
**Report a vulnerability** under this repository's Security tab. Please do not
open a public issue for anything that could be a vulnerability: a file that
makes a tool run code, write outside its output folder or overwrite an
original, or a way to make the app download something other than the engine
it pins.

The aim is an acknowledgement within 72 hours. Please include steps to
reproduce, the file that triggers it if you can share it, and the version you
tested.

## Scope

MQKit converts files on the PC it runs on. There is no MQKit server and no
account. Three things reach the network:

- **The version check.** Once a day, unless turned off in Settings, the app
  reads `https://mqkit.com/latest.json`. The request carries only the app's
  version, in its User-Agent. A newer version is announced with a link to the
  website; the app never updates itself.
- **Engine downloads.** The first time a tool needs FFmpeg, LibreOffice, a
  portable Python or an AI model, the app downloads it from the address in
  `src-tauri/engines.toml` or `src-tauri/engines.python.toml`, over HTTPS only,
  and refuses it unless its size and SHA-256 match the catalog.
- **A link the person pastes.** "From a link…" downloads that one file into
  the Downloads folder (share links of the common cloud drives are turned into
  their direct download first). Nothing is sent but the request for that link.

Every tool runs in a separate worker process, apart from the app. Originals
are never changed unless the person chooses to replace them.

## Supported versions

Only the latest release receives fixes. Every release is free and complete,
so a security fix reaches everyone the same way.
