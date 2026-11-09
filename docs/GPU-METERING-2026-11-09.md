# Contributed GPU utilization metering (2026-11-09)

This upgrade closes the GPU metering gap that
[PRODUCT-UI-UPGRADE-2026-11-06.md](PRODUCT-UI-UPGRADE-2026-11-06.md),
[PASSKEY-RESOURCE-UPGRADE-2026-11-07.md](PASSKEY-RESOURCE-UPGRADE-2026-11-07.md) and
[ECONOMY-LOCALIZATION-2026-11-08.md](ECONOMY-LOCALIZATION-2026-11-08.md)
left open honestly: `resource_meter.rs` now reads real GPU busy
percentages where the host exposes them and integrates them into the same
per-period `MeasuredUsage` that the resource economy already prices,
clamps and settles. No economy rule, grant limit, rate or settlement
behaviour changed.

## What was added

### Real GPU busy-percentage sources (Linux)

`read_gpu_busy_percent()` sums the busy percentages of all GPUs the host
reports, trying two read-only sources in order:

1. **amdgpu sysfs** — `device/gpu_busy_percent` under each card device in
   `/sys/class/drm` (connector entries like `card0-HDMI-A-1` are skipped).
2. **nvidia-smi** — `nvidia-smi --query-gpu=utilization.gpu
   --format=csv,noheader,nounits`, executed with fixed arguments and no
   shell, only if the tool exists and exits successfully.

A host that reports no GPU counter yields `None`; the meter then reports
zero GPU ticks instead of inventing a number. Unreadable lines (for
example nvidia-smi `[N/A]`) invalidate the whole reading rather than being
silently dropped.

### Integration into `gpu_unit_ticks`

`SystemCounters` gains a cumulative `gpu_unit_ticks` counter. On each
Linux host read, `advance_gpu_ticks` credits the **previously sampled**
busy percentage over the uptime elapsed since the previous read:

- One tick is one GPU-second at 1% duty, so a fully busy GPU accumulates
  100 ticks per second — the same scale as one fully busy CPU core at
  HZ=100, which keeps the founder's per-tick rate table comparable.
- The first read only establishes the baseline and credits nothing.
- An interval whose previous sample was missing credits nothing.
- Uptime going backwards never credits anything, and the accumulator
  saturates instead of overflowing.
- `UsagePeriodMeter::sample_period` now reports the GPU tick delta like
  the CPU tick delta; `MeasuredUsage.gpu_unit_ticks` stops being hardcoded
  to zero. A wrapped or lying counter is clamped to zero by the same
  saturating subtraction the CPU path uses.

The settlement path is unchanged and still conservative: the consented
grant limits cap what is paid (`gpu_unit_ticks > limits.gpu_unit_ticks`
records a `Gpu` violation and clamps), the effective usage is the minimum
across independent signals, and signed verifier reports still require at
least two distinct verifiers.

## Honest status

- **Sampling, not accounting.** Busy percentage is sampled at meter reads
  and held over the elapsed interval. Short bursts between reads are not
  individually tracked; the verifier consensus and grant caps remain the
  anti-inflation backstops.
- **No per-process attribution.** The meter reads whole-GPU utilization
  only; it cannot prove which workload used the GPU.
- **Windows and macOS.** Host metering remains Linux-only; other
  platforms keep the honest `Unsupported` error, and `gpu_unit_ticks`
  stays zero there.
- **nvidia-smi is a subprocess.** The call is read-only with fixed
  arguments, but it has no timeout mechanism; a wedged `nvidia-smi`
  would block its read. If that ever bites, the fix is a bounded
  helper process, not a fake reading.
- **Multi-node usage verifiers and multi-node marketplace sync** remain
  open exactly as in the 2026-11-06 document.

## Tests

- `gpu_ticks_integrate_sampled_busy_percent` (Linux): baseline behaviour,
  percent-second integration, missing samples, backwards uptime and
  saturation — all against the pure `advance_gpu_ticks` step, so no GPU is
  needed.
- `gpu_parsers_read_real_formats` (Linux): amdgpu `gpu_busy_percent` and
  nvidia-smi noheader/nounits output, including the `[N/A]`, over-100 and
  empty-input rejections.
- `period_meter_converts_gpu_ticks_into_usage` and
  `gpu_ticks_never_go_backwards_between_reads`: the `UsagePeriodMeter`
  delta path, both platforms.
- `host_meter_reads_real_counters` (Linux) now also asserts the GPU tick
  accumulator never decreases between reads; on the CI hosts (no readable
  GPU counter) it stays zero, which is the honest value.

CI runs the same gates as `main`: `cargo fmt --check`, workspace build,
unit and integration tests, the product smoke test and `cargo clippy
--workspace --all-targets -- -D warnings`, on both Linux and Windows.
