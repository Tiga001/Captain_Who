<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="resources/brand-mark-dark.png">
    <img src="resources/brand-mark-light.png" width="88" alt="Captain Who sailboat logo">
  </picture>
</p>

<h1 align="center">Captain Who</h1>

<p align="center"><strong>An open-source desktop workspace for AI agents.</strong></p>
<p align="center">Research, write, and code with your own model APIs.</p>

<p align="center">
  <a href="https://github.com/Tiga001/Captain_Who/stargazers"><img src="https://img.shields.io/github/stars/Tiga001/Captain_Who?style=flat-square&amp;color=1f6feb" alt="GitHub stars"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/Tiga001/Captain_Who?style=flat-square&amp;color=1f6feb" alt="License: Apache-2.0"></a>
  <a href="public-docs/releases/supported-platforms.md"><img src="https://img.shields.io/badge/macOS-12%2B%20%C2%B7%20Apple%20Silicon-1f6feb?style=flat-square" alt="Official installer: macOS 12+, Apple Silicon"></a>
</p>

<p align="center">English · <a href="README.zh-CN.md">简体中文</a></p>
<p align="center">
  <a href="https://captainwhoagent.com/">Website &amp; download</a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="public-docs/README.md">Docs (中文)</a> ·
  <a href="GALLERY.md">Gallery</a> ·
  <a href="https://github.com/Tiga001/Captain_Who/issues">Feedback</a>
</p>

Captain Who brings conversations, local files, terminals, and a browser into one workspace. Work with one agent, delegate subtasks in parallel, or use **Organizations** to let independent conversations work together through an internal mailbox.

> **Availability:** official installers support Apple Silicon Macs on macOS 12+. This README also covers the current source, including Organizations and built-in local sign-in; those features may be ahead of the website's installers. See [release information](public-docs/releases/README.md) (中文). Bring your own model API; model usage credits are not included.

![Four subagents gathering material for a Zhejiang University presentation.](assets/screenshots/parallel-research.png)

_A presentation task split into four research assignments. This screenshot shows work in progress, not a finished slide deck._

## Try a task

Start with a folder of non-sensitive notes or reference material. Once connected to a model, ask:

```text
Read the materials in this project and save a summary as summary.md.
Include the main conclusions, supporting sources, and open questions.
Do not modify the original files.
```

Review any approval requests, then open `summary.md` beside the conversation and check its claims against the source files. No search service or multi-agent setup is needed for this example.

## Quick start

