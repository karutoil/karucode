# karucode performance report

Generated: 2026-10-27 17:32:16
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
| 1 | 10.1 | 10.3 | 12.4 | 158.2 | 10.48 |
| 2 | 5.7 | 10.0 | 12.4 | 159.7 | 10.48 |
| **avg** | **7.9** | **10.2** | **12.4** | **158.9** | **10.5** |

These numbers are specific to the host above and to the mock provider:
RSS during a real run also depends on the provider TLS stack and response sizes.
