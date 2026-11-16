# aster_forge_process

`aster_forge_process` owns product-neutral child-process lifecycle mechanics: direct argument execution, optional stdin, bounded stdout/stderr capture, cancellation, deadlines, and owned process-tree cleanup. It does not decide product retries, protocol errors, logging policy, or whether a side effect is safe to repeat.

Unix children run in an owned process group and Windows children in a Job Object. Captured bytes are bounded and drained; the callback receives retained bytes only. `ProcessSpec` debug output redacts stdin and environment values.

```rust
let spec = aster_forge_process::ProcessSpec::new(program, timeout);
let outcome = aster_forge_process::run(spec, &cancellation, |_stream, _bytes| {}).await;
```

Product adapters map `ProcessFailure` into their own error contracts and decide how captured output is presented.

When cancellation or a deadline occurs, `run` waits through the owned process-group or Job Object wrapper before returning. Owned descendants have completed cleanup at return; a daemon that deliberately detaches is outside this guarantee.

```bash
cargo test -p aster_forge_process --locked
```
