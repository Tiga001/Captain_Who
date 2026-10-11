第四轮最终证据，2026-10-11；实现源码 e787b88d045549ea5d204f5c01f77f391b7cfc49。

metadata.json、cases.jsonl.gz、resources.jsonl、build/test stdout/stderr 压缩日志
来自完整有效运行 round4-full-20261011-r1：36 场景、退出 0、运行期间源码保持不变。
comparison.json.gz / comparison.txt / comparison.csv 对比第三轮归档。
summary.json 是比较和元数据生成的摘要，不是 runner 自带产物。
compare-results.py 检查负载、成功接纳/持久终态/重开计数、错误与已报告容量。
分布变化会独立报告，不能假设拒绝/终态必须等同；本次聚合计数恰好逐场景一致。
并发审批成功 owner 有替换，不能视为逐 owner 配对的成功响应延迟比较。

复算（在仓库根目录执行，必须使用新的输出前缀）：
python3 docs/performance/backend-concurrency/results/2026-10-11-round4/compare-results.py docs/performance/backend-concurrency/results/2026-10-11-round4 --output-prefix .cache/backend-concurrency/new-round4-recomparison
python3 docs/performance/backend-concurrency/results/2026-10-11-round4/compare-results.py --self-test

regressions/：最终构建、Clippy、格式、布局、文档、基础设施和 release 测试日志。
accepted-regression-summary.json：37 组，1628 次通过，1622 个不同名字，0 失败，1 既有 ignored。
最终联合构建与完整矩阵独立 Server 测试构建的二进制名/hash不同，详见结果页。
run-regressions.py / summarize-regressions.py / archive-round4.py 为历史执行脚本。
复跑前将前两个脚本复制到新的验证目录；输出依赖脚本所在目录，不能直接在归档内执行。
run-regressions.py 从固定隔离 target 自动选最新可执行文件；先核对当前 Cargo 输出与源码/hash。
summarize-regressions.py 需要未压缩日志及当时对应二进制；其已有输出可直接读取。
archive-round4.py 固定本轮输出目录且拒绝覆盖；复用须改为新的目录。
文档收尾检查另存 docs-delivery.log.gz；仅改文档后不重复业务测试。

诊断目录只保留早期失败/修复证据。它们使用不同源码状态，不能合并进最终通过数或性能样本。
预期故障注入可能打印 worker panic；以完整测试结果和退出码判断。

测量范围：Core 入口到回执；n=1 仅单次观察，stage p95 为桶上界。
固定输入稳态不是最大吞吐；有限 owner/慢工具样本不证明持续无饥饿。
源码/回归保留逻辑 cap 50，但矩阵无独立逻辑峰值观测，summary 中保持 null。
RSS/线程为低频进程样本，不含命令子进程，不是精确峰值或总资源上限。
两组过载场景未自然排空，关机/恢复后才完成终态；不将清理前队列快照当关闭结果。
DB累计等待、部分Stop/审批与稳态最大延迟、CPU/RSS有回退，详见 ../../round4-results.md。

SHA256SUMS 覆盖本目录除自身以外全部文件。没有数据库、密钥、模型正文或编译二进制归档。
第五、六轮未开始或完成，本目录只为第四轮实现与本轮验证提供证据。
