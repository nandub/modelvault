# Storage measurement model

ModelVault 1.1.0 reports storage effects in separate layers.

- **Logical bytes**: sum of artifact sizes represented by manifests.
- **Unique logical bytes**: unique manifest-referenced CAS object bytes before physical encoding.
- **Dedup savings**: logical bytes minus unique logical bytes.
- **Full encoded bytes**: estimated pack-v2 raw/Zstd size for each unique logical object.
- **Compression savings**: unique logical bytes minus full encoded bytes.
- **Primary encoded bytes**: best retained full/delta representation, including required delta-only base dependencies.
- **Delta savings**: full encoded bytes minus primary encoded bytes, when positive.
- **Duplicate representation bytes**: redundant reachable physical copies such as loose + pack representations.
- **Metadata overhead**: pack indexes, manifests, and repository/config metadata.
- **Net physical savings**: logical bytes minus actual repository physical bytes. This value can be negative when the repository is physically larger than its logical artifacts.

Per-artifact attribution counts shared logical objects across manifests and divides their estimated primary physical representation evenly among referencing artifacts. Attribution is approximate and intended for comparison/diagnostics, not billing.

`modelvault benchmark-repo --json` emits a versioned snapshot suitable for checking into a benchmark/results repository or comparing across ModelVault releases and storage policies.

## Reproducible chunking comparisons

Use `modelvault benchmark` with two versions of the same Safetensors artifact to
compare generic chunking with ModelVault's tensor-bounded strategies:

```powershell
cargo run --locked -- benchmark `
    .\artifacts\base\model.safetensors `
    .\artifacts\tuned\model.safetensors `
    --avg-chunk-size 4194304 `
    --json
```

The report compares `fixed`, `tensor-fixed`, `fastcdc`, and
`tensor-fastcdc`. For each strategy it reports chunk counts, bytes reused from
the left artifact, reuse percentage, and elapsed time. The default mode treats
the inputs as Safetensors and ensures tensor-aware strategies never make a
chunk cross a tensor boundary. Pass `--raw` only when the inputs are not
Safetensors or when deliberately measuring generic byte-stream chunking.

For a comparison that others can reproduce, record:

- the exact two input artifacts and their BLAKE3 hashes (or ModelVault artifact
  IDs when they are already tracked);
- the ModelVault version and the full command line, including
  `--avg-chunk-size`;
- the complete command output; and
- the host and operating-system details when reporting elapsed time.

Reuse percentage is the primary comparison metric. Elapsed time is a diagnostic
measurement and should not be treated as a cross-machine performance claim.
The command estimates shared logical chunks; it does not by itself measure
pack, compression, delta, or remote-transfer savings.

`--json` emits a versioned report intended for archival and comparison. The
report includes the supplied paths, chunk-size target, Safetensors mode, and
one result per strategy. Paths can be machine-specific, so sanitize them before
publishing a benchmark report.

To capture repository-wide physical-storage results, write a versioned
snapshot and compare it with another snapshot:

```powershell
cargo run --locked -- benchmark-repo --output .\benchmarks\before.json
# Apply one intentional storage-policy or repository change.
cargo run --locked -- benchmark-repo --output .\benchmarks\after.json
cargo run --locked -- benchmark-compare .\benchmarks\before.json .\benchmarks\after.json
```

Keep benchmark snapshots outside ordinary artifact history when they contain
machine-specific paths or sensitive repository measurements.
