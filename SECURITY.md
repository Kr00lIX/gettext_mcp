# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| `main` branch | Yes |
| Latest published crate (`gettext-mcp` on crates.io) | Yes |
| Older releases | No |

Security fixes land on `main` and are released to crates.io as part of the next version.

## Reporting a Vulnerability

Please report security vulnerabilities privately. Either:

- Open a [GitHub Security Advisory](https://github.com/Kr00lIX/gettext_mcp/security/advisories/new), or
- Email <me@kr00lix.com>.

**Do not open a public issue for security vulnerabilities.**

Please include:

- A description of the issue and its impact.
- Steps to reproduce, ideally with a minimal `.po` file or MCP call.
- Affected version (`main` commit hash or crate version).

### Disclosure timeline

- Initial acknowledgement on a best-effort basis, typically within a few days.
- Coordinated disclosure window of up to 90 days from the initial report. If a fix is ready sooner, the advisory is published sooner.

## Scope

In scope:

- **Parser vulnerabilities** — malformed `.po`/`.pot` input causing panics, denial of service, excessive memory use, or memory unsafety.
- **Path traversal** — file paths escaping the configured base directory during read or write operations.
- **MCP protocol handling** — malicious tool arguments causing unintended file mutation, information disclosure, or crashes.
- **Web UI** — unauthenticated access from a non-localhost origin, XSS in the embedded SPA, or CSRF against the REST API.

Out of scope: anything that requires write access to the machine running the server, or to the `.po` files themselves, is already assumed in the threat model.
