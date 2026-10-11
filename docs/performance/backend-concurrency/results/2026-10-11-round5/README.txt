第五轮证据归档；最终结论以本轮结果页为准。

根目录为 full matrix 原始 metadata/cases/resources、stdout/stderr 和 comparison。
accepted/ 为明确选定的 TS/browser/Host 检查、Clippy、格式与基础设施门禁。
regressions/accepted-regression-summary.json 是唯一最终测试计数入口。
build-records/ 保留不同构建阶段记录；测试与生产二进制可能不同，不能混用其 SHA。
measurements/host/ 是真实隔离 Electron 管道窄读取；各 20 RPC，非饱和/paint/SLO 证据。
measurements/renderer/ 是 advisory Node/Vite 序列化样本，不能作隔离 CPU 门禁。
measurements/refresh/ 含受控浏览器结果日志、数据、配置与 baseline；不复制截图，源截图保持原样。
diagnostics/checkpoint-first/ 单列早期 checkpoint 门禁；其余早期失败也仅供排错。
diagnostics/ 中的源码阶段、二进制、失败和通过数不得混入最终回归或性能样本。
audits/ 保存工作说明，可能描述早期阶段；以最终摘要和原始日志交叉核对。

复算比较（仓库根目录；输出必须是未使用的新前缀）：
python3 docs/performance/backend-concurrency/results/2026-10-11-round5/compare-results.py docs/performance/backend-concurrency/results/2026-10-11-round5 --baseline docs/performance/backend-concurrency/results/2026-10-11-round4 --output-prefix .cache/backend-concurrency/round5-recomparison-new
python3 docs/performance/backend-concurrency/results/2026-10-11-round5/compare-results.py --self-test

scripts/ 保留本轮执行脚本。run-regressions.py/summarize-regressions.py 必须先复制到
新的隔离验证目录，核对其中 binary/target 路径和最终 SHA 后才可运行，勿写归档目录。
archive-evidence.py 默认只 preflight；审阅选定日志后用 --write 显式创建全新输出目录。
它运行时读取最新证据，核对 Host binary receipt，拒绝已有目标路径，不删除任何文件。
在缺少最终 matrix/comparison/accepted summary 时会拒绝生成不完整的最终归档。

全部原始 log，以及 >= 65536 字节的 jsonl 和 cases/comparison JSON 使用 gzip mtime=0；
压缩前后路径及 SHA 见 archive-manifest.json。SHA256SUMS 覆盖自身之外全部文件。
没有数据库、密钥或可执行二进制归档。内部一致性通过不代表整体性能或 SLO 达标。
