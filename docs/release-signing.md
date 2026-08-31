# Release Artifact Signing

ModelVault GitHub Releases use Sigstore keyless signing for each platform ZIP
archive and for `SHA256SUMS`.

## One-command verification

Install [GitHub CLI](https://cli.github.com/) and
[Cosign](https://docs.sigstore.dev/cosign/system_config/installation/), then
run this from a ModelVault source checkout on Windows:

```powershell
.\scripts\Verify-PublishedRelease.ps1 -ReleaseTag v1.7.6
```

The helper downloads the Windows x86_64 archive plus its checksum and Sigstore
bundles to a temporary directory, verifies the archive SHA-256 value, verifies
the keyless signatures for both the archive and `SHA256SUMS`, and removes the
temporary files. Pass `-Target` for a different release target or
`-KeepDownloads` to retain the downloaded assets.

The release workflow receives a GitHub Actions OpenID Connect token and creates
one `<asset>.sigstore.json` bundle per signed asset. A bundle contains the
signature, short-lived certificate, and Rekor transparency-log inclusion proof.
No long-lived signing key or private-key secret is stored in the repository.

## Verify a downloaded release asset

Download an archive and its corresponding `.sigstore.json` bundle from the
same GitHub Release, then use Cosign:

```powershell
cosign verify-blob `
  --bundle .\modelvault-v1.7.0-x86_64-pc-windows-msvc.zip.sigstore.json `
  --certificate-identity-regexp '^https://github[.]com/nandub/modelvault/[.]github/workflows/release[.]yml@refs/' `
  --certificate-oidc-issuer https://token.actions.githubusercontent.com `
  .\modelvault-v1.7.0-x86_64-pc-windows-msvc.zip
```

Then verify the archive's SHA-256 value against the signed `SHA256SUMS` file.
The same command works for `SHA256SUMS` by replacing both asset paths with the
checksum file and its bundle.

## Trust boundary

The signature proves that the asset was signed by a GitHub Actions identity for
the ModelVault release workflow. It does not replace artifact-level ModelVault
integrity checks: downloaded model data remains untrusted until ModelVault
verifies its BLAKE3 object and artifact identities.
