# Captain Who: Scenario Gallery

[English overview](README.md) · [简体中文](README.zh-CN.md)

A closer look at research, writing, code review, browser interaction, and collaboration in Captain Who. Screenshots are examples of the interface, not evidence that a task or its results have been independently verified. Their language, model names, settings, and layout may differ from your installed version.

Full-access modes shown here are not a recommendation; start with default permissions and review approvals. Displayed token counts, cache hit rates, and costs are example local statistics, not benchmarks or provider bills.

## Research a topic with parallel subagents

![A presentation task with a five-step plan and four research subagents listed on the right.](assets/screenshots/parallel-research.png)

The example presentation task divides research on university history, academic research, notable people, and rankings among four subagents. The panel lets you inspect their progress; the resulting claims still need source checks.

![A generated campus illustration in the conversation beside active and completed research subagents.](assets/screenshots/generated-artwork.png)

The conversation shows an AI-generated campus illustration for the presentation, alongside subagent activity. This is generated artwork, not a photograph of a real campus or a verified architectural reference.

[Research and writing tutorial](public-docs/user/tutorials/research-and-write.md) · [Multi-Agent tutorial](public-docs/user/tutorials/use-multiple-agents.md)

## Coordinate independent conversations in an organization

![Independent member conversations on the left, an explanation of internal organization mail in the center, and the organization board on the right.](assets/screenshots/organization-research.png)

The organization view brings member conversations and a shared board together. Members keep independent conversations and exchange tasks and results through internal mail; this is separate from a conversation's temporary subagent tree. The screenshot illustrates the interface and mail tools, not a completed research result.

**Development-version feature:** organization behavior documented here is verified in the current development code, not guaranteed in every website installer. Current organizations do not use fixed execution wires or input/output gates. See the [organization guide](public-docs/user/capabilities/organizations.md) and [two-member research tutorial](public-docs/user/tutorials/create-an-organization.md).

## Review files and code beside the conversation

![A project conversation beside a Python binary-search diff, with the integrated terminal below.](assets/screenshots/code-review-terminal.png)

Review a proposed code change while keeping the conversation and project terminal visible.

![A Markdown document preview beside the conversation, with a response's token-usage popover open.](assets/screenshots/markdown-preview-tokens.png)

Read a Markdown file alongside the discussion and inspect usage for an individual response. This is a Markdown preview; Word, Excel, and PowerPoint files cannot be previewed directly in the Files panel.

<details>
<summary>More: project conversations and chat commands</summary>

![A project conversation with model and permission selectors in the composer.](assets/screenshots/workspace-chat.png)

Choose the conversation's model and check its permission mode before starting a task.

![A slash-command menu showing model, context compaction, and conversation-management actions.](assets/screenshots/chat-commands.png)

Use `/` to access model selection, context compaction, and conversation actions.

</details>

## Inspect a webpage and hand control back when needed

![An English conversation beside the built-in browser with the Captain Who website's contact dialog open.](assets/screenshots/browser-inspection.png)

Discuss the currently displayed webpage and its dialog without leaving the workspace.

![A website sign-in page in the built-in browser beside a request for user assistance.](assets/screenshots/browser-login-handoff.jpg)

The assistant requests user action at a website sign-in page. Complete authentication yourself; do not put passwords or verification codes into the conversation.

[Search and browser guide](public-docs/user/everyday-use/search-and-browser.md)

## Gather structured input

![An interactive question card with two suggested choices and a custom-answer field.](assets/screenshots/interactive-questions.png)

An MBTI-style questionnaire demonstrates question cards with choices and custom answers. The example is an interface demonstration, not a validated psychological assessment.

## Configure the tools and boundaries for a task

<details>
<summary>Skills, local MCP servers, and subagent templates</summary>

![A Skill installation dialog offering GitHub and local-folder sources.](assets/screenshots/install-skills.png)

Install reusable task instructions and resources from GitHub or an authorized local folder. Review the source before installing.

![MCP settings listing local servers, with Filesystem ready and the other connections disabled.](assets/screenshots/mcp-servers.png)

Check local MCP server status and enable the tools needed for a task. The current product supports local stdio servers; the listed examples do not imply support for every MCP transport or feature.

![Subagent settings showing an enabled visual-review specialist template with model and project assignments.](assets/screenshots/subagent-templates.png)

Save a specialist role, choose its model, and assign the template to a project.

</details>

<details>
<summary>Permissions and model API configuration</summary>

![Permission settings with default, full-access, and custom modes, file scopes, and approval options.](assets/screenshots/permissions.png)

Review file-access scope and approval rules. Default permissions are the starting point; a full-access selection in an example does not make it appropriate for your task.

![Model settings with endpoint, context-window, pricing, and image-input fields, plus a masked API token.](assets/screenshots/model-configuration.png)

Configure a model connection and the prices used for local estimates. The API token is masked in this example. Keep credentials in settings and out of shared screenshots or conversations.

</details>

<details>
<summary>Usage estimates and appearance</summary>

![A seven-day usage dashboard with a model filter, token totals, cache statistics, and estimated costs.](assets/screenshots/usage-costs.png)

Inspect local usage by model and date. These values describe this example's activity; they are not performance guarantees or a substitute for your provider's bill.

![Appearance settings with theme choices, a code-diff preview, and font and sidebar preferences.](assets/screenshots/appearance.png)

Choose a theme and inspect its code-diff colors before adjusting other display preferences.

</details>

[Capability limits](public-docs/user/reference/capability-limits.md) · [Safe agent usage](public-docs/user/best-practices/safe-agent-usage.md)

<details>
<summary>Historical interface: the retired visual workflow</summary>

![A historical workflow canvas connecting a coordinator, two copy editors, and a reviewer.](assets/screenshots/visual-workflow.png)

This early interface used connected workflow nodes. That workflow feature has been removed; this is not the current organization interface or its execution model. Current organizations use independent member conversations and internal mail, as shown above.

</details>

---

[Back to the English overview](README.md) · [返回中文介绍](README.zh-CN.md)
