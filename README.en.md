# Captain Who

[简体中文](README.md) | English

**Research, create documents, and edit code with AI—in your local projects.**

Captain Who is a local-first desktop AI work assistant. Connect your own model API and describe a task in natural language. It can read and write files, run commands, operate the built-in browser, and delegate work to multiple agents in parallel. Conversations, files, terminals, and execution records share one workspace.

[Website & download](https://captainwhoagent.com/) · [Interface & features](#interface--features) · [Quick start](#quick-start) · [Run from source](#run-from-source) · [User documentation](public-docs/README.md)

![Four subagents research different topics for a presentation about Zhejiang University.](assets/screenshots/parallel-research.png)

Four subagents gather material on university history, academic research, notable people, and rankings in parallel for a presentation.

## Download and project status

Download the macOS installer from **[captainwhoagent.com](https://captainwhoagent.com/)**. The current release supports **Apple Silicon Macs running macOS 12 or later**.

This is a solo-developed project. There has not yet been enough time to complete the Windows version, so no Windows installer or release date is available.

This is Captain Who's active development repository, with ongoing updates under the [Apache-2.0](LICENSE) open-source license. Development code may be ahead of the website's installers; see [Releases and upgrades](public-docs/releases/README.md) for released versions.

> Starting an agent task requires a Captain Who account, a valid software license, and your own model API configuration. Model usage credits are not included.

## Interface & features

These screenshots show real usage, grouped by scenario. Model names, settings, and interfaces may differ in your installed version. Full-access permissions shown in screenshots are not a recommendation; start with the default permissions.

### Work with files and code

Work in a local folder, view files beside your conversation, review code changes, and use the built-in terminal.

![A project conversation, Python file diff, and built-in terminal displayed together.](assets/screenshots/code-review-terminal.png)

Review added code in a binary-search script with the project terminal open below.

<details>
<summary>More: project conversations, document previews, and shortcuts</summary>

![A project conversation with model and permission selectors below the input.](assets/screenshots/workspace-chat.png)

Start a project conversation, ask what the assistant can do, and check the selected model and permissions.

![A Markdown preview beside the conversation, with usage statistics expanded for one response.](assets/screenshots/markdown-preview-tokens.png)

Discuss a document while previewing its Markdown file and checking the response's token usage and cache hit rate.

![A slash-command menu with model, context compaction, and conversation options.](assets/screenshots/chat-commands.png)

Type `/` to switch models, compact context, or manage the current conversation.

</details>

### Research in parallel and create assets

Subagents can research, write, and review independently. Use the relevant Skills for documents, spreadsheets, presentations, PDFs, and images.

![A generated campus illustration in the conversation, with subagent progress on the right.](assets/screenshots/generated-artwork.png)

Generate a campus illustration for a presentation and add focused research on Yuquan Campus after a follow-up request.

### Browser and human collaboration

The agent can read and interact with pages in the built-in browser, using interactive cards to ask for your judgment or help when needed.

![An English conversation beside the built-in browser, with a website contact dialog open.](assets/screenshots/browser-inspection.png)

Ask the assistant to inspect the official website and describe the page and its open dialog.

<details>
<summary>More: login handoff and interactive questions</summary>

![A website login page in the browser and an interactive card requesting user assistance.](assets/screenshots/browser-login-handoff.jpg)

When an email check reaches a login page, the assistant asks you to sign in before continuing.

![A multi-question interactive card with preset choices and custom answers.](assets/screenshots/interactive-questions.png)

Answer an MBTI-style questionnaire that demonstrates interactive questions with choices and custom responses.

</details>

### Skills, MCP, and subagent templates

Skills package task methods and resources, MCP connects external tools, and subagent templates save reusable roles and model settings.

![The Skill installation dialog offers GitHub and local-folder sources.](assets/screenshots/install-skills.png)

Install a Skill from a public GitHub repository or a local folder to extend how the assistant works.

<details>
<summary>More: MCP connections and specialist templates</summary>

![MCP settings list local servers, their switches, and connection status.](assets/screenshots/mcp-servers.png)

Manage local MCP servers, with filesystem tools ready and the other connections disabled.

![An enabled visual-review specialist template in the subagent settings.](assets/screenshots/subagent-templates.png)

Enable a visual-review specialist template, choose its model, and assign it to a project.

</details>

### Models, permissions, and usage

Choose your model provider, set file-access boundaries and approval rules, and review model usage and estimated costs.

![Permission settings with default, full-access, and custom modes, file scopes, and approval options.](assets/screenshots/permissions.png)

Configure permission modes and adjust the custom mode's file-access scope and approval rules.

<details>
<summary>More: model configuration, usage statistics, and appearance</summary>

![Model settings show the model ID, context window, pricing, image input, and API configuration.](assets/screenshots/model-configuration.png)

Configure a model's API connection, context window, image-input support, and prices used for cost estimates.

![The usage panel shows input, output, cache, and cost statistics by date.](assets/screenshots/usage-costs.png)

Review seven days of model usage, cache hit rates, and estimated costs.

![Appearance settings with theme choices, a code-diff color preview, and display preferences.](assets/screenshots/appearance.png)

Choose a dark theme, preview code-diff colors, and adjust font smoothing and sidebar effects.

</details>

### How collaboration has evolved

The current development version uses **Organization**: independent agents communicate by role through a free-form mail network, exchanging tasks and results. This differs from temporary subagents dispatched within a regular conversation. See the [Organization development documentation](docs/subsystems/workflow-authoring.md) for the design. Development features may not yet be available in the website's installers.

<details>
<summary>Early workflow interface (historical screenshot, not the current Organization interface)</summary>

![An early workflow canvas connects a copy coordinator, two editors, and a reviewer.](assets/screenshots/visual-workflow.png)

An early version used connected nodes for coordination, editing, and review; current Organization collaboration no longer uses this wiring mechanism.

</details>

You can also set up recurring tasks under Scheduled and review run history and notifications. The app must remain running for scheduled execution. See [Scheduled tasks](public-docs/user/capabilities/automations.md).

## Quick start

1. Download and install from the [website](https://captainwhoagent.com/), sign in, and confirm that your software license is valid.
2. Open **Settings → Configuration** and enter your model API URL and token. Under **Manage models**, add the actual model ID, select the matching provider configuration, and enable the model. A fresh installation has no configured models.
3. Create a conversation, choose a local folder as the project, select a model, and keep the default permissions.
4. Describe the task and output location. If needed, choose a Skill through **“+” → Skills** in the input area, then review approvals and the final files.

Try a simple task:

```text
Read the materials in this folder and save a summary as summary.md.
Include the main conclusions, supporting sources, and open questions.
Do not modify the original files.
```

Model connections support OpenAI-compatible, Anthropic-compatible, DeepSeek, and Moonshot configurations. Web search and image generation require separate service configuration; they are not included in the chat model setup.

[Installation and first launch](public-docs/user/getting-started/installation.md) · [Connect a model](public-docs/integrations/model-provider-integration.md) · [Your first task](public-docs/user/getting-started/first-task.md)

## Data and usage boundaries

- **Local-first does not mean offline.** Signing in does not sync projects, conversations, or model keys to the Captain Who account service. Model providers, search and image services, websites, and MCP services still receive the data needed for their tasks. See [Data and permissions](public-docs/security/data-and-permissions.md).
- **Permissions are not an operating-system sandbox.** Default mode is not read-only. Review approvals for file changes, commands, and external actions. Stopping a task does not undo completed actions.
- **Review the output.** Skills can work with Word, Excel, and PowerPoint files, but the right-side file viewer cannot render them directly; inspect finished files in their respective apps. Costs are local estimates, not provider bills, and screenshot cache hit rates are not performance guarantees.
- **Protect credentials.** Enter API tokens only in settings, not in conversations, Issues, or public screenshots. Install Skills and MCP servers only from trusted sources.

## Run from source

The interface uses Electron, React, and TypeScript. Rust Core handles agents, tool execution, permissions, and local storage.

You need Node.js 22, pnpm 11.10.0, Rust stable with `rustfmt` and `clippy`, and your platform's native build toolchain.

```bash
git clone https://github.com/Tiga001/Captain_Who.git
cd Captain_Who
pnpm install --frozen-lockfile
pnpm dev
```

The first launch prepares managed components and compiles Rust. Development runs also require an account, a software license, and model configuration. Installers must be built on their target operating system; Windows/Linux build commands do not imply official support for those platforms.

Common checks: `pnpm check` runs the standard checks and tests; `pnpm test:automation-core-e2e` separately checks end-to-end behavior for scheduled tasks and Core Server.

[Development setup](docs/development/getting-started.md) · [System architecture](docs/architecture/overview.md) · [Testing](docs/development/testing.md) · [Build and release](docs/development/build-and-release.md)

## Documentation and feedback

The linked user and developer documentation is currently in Chinese.

- [User documentation](public-docs/README.md): installation, everyday use, capabilities, and safety.
- [Developer documentation](docs/README.md): architecture, subsystems, testing, and maintenance conventions.
- [GitHub Issues](https://github.com/Tiga001/Captain_Who/issues): include your operating system, app version, and reproduction steps, with credentials and private information removed.

Issues and pull requests are welcome for features, fixes, and documentation improvements. Run the checks and tests relevant to your changes before submitting code.

## License

Captain Who's original code is released under the [Apache License 2.0](LICENSE). Third-party software and assets retain their respective licenses; see [Third-party software notices](THIRD_PARTY_NOTICES.txt) and [Third-party grammar notices](THIRD_PARTY_GRAMMAR_NOTICES.txt).
