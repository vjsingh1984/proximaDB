# Publishing the ProximaDB OSS image to Azure Container Registry

The `.github/workflows/publish-image.yml` workflow builds the OSS server image
(`deploy/docker/Dockerfile`, target `runtime`) and pushes it to an Azure
Container Registry (ACR) using **OIDC-federated** auth — no long-lived registry
credentials are stored in GitHub.

This is the **OSS base image** (`<registry>/proximadb`). AnvaiOps' commercial
pipeline consumes it as a `FROM` base and AKS pulls it directly; see
`anvaiops/docs/RUNBOOK_COMMERCIAL_IMAGE.md`.

## What gets published

| Trigger | Tags |
|---|---|
| Push tag `v<X.Y.Z>` | `<X.Y.Z>`, `sha-<short>`, `latest` |
| Push to `develop` | `develop`, `sha-<short>` (no `latest`) |
| `workflow_dispatch` | `<version-from-Cargo.toml>`, `sha-<short>`, `latest` (if `tag_latest=true`) |

The `runtime-full` target (ONNX + baked bge-small) publishes a parallel
`-full`-suffixed tag namespace when selected via dispatch.

## One-time setup

### 1. GitHub configuration (matches anvaiops convention)

Repository **variables**:

| Variable | Example | Purpose |
|---|---|---|
| `ACR_REGISTRY` | `anvaiopsmvp1234.azurecr.io` | Login-server FQDN (required; workflow no-ops if empty) |
| `ACR_IMAGE_NAME` | `proximadb` | Repository name within the registry (optional; defaults to `proximadb`) |
| `ACR_PURGE_KEEP` | `10` | Tagged manifests to retain during prune (optional; default `10`) |
| `ACR_PURGE_AGO` | `30d` | Age threshold for purge (optional; default `30d`) |

Repository **secrets** (federated identity coordinates — not passwords):

| Secret | Purpose |
|---|---|
| `AZURE_CLIENT_ID` | App registration / user-assigned identity client ID |
| `AZURE_TENANT_ID` | Azure AD tenant ID |
| `AZURE_SUBSCRIPTION_ID` | Subscription containing the ACR |

### 2. Azure identity + federated credential

Create (or reuse the anvaiops) app registration and grant it **AcrPush** on the
registry, then add a federated credential trusting this repo's workflow:

```bash
ACR_NAME=anvaiopsmvp1234
SUBSCRIPTION_ID=<sub>
APP_ID=<client-id of the push identity>

# AcrPush on the registry
ACR_ID=$(az acr show --name "$ACR_NAME" --query id -o tsv)
az role assignment create \
  --assignee "$APP_ID" \
  --role AcrPush \
  --scope "$ACR_ID"

# Federated credential: trust GitHub OIDC for this repo's branches + tags
az ad app federated-credential create --id "$APP_ID" --parameters '{
  "name": "proximadb-publish-develop",
  "issuer": "https://token.actions.githubusercontent.com",
  "subject": "repo:vjsingh1984/proximadb:ref:refs/heads/develop",
  "audiences": ["api://AzureADTokenExchange"]
}'
az ad app federated-credential create --id "$APP_ID" --parameters '{
  "name": "proximadb-publish-tags",
  "issuer": "https://token.actions.githubusercontent.com",
  "subject": "repo:vjsingh1984/proximadb:ref:refs/tags/v*",
  "audiences": ["api://AzureADTokenExchange"]
}'
```

> `AcrPush` (push) is distinct from the `AcrPull` role that anvaiops' Terraform
> (`acr_attach.tf`) grants the AKS kubelet identity for pulls. The publish
> identity here needs push; AKS only needs pull.

### 3. Enable downstream consumption

Once the OSS base exists in ACR, in **anvaiops**:

- Set repo variable `OSS_BASE_IMAGE_AVAILABLE=true` and
  `PROXIMADB_OSS_BASE=<registry>/proximadb` to enable the commercial build.
- Pin the base **by digest** (ADR-0016): set `PROXIMADB_BASE`/`PROXIMADB_VERSION`
  in `deploy/docker/Dockerfile.commercial`, or `proximadb_image` in Terraform,
  to `<registry>/proximadb@sha256:...` (the digest is printed in this workflow's
  run summary).

## Cost control

The `prune` job runs after every successful push and executes `acr purge`
server-side: it keeps the newest `ACR_PURGE_KEEP` tagged manifests, deletes
tagged manifests older than `ACR_PURGE_AGO`, and removes dangling untagged
manifests left by re-pushed mutable tags (`latest`, `develop`). No images are
pulled to the runner.

## Local build (parity check)

```bash
# minimal OSS server (default target)
docker build -f deploy/docker/Dockerfile --target runtime -t proximadb:local .

# full image with ONNX + baked bge-small-en-v1.5
docker build -f deploy/docker/Dockerfile --target runtime-full -t proximadb:local-full .
```
