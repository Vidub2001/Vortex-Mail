# Security policy

**English** · [Français](SECURITY.fr.md) · [Español](SECURITY.es.md)

Vortex Mail reads messages written by strangers every day, so security problems are taken seriously. Thank you for reporting them privately.

## Supported versions

Only the latest published release receives security fixes. Please check that the problem still exists in it before reporting.

## Reporting a vulnerability

**Never in a public issue.** Use one of these private channels:

1. **Preferred:** GitHub's private reporting, "[Report a vulnerability](https://github.com/Vidub2001/Vortex-Mail/security/advisories/new)" in the *Security* tab of this repository.
2. Otherwise: the [contact form](https://vortexmail.fr/contact.html) of the website, asking for a private channel.

Please include:

- the Vortex Mail version and your system (distribution or Windows version);
- the steps to reproduce, and what happens compared with what should happen;
- if the problem comes from a message, the message itself as an `.eml` file (it will be added to the regression corpus once fixed);
- what an attacker could obtain, as far as you know.

## Scope

What Vortex Mail protects, and against whom, is described in the [threat model](docs/MODELE-MENACE.en.md). A behaviour that contradicts it is a vulnerability.

The limits the threat model already states (section "Out of scope and known limits") are not vulnerabilities, for example: a malicious program running under the user's account, a sender writing from a look-alike domain, or the IP address learnt by an image host once the user has chosen to show the images.

## What to expect

Vortex Mail is maintained by one person. The commitments are therefore modest, but kept:

- acknowledgement of your report within **7 days**;
- a fix, or at least a status update, within **30 days**;
- coordinated disclosure: the vulnerability is made public once the fixed version is published, and at the latest **90 days** after your report, unless we agree otherwise.

If you wish, you will be credited in the release notes of the fix. There is no bug bounty.
