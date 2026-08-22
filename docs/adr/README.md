---
status: current
audience: maintainers
owner: engineering
last_verified: 2026-08-23
---

# Architecture Decision Records

本目录记录已经接受、并持续影响实现的架构决策。当前态行为仍由 `architecture/`、`subsystems/`、代码
和测试说明；ADR 只解释选择背景与长期后果。

## 命名

使用 `NNNN-short-title.md`，编号只增不复用。例如：

```text
0001-rust-core-sidecar.md
0002-canonical-storage-reset-policy.md
```

## 模板

```markdown
---
status: proposed
audience: maintainers
owner: engineering
last_verified: YYYY-MM-DD
---

# NNNN：标题

## 背景

## 决定

## 后果

## 备选方案

## 相关代码与文档
```

接受后将 `status` 改为 `accepted`。被替代时改为 `superseded` 并链接新 ADR，不删除旧记录。