1. **Install.** Download from [captainwhoagent.com](https://captainwhoagent.com/). Windows, Intel Mac, and Linux do not currently have official public installers. To try the current development code, [run from source](#run-from-source).
2. **Sign in.** In a source build, choose account/password login and enter `captainwho` for both fields. These public local credentials need no cloud registration or online license check. For downloaded installers, follow the account flow available in that version. See [account and access](public-docs/user/getting-started/account-and-license.md) (中文).
3. **Connect a model.** In **Settings → Configuration**, enter your API URL and token. In **Manage models**, add the model ID accepted by your provider and enable it. A fresh installation has no configured models.
4. **Choose a project and send the task.** Open **New chat → project selector → New project** and select a local folder. Choose your model and keep the default permissions. Review approvals and the resulting files.

Model connections include **OpenAI Chat Completions-compatible** and **Anthropic Messages-compatible** APIs, plus DeepSeek and Moonshot configurations. Compatibility depends on the provider's implementation. Web search and image generation need separate service configuration.

[Installation](public-docs/user/getting-started/installation.md) · [Model setup](public-docs/integrations/model-provider-integration.md) · [First-task guide](public-docs/user/getting-started/first-task.md) — guides in Chinese.

## Organizations: independent conversations, one team

**Available in the current development source; check your installed version.**

Instead of drawing a fixed route between agents, give each member a role and a mailbox. Members can ask for help, exchange findings, and send work to one another as the task develops.

- **Independent conversations.** Each member keeps its own conversation, model, responsibilities, and tool permissions. You can open and talk to any member directly.
- **Internal mail.** Members read and send messages inside Captain Who. No external email account is involved, and departments do not restrict who can exchange messages.
- **A team that can change.** Members with management permissions can create departments and add, edit, or remove lower-ranked members within their scope. Management authority does not grant unrestricted tool access.

For a small research team, create a template with a **Researcher** and a **Reviewer**, assign models, then activate it. Give the Researcher two public documents and ask:

```text
Compare the approaches in these two documents. Send your findings and sources
to the Reviewer through organization mail and ask them to check the evidence.
Use their feedback to give me a short conclusion and list unresolved questions.
Do not send external messages or modify files.
```

![Independent research conversations on the left, organization mail tools in the center, and departments and members on the right.](assets/screenshots/organization-research.png)

_A larger research organization, with separate members for method extraction, applicability assessment, mathematical review, numerical validation, and archiving._

**Subagents and Organizations solve different problems:** subagents handle delegated work within a parent conversation; Organizations connect independent conversations into a team. Organization members can still delegate to subagents. The extra coordination can consume more tokens, so a single agent may be enough for a small task.

[Create an organization](public-docs/user/tutorials/create-an-organization.md) · [Organization behavior and permissions](public-docs/user/capabilities/organizations.md) — guides in Chinese.

## More in the workspace

| Capability                                                                                                                                 | What you can do                                                                                                                  |
| ------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------- |
| [Files, code, and terminal](public-docs/user/capabilities/tools.md)                                                                        | Read and edit project files, run commands, inspect Git diffs, and review changes beside the conversation.                        |
| [Browser](public-docs/user/capabilities/browser-automation.md) and [human interaction](public-docs/user/capabilities/human-interaction.md) | Let an agent work on webpages and ask for your input or a login handoff when needed.                                             |
| [Skills](public-docs/user/capabilities/skills.md) and [MCP](public-docs/user/capabilities/mcp.md)                                          | Add reusable task methods and connect external tools. Use relevant Skills for documents, spreadsheets, slides, PDFs, and images. |
| [Subagents](public-docs/user/capabilities/multi-agent.md)                                                                                  | Delegate research, drafting, or review in parallel, with reusable specialist templates.                                          |
| [Scheduled tasks](public-docs/user/capabilities/automations.md)                                                                            | Set up recurring work and inspect run history. Scheduled execution requires the app to be running.                               |
| [Context](public-docs/user/learn/context-management.md) and [usage](public-docs/user/reference/settings.md#用量与费用估算)                 | Manage long conversations and review model usage, cached tokens, and estimated costs.                                            |

Capability guides are in Chinese. Models, permissions, and optional services remain configurable.

![A Python code diff beside the conversation, with the project terminal below.](assets/screenshots/code-review-terminal.png)

_Review a file change without leaving the task. [See more screenshots →](GALLERY.md)_

## Before you start

- **Local-first is not offline.** Model providers and enabled search, image, browser, or MCP services receive the data needed for their tasks. With multiple model providers in an organization, shared background and mail can reach more than one provider. See [data and permissions](public-docs/security/data-and-permissions.md) (中文).
- **Permissions are not an OS sandbox.** Default mode is not read-only. Start with default permissions and review requests to change files, run commands, or act externally. Full-access settings in screenshots are examples, not a recommendation. Stopping a task does not undo completed actions.
- **Check the output.** Verify sources and file changes. Skills can create or process Office files, but the side-panel file viewer does not render Word, Excel, or PowerPoint files directly; open them in a suitable app. See [document support](public-docs/user/capabilities/artifacts-and-office.md) (中文).
- **Watch usage and credentials.** Costs are estimates, not provider bills, and screenshot cache rates are not benchmarks. Enter API keys in settings, not in chats, Issues, or screenshots. Only install trusted Skills and MCP servers.

## Run from source

The desktop UI uses Electron, React, and TypeScript; Rust Core handles agents, tools, permissions, and local storage.

You need **Node.js 22**, **pnpm 11.10.0**, Rust installed through **rustup**, and your platform's native build tools. The repository pins **Rust 1.99.0**, including `rustfmt` and `clippy`, in [rust-toolchain.toml](rust-toolchain.toml).

```bash
git clone https://github.com/Tiga001/Captain_Who.git
cd Captain_Who
pnpm install --frozen-lockfile
pnpm dev
```

The first launch prepares managed components and compiles Rust. Then follow [Quick start](#quick-start) for local sign-in and model setup. Build commands for other operating systems do not imply official support.

For contributors, `pnpm check` runs the standard checks and tests; `pnpm test:automation-core-e2e` separately checks scheduled tasks with Core Server.

[Development setup](docs/development/getting-started.md) · [Architecture](docs/architecture/overview.md) · [Testing](docs/development/testing.md) · [Build and release](docs/development/build-and-release.md)

## Documentation and contributing

- [User documentation](public-docs/README.md): installation, everyday tasks, capabilities, and safety (中文).
- [Developer documentation](docs/README.md): architecture, subsystems, testing, and maintenance (Chinese and English).
- [GitHub Issues](https://github.com/Tiga001/Captain_Who/issues): bugs and feature requests. Include your OS, app version, and reproduction steps; remove keys and private content.

Captain Who is an independently developed project. Contributions to code, documentation, translations, and reproducible bug reports are welcome. Run the checks relevant to your changes before submitting a pull request.

## License

Captain Who's original code is available under the [Apache License 2.0](LICENSE). Third-party software and assets retain their own licenses; see [software notices](THIRD_PARTY_NOTICES.txt) and [grammar notices](THIRD_PARTY_GRAMMAR_NOTICES.txt).
