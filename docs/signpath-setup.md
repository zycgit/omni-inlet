# SignPath Foundation setup

This document records the one-time external configuration needed by the release
workflow. It contains no signing key: SignPath generates and retains the private
key in its hardware security module.

## Foundation application

1. Enable multi-factor authentication for the `zycgit` GitHub account.
2. Apply at <https://signpath.org/> for the public repository
   `https://github.com/zycgit/omni-inlet`.
3. Reference the repository's [code signing policy](code-signing-policy.md),
   [privacy policy](privacy.md), and Apache-2.0 license in the application.
4. Install the SignPath GitHub App for this repository when requested.

The SignPath Foundation review and acceptance of its terms must be completed by
the repository owner. It cannot be delegated to CI or committed as source code.

## SignPath project

After approval, create or confirm these objects in SignPath:

- Project repository: `https://github.com/zycgit/omni-inlet`
- Artifact configuration: `windows-portable`, using
  `.github/signpath/artifact-configuration.xml`
- Signing policy: `release-signing`, restricted to tag builds from this
  repository and requiring an approver
- Trusted build system: GitHub Actions for `zycgit/omni-inlet`

Create a CI user/API token that can only submit signing requests. Do not give the
CI token approval or project-administration permissions.

## GitHub configuration

Add these Actions repository variables:

| Variable | Value |
| --- | --- |
| `SIGNPATH_ORGANIZATION_ID` | Organization UUID shown by SignPath |
| `SIGNPATH_PROJECT_SLUG` | `omni-inlet` unless changed in SignPath |
| `SIGNPATH_SIGNING_POLICY_SLUG` | `release-signing` |
| `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` | `windows-portable` |

Add one Actions repository secret:

| Secret | Value |
| --- | --- |
| `SIGNPATH_API_TOKEN` | Token belonging to the restricted SignPath CI user |

The release workflow checks that all values exist. A tag or manual release build
fails closed when signing is unavailable; it never publishes an unsigned Windows
archive as an official release.

## Release approval

Start the existing release workflow with a version tag or a non-empty
`release_tag`. Open the signing-request URL reported by its Windows job and
approve the request in SignPath. The job then downloads the signed application,
validates every required Authenticode signature, creates the ZIP, and uploads it
with the Linux and macOS archives.
