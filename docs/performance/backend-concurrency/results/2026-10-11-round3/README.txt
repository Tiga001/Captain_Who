第三轮最终证据，2026-10-11；源码 354762d6fae3f8959027b10069a972c9b44b427f。

metadata.json、cases.jsonl.gz、resources.jsonl、test.stdout.log.gz、test.stderr.log.gz
来自唯一完整有效运行 round3-full-20261011-r1，36 场景、退出 0、源码运行期间不变。
comparison.json.gz / comparison.txt / comparison-metrics.csv 对比第二轮归档，处理量相同。
compare-results.py 检查场景、负载与完成计数，不一致则非零退出。不要混用小样本与可靠 p95/p99。
summary.json 只裁去 comparison.after 的逐场景大块，保留汇总与完整性。

regressions/：最终构建、Clippy、格式、布局、文档、基础设施和 release 测试日志。
accepted-regression-summary.json：26 组，1478 次通过，1471 个不同名字，0 失败，1 既有 ignored。
run-server-regressions.py 固定当时的测试二进制名，默认写验证缓存；复跑须先构建并核对路径。
文档收尾后的检查另存 docs-delivery.log.gz，源码已冻结，不重跑相同业务测试。

diagnostics/：开发中失败摘要、Provider shutdown 同步等待采样、独立线程峰值诊断。
它们只解释修复/局限，不能计入最终通过数、吞吐或响应分位数。
thread-burst 的 macOS sample/high-frequency ps 扰动运行；正式矩阵峰值 63，诊断 ps 峰值 80，
稍后 sample 栈 55 条（10 Tokio async、26 Tokio blocking idle、13 固定执行器、1 SQLite writer、
1 command settlement、4 test/fixture/system）；这些时刻不同，不能逐条反推正式峰值。

SHA256SUMS 覆盖本目录除自身以外的归档文件。没有数据库、密钥或编译二进制纳入归档。
详细方法、边界、失败修复与回退见 ../../round3-results.md。
