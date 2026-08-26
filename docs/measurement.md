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
