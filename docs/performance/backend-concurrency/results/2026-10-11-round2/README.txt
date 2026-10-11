Round 2 backend blocking isolation evidence
Source: d7b3fbaf4bc5169036a405045a87f4af371bdba2; baseline: acae06623b7ce6031b90795e730968fb36e38107.
Main cases/metadata/resources describe the final valid 36-case matrix only.
comparison-metrics.csv has exact success-only p95 values; n=1 is a single observation.
comparison.json.gz includes full per-case stage and queue data.
Raw test log bytes are gzip-compressed without whitespace rewriting. CSV uses LF line endings.
regressions contains 21 groups / 223 test executions / 221 distinct tests, plus the earlier 20-run receipt check.
attempts retains invalid first-matrix and partial-fix long50 evidence plus the passing targeted check. Their metrics must not be mixed into the final matrix.
interrupt-receipt-timeout.sample.txt.gz documents the earlier test polling starvation; the production single-connection constraint remains.
Read ../../round2-results.md and ../../round2-blocking-lifecycle.md for methodology, lifecycle, explicit regressions and next-round gates.
