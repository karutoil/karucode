# karucode performance report

Generated: 2026-10-27 15:42:09
Host: Linux 6.12.113+deb13-amd64, AMD Ryzen 7 3700X 8-Core Processor, 31.3 GiB RAM, 16 CPUs
Toolchain: rustc 1.96.0 (ac68faa20 2026-05-25)
Binary: target/release/karucode (strip + LTO thin)

## Method

`scripts/bench.py` starts the release server with a temp SQLite DB, points it at
`scripts/mock_openai.py` (a local streaming mock, no network), polls `/api/health`
for cold-start time, reads `VmRSS` from `/proc/<pid>/status` after 3s idle, then
starts 3 concurrent agent runs and samples RSS every 50ms until they finish.

## Results (per run)

| run | startup ms | idle RSS MiB | peak RSS (3 runs) MiB | 3 runs wall ms | binary MiB |
| --- | --- | --- | --- | --- | --- |
| 1 | 11.2 | 9.9 | 12.1 | 160.6 | 10.35 |
| 2 | 6.1 | 10.0 | 12.1 | 158.7 | 10.35 |
| 3 | 5.8 | 9.9 | 12.0 | 158.9 | 10.35 |
| **avg** | **7.7** | **9.9** | **12.1** | **159.4** | **10.3** |

These numbers are specific to the host above and to the mock provider:
RSS during a real run also depends on the provider TLS stack and response sizes.
