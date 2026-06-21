# Security Policy

ProximaDB is a high-performance vector + graph database. Because it stores and
serves tenant data, we treat security and tenant isolation as first-class
correctness properties, not add-ons.

## Reporting a Vulnerability

**Do not open a public issue for security vulnerabilities.**

Please report privately via GitHub's [private vulnerability
reporting](https://github.com/vjsingh1984/proximaDB/security/advisories/new)
(Security tab → "Report a vulnerability"). This routes the report to the
maintainers confidentially.

When reporting, please include:

- A description of the vulnerability and its impact.
- Steps to reproduce (proof-of-concept where possible).
- Affected version / commit, and configuration (transport, auth mode, storage
  backend) if relevant.
- Any suggested remediation.

### Response targets

| Stage | Target |
| --- | --- |
| Acknowledgement | within 3 business days |
| Triage + severity assessment | within 7 business days |
| Fix or mitigation plan | depends on severity; criticals prioritized |

We will keep you informed through the advisory thread and credit you in the
release notes unless you ask us not to.

## Scope

In scope:

- The server (`proximadb-server`) and all crates in this workspace.
- Tenant isolation boundaries (path isolation under `DrPathBuilder`,
  `TenantContext` enforcement at I/O boundaries).
- Authentication / authorization, transport security (REST, gRPC, pgwire,
  Arrow Flight), and the embedded/SDK clients.
- Data-at-rest handling (object store paths, WAL, segment formats).

Out of scope:

- Issues that require a privileged local account on the host running the server.
- Denial of service from unbounded but documented configuration (e.g. operator
  setting limits too high).
- Vulnerabilities in third-party dependencies already tracked by an upstream
  advisory — please still tell us so we can bump.

## Supported Versions

ProximaDB is pre-1.0. Security fixes land on `main` and the active `develop`
line. Older tags are not separately patched; please track `main`.

## Our Defensive Posture

The codebase enforces several security invariants automatically in CI:

- **No panics in production paths** — `unwrap()` / `expect()` / `panic!()` are
  gated by the panic-policy checks; errors propagate via `Result`.
- **Structural tenant isolation** — isolation is enforced at the boundary, never
  as a per-query predicate; a tenant-path guard runs in CI.
- **Secret scanning + push protection** are enabled on this repository.
- **Dependency and code scanning** run against pushes and pull requests.
- **Tiered merge gates** — feature branches run lint/build/security checks;
  promotion to `qa` and `main` adds progressively heavier test and audit gates.

Thank you for helping keep ProximaDB and its users safe.
