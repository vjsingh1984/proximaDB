# Publishing the ProximaDB OSS image to Docker Hub

`.github/workflows/publish-image.yml` builds the OSS server image
(`deploy/docker/Dockerfile`, target `runtime`) and pushes a **multi-arch
manifest (`linux/amd64` + `linux/arm64`)** to **Docker Hub** as
`vjsingh1984/proximadb`.

This is the **public OSS image**. AnvaiOps' *commercial* pipeline
(`anvaiops/.github/workflows/build-commercial-image.yml`) consumes it as a
`FROM` base, bakes pricing tiers in, and pushes the result to **ACR** — the OSS
image itself is on Docker Hub, not ACR.

## Architectures

Built for both **amd64** and **arm64** so the same tag runs on x86 clouds,
Graviton/Ampere, Apple-Silicon dev, and anvaiops' arm64 node pool (ADR-0016 /
`proximadb_node_arch = "arm64"`). Each arch builds **natively** on its own
hosted runner (free for public repos: `ubuntu-latest` = amd64,
`ubuntu-24.04-arm` = arm64), is pushed **by digest**, and a final `merge` job
combines both digests into one multi-arch manifest list that carries the tags.
QEMU emulation is intentionally avoided — the heavy Rust release build would be
far too slow under it.

## What gets published

| Trigger | Tags |
|---|---|
| Push tag `v<X.Y.Z>` | `<X.Y.Z>`, `sha-<short>`, `latest` |
| Push to `develop` | `develop`, `sha-<short>` (no `latest`) |
| `workflow_dispatch` | `<version-from-Cargo.toml>`, `sha-<short>`, `latest` (if `tag_latest=true`) |

The `runtime-full` target (ONNX + baked bge-small) publishes a parallel
`-full`-suffixed tag namespace when selected via dispatch.

## One-time setup

Repository **secrets**:

| Secret | Purpose |
|---|---|
| `DOCKERHUB_USERNAME` | Docker Hub account that owns `vjsingh1984/proximadb` |
| `DOCKERHUB_TOKEN` | Docker Hub access token (Account Settings → Security → New Access Token, Read/Write/Delete) |

Repository **variables** (all optional):

| Variable | Default | Purpose |
|---|---|---|
| `DOCKERHUB_IMAGE` | `vjsingh1984/proximadb` | Override the published repo |
| `DOCKERHUB_PRUNE_KEEP` | `10` | Number of newest `sha-*` tags to retain |

The `DOCKERHUB_TOKEN` needs **delete** scope for the prune job to remove stale
`sha-*` tags.

## Downstream consumption (AnvaiOps commercial overlay)

Once published, in **anvaiops** set the repo variable
`PROXIMADB_OSS_BASE=vjsingh1984/proximadb` and flip
`OSS_BASE_IMAGE_AVAILABLE=true`, then pin the base **by digest** (ADR-0016) —
the digest is printed in this workflow's run summary — via the
`proximadb_version` dispatch input / build arg in `build-commercial-image.yml`.
AKS pulls the public image directly (no imagePullSecret needed).

## Cost control

The `prune` job runs after every successful push and deletes superseded rolling
`sha-*` tags via the Docker Hub API, keeping the newest `DOCKERHUB_PRUNE_KEEP`.
It **never** touches semver tags, `latest`, or `develop`.

## Local build (parity check)

```bash
# minimal OSS server (default target)
docker build -f deploy/docker/Dockerfile --target runtime -t proximadb:local .

# full image with ONNX + baked bge-small-en-v1.5
docker build -f deploy/docker/Dockerfile --target runtime-full -t proximadb:local-full .
```
