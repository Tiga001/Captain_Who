# Captain Who

[简体中文](README.md) | English

Captain Who is a local-first desktop AI work assistant. Its interface is built with Electron, React, and TypeScript; Rust Core handles the agent, tool execution, and local storage.

## Download and source code

To use the app, download the macOS installer from the [Captain Who website](https://captainwhoagent.com/). The current official release supports Apple Silicon Macs running macOS 12 or later. See [Versions and releases](public-docs/releases/README.md) for installers, verification information, and upgrade instructions.

There is currently no Windows installer. This is an independently developed project, and there has not yet been enough time to complete the Windows version. No release date has been set.

This [GitHub repository](https://github.com/Tiga001/Captain_Who) is the active development repository for Captain Who. The project is open source under the [Apache License 2.0](LICENSE) and continues to receive updates. Development builds may be ahead of the installers available on the website. For the released app, refer to the website and [release notes](public-docs/releases/release-notes/README.md).

The linked developer and public documentation is currently in Chinese.

Current features include:

- Local projects, conversations, drafts, archiving, and full-text search
- Provider Profiles, OpenAI-compatible, Anthropic-compatible, and DeepSeek model connections, with safe switching between configurations
- Reading file, image, PDF, Word, presentation, and spreadsheet attachments
- Workspace search, unified file creation/update/deletion through FileChange, Git diffs, managed commands, and persistent Command Sessions
- File and command operations with approval controls, plus default, full-access, and custom permission modes
- Web search and page retrieval through Tavily
- A built-in terminal, an interactive browser, Managed Playwright, persistent browsing/download history, a download center, and browser settings
- User-configured stdio MCP servers, internal HostBridge capabilities, and bundled, installed, and workspace Skills
- Multi-agent collaboration, project-assignable agent templates, attachment/artifact sharing within an agent tree, read-only observation, and crash recovery
- Scheduled Automation: agent runs launched on structured schedules, with run history, attention tracking, and native notifications
- Persistent system notifications shared by regular tasks and automations, with sound, content previews, and status-based notification policies
- Managed artifact workflows for Word documents, spreadsheets, presentations, PDFs, and image generation
- Agent tool traces across turns, context compaction, Exact Archive, and long-term usage indicators
- Interface support for Simplified and Traditional Chinese, British and American English, Japanese, Korean, French, Italian, and Russian

## Architecture

```text
src/main/                 Electron main process, domain-specific IPC, browser/terminal bridges
src/preload/              Isolated Renderer Host API
src/renderer/             React interface
packages/protocol/        TypeScript cross-process data types
packages/host-api/        Host API types available to the Renderer
crates/protocol-rs/       Rust JSON-RPC protocol
crates/core/              Agent, tools, permissions, and SQLite storage
crates/core-server/       Core Server application boundary (application / transport / adapters)
crates/mcp-client/        MCP protocol, catalog, connections, and stdio transport
packages/artifact-runtime-node/  Node entry point for the managed Artifact Runtime
scripts/                  Component preparation, testing, packaging, signing, and release verification
public-docs/              Public documentation for users, integration developers, and support
```

In development, Electron launches `core-server` through Cargo. Production packages include the release binary in `process.resourcesPath`. Electron and Rust communicate using newline-delimited JSON-RPC.

The [developer documentation index](docs/README.md) covers architecture, subsystems, development, testing, releases, and security. New contributors should start with [Development setup](docs/development/getting-started.md), [Repository layout](docs/development/repository-layout.md), and [Architecture overview](docs/architecture/overview.md).

For product usage, integrations, release status, and troubleshooting, see the [public documentation index](public-docs/README.md).

## License

Captain Who's original code is released under the [Apache License 2.0](LICENSE). Third-party software, assets, and grammar files included in the repository and app packages retain their respective licenses. For attribution and notices, see [Third-party software notices](THIRD_PARTY_NOTICES.txt) and [Third-party grammar notices](THIRD_PARTY_GRAMMAR_NOTICES.txt).

## Requirements

- Node.js 22 (see `.node-version`)
- pnpm 11.10.0 (see `package.json#packageManager`)
- Stable Rust, including `rustfmt` and `clippy`
- A native build toolchain for your platform; both `node-pty` and the Rust sidecar require native compilation

Install dependencies:

```bash
pnpm install --frozen-lockfile
```

The first development launch compiles Rust and takes longer than subsequent launches:

```bash
pnpm dev
```

Once the app starts, open Settings → Configuration and enter your model API URL, token, model identifier, and optional Tavily API key. The repository no longer includes institution-specific endpoints or placeholder search keys.

## Common commands

| Command                         | Purpose                                                                            |
| ------------------------------- | ---------------------------------------------------------------------------------- |
| `pnpm dev`                      | Start the Electron development environment and Core Server                         |
| `pnpm format`                   | Format TypeScript, CSS, documentation, and Rust                                    |
| `pnpm check:docs`               | Check documentation metadata, links, paths, commands, and version sources of truth |
| `pnpm check:public-docs`        | Check public documentation structure, indexes, and scope boundaries                |
| `pnpm check:test-layout`        | Check test-file ownership and registration of ignored Rust tests                   |
| `pnpm lint`                     | Run ESLint                                                                         |
| `pnpm typecheck`                | Type-check Main, Preload, and Renderer                                             |
| `pnpm lint:rust`                | Run strict Clippy checks across the Rust workspace                                 |
| `pnpm test:unit`                | Run the Node Vitest unit project                                                   |
| `pnpm test:browser`             | Run the Vitest browser project using the pinned Chromium version                   |
| `pnpm test:electron`            | Run real Electron fixtures and Managed Playwright tests                            |
| `pnpm test:web`                 | Run the combined unit, browser, and Electron tests                                 |
| `pnpm test:automation-core-e2e` | Run dedicated end-to-end tests for the Automation Host API and real Core Server    |
| `pnpm test:rust`                | Run Rust workspace tests                                                           |
| `pnpm test`                     | Run the standard script, web/browser, and Rust tests                               |
| `pnpm check`                    | Run formatting, documentation, lint, type, Clippy, and test checks                 |
| `pnpm build`                    | Type-check and generate Electron build output in `out/`                            |
| `pnpm build:core`               | Build and verify the release Core Server binary                                    |
| `pnpm build:unpack`             | Produce an unpacked app for the current platform for packaging smoke tests         |

## Packaging

Installers must be built on a native runner for the target operating system. The scripts reject attempts to build Windows or Linux packages directly on macOS, preventing Rust binaries for the wrong platform from being included.

```bash
# Windows
pnpm build:win

# macOS
pnpm build:mac

# Linux
pnpm build:linux
```

`electron-builder.yml` includes only `out/`, runtime resources, production dependencies, and the platform-specific `core-server` in the app. Source code and the Cargo `target/` cache are excluded from the ASAR archive.

The macOS `build:mac` command enforces Developer ID signing, hardened runtime, and signature verification through the managed native-component and privacy release gates. The DMG container is also signed. The official 1.0.5 DMG available on the website has been notarized by Apple and has its notarization ticket stapled. The public update feed is enabled; when a new version is available, the app offers user-initiated download and installation. The repository's build command does not perform Apple notarization, and `pnpm check` does not include every dedicated release gate. A locally built artifact should not be treated as an official installer. See [Build and release](docs/development/build-and-release.md) for the release process.

## Local data and privacy

The Electron app uses the location returned by `app.getPath('userData')` as its single authoritative data root and explicitly passes it to Core Server at startup. The database is stored at `storage.sqlite` within this root. Attachments, installed Skills, generated images, and credentials for unsigned macOS development builds are stored in separate managed subdirectories alongside it. At startup, orphaned attachment files with no database references are removed. Electron resolves the actual path for the current operating system and application identity; application code does not independently infer macOS, Windows, or Linux paths.

When running `core-server` independently, `MYCOPILOT_STORAGE_DB` can still specify the database path. This is a testing and standalone diagnostics interface. The regular app launched through Electron overrides it with the data root supplied by the Host.

To rebuild the SQLite baseline during development, fully quit Captain Who first, then run the non-destructive preflight:

```bash
pnpm storage:reset-dev
```

After reviewing the preflight summary, explicitly confirm the rebuild:

```bash
pnpm storage:reset-dev -- --confirm-reset
```

The command uses Electron to resolve the same authoritative data root and refuses to proceed while the app or Core Server still holds the database. A confirmed rebuild first creates a timestamped backup in `storage-backups/` under the data root, with restricted permissions and SQLite validation, then atomically replaces the database with a fresh canonical database. Model and search configuration, UI/prompt preferences, Skill enablement, MCP server configuration, notification settings, browser download/link preferences, and valid image-generation profiles are restored through the current strict write paths. Conversations, projects, agent templates, drafts, browsing/download history, general notification records, automation tasks/runs/events/outbox entries, usage records, approvals, continuations, compaction state, and forks are not restored.

Attachment, installed Skill, generated-image, and credential directories are not deleted or moved as part of the rebuild transaction. Attachment records linked to removed conversations are not restored; their files are cleaned up by the existing orphaned-attachment policy on a subsequent normal app launch. The command prints only paths and counts, never tokens or configuration values.

The built-in browser uses a separate persistent session. Rust Core stores browsing history, download records, and opening preferences. Clear browsing data lets you remove history, cookies/site data, cache, or download records by category and time range. The Main process fetches site icons through a managed session; the cache is limited to 256 entries and 30 days.

Please note:

- Model tokens, Tavily keys, and image-generation API keys are stored separately from ordinary configuration. The current v33 SQLite schema stores only credential references, configuration status, and non-secret metadata. The Renderer receives credential status and values newly entered by the user, but cannot retrieve existing keys or references.
- Release builds with a stable signing identity use the operating system's credential store. Unsigned macOS development builds use a private file backend inside the data root, with directory permissions of `0700` and file permissions of `0600`. The entire data root should still be treated as sensitive.
- Current SQLite backups do not contain current model/search secrets, but historical backups from older schemas may still contain plaintext credentials. Restoring SQLite alone does not restore operating-system credentials. Clearing or deleting data does not guarantee secure erasure from SSDs, system backups, or operating-system credential stores. If a key may have been exposed, revoke or rotate it with the provider.
- Model requests are sent to your configured API URL. When web search is enabled, queries or target URLs are sent to Tavily.
- The built-in browser denies website requests for camera, microphone, location, notifications, and other system permissions by default.
- Removing a project permanently deletes its local conversations, messages, and attachments from Captain Who, but does not modify files in the project directory.
- Cost figures are local estimates based on the configured price per 1,000 tokens. They are not provider invoices and do not distinguish between currencies.

## Document and artifact support

- Agent attachment reading supports `.docx`, `.pptx`, `.xlsx`, `.csv`, `.tsv`, and other formats. File previews in the right sidebar support a different set of formats; see [Workspace files](docs/subsystems/workspace-files.md).
- Legacy `.doc`: parsed through the system `textutil` utility on macOS only.
- Legacy `.ppt` and `.xls`: not currently supported. Convert them to `.pptx`, `.xlsx`, or a text format first.
- PDF attachments support text extraction. Whether a scanned document can be read depends on whether it contains a text layer. More complex PDF processing is provided through the managed PDF Skill and command workflows.
- Word document, spreadsheet, and presentation creation/editing uses managed builders, editors, renderers, and artifact publication gates. See [Office and artifacts](docs/subsystems/office-and-artifacts.md).
