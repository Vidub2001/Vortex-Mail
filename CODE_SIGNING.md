# Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by [SignPath Foundation](https://signpath.org/).

## What is signed

Only the Windows files of a published Vortex Mail release are signed:

- the installer (`Vortex Mail_<version>_x64-setup.exe`);
- the executable inside the portable version (`VortexMail-portable-<version>.zip`).

They are built from this repository by GitHub Actions ([`.github/workflows/release.yml`](.github/workflows/release.yml)) when a version tag (`v*`) is pushed. Files built anywhere else, including on a developer's computer, are never submitted for signing. Every signing request is approved by hand before it is signed.

Every release also lists the SHA-256 checksums of all its files (`SHA256SUMS`), Linux packages included.

## Team roles

| Role | Members |
| --- | --- |
| Committers and reviewers | [Fabrice Joffre](https://github.com/Vidub2001) |
| Approvers | [Fabrice Joffre](https://github.com/Vidub2001) |

Changes proposed by anyone outside this list are reviewed by a reviewer before they are merged. All team members use multi-factor authentication for their GitHub and SignPath accounts.

## Privacy policy

This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it.

Vortex Mail connects only to the servers of the accounts the user adds (mail, calendar and contacts servers, and the sign-in pages of Google and Outlook.com when the user chooses to sign in with them). It contains no telemetry, no analytics and no automatic update check.
