# Code signing policy

Free code signing is provided by [SignPath.io](https://signpath.io/), with a
certificate provided by [SignPath Foundation](https://signpath.org/).

## Scope

Official Windows releases are built from this public repository by GitHub
Actions. The project's own Windows executables and runtime library are submitted
directly from that trusted build to SignPath for Authenticode signing. A release
is rejected if any required project binary has no valid signature.

Bundled third-party libraries are distributed under their respective open-source
licenses and are not signed using the OmniInlet project certificate.

## Team roles

- Committer and reviewer: [zycgit](https://github.com/zycgit)
- Signing approver: [zycgit](https://github.com/zycgit)

Additional maintainers must use multi-factor authentication for GitHub and
SignPath before receiving any of these roles. Changes to build or signing
configuration receive the same review as product source changes.

## Privacy

See the [OmniInlet privacy policy](privacy.md). The application does not transfer
information to other networked systems unless specifically requested by the
person installing or operating it.

## Verification

On Windows, an official binary can be checked with PowerShell:

```powershell
Get-AuthenticodeSignature .\omni-inlet.exe |
  Format-List Status,StatusMessage,SignerCertificate
```

`Status` must be `Valid`. Release automation performs this check for every
project-owned executable and DLL before uploading the portable ZIP.
