# wassette-gkeep

Google Keep MCP component for [Wassette](https://github.com/microsoft/wassette). Provides 5 tools: list notes, get note, create text note, create checklist note, delete note.

## Quick Start

Install Wassette ([instructions](https://microsoft.github.io/wassette/latest/installation.html)), then run:

```sh
GOOGLE_KEEP_TOKEN=ya29.xxx wassette serve --stdio \
  --load oci://ghcr.io/schonhoffer/wassette-gkeep:latest \
  --env GOOGLE_KEEP_TOKEN \
  --net-allow keep.googleapis.com
```

Or with the policy file instead of inline flags:

```sh
GOOGLE_KEEP_TOKEN=ya29.xxx wassette serve --stdio \
  --load oci://ghcr.io/schonhoffer/wassette-gkeep:latest \
  --policy policy.yaml
```

## Docker

Run Wassette + this component in a container:

```sh
docker run --rm -i \
  -e GOOGLE_KEEP_TOKEN=ya29.xxx \
  ghcr.io/microsoft/wassette:latest \
  serve --stdio \
  --load oci://ghcr.io/schonhoffer/wassette-gkeep:latest \
  --env GOOGLE_KEEP_TOKEN \
  --net-allow keep.googleapis.com
```

## MCP Client Config

Configure your AI agent to use this as an MCP server:

```json
{
  "mcpServers": {
    "gkeep": {
      "command": "wassette",
      "args": [
        "serve", "--stdio",
        "--load", "oci://ghcr.io/schonhoffer/wassette-gkeep:latest",
        "--env", "GOOGLE_KEEP_TOKEN",
        "--net-allow", "keep.googleapis.com"
      ],
      "env": {
        "GOOGLE_KEEP_TOKEN": "ya29.xxx"
      }
    }
  }
}
```

## Tools

| Tool | Description |
|------|-------------|
| `list-notes` | List notes with optional filter, page size, and pagination token |
| `get-note` | Get a note by ID |
| `create-text-note` | Create a note with a title and text body |
| `create-list-note` | Create a checklist note (items as JSON array) |
| `delete-note` | Delete a note by ID |

## Building from Source

```sh
cargo build --release --target wasm32-wasip2
wassette serve --stdio --load target/wasm32-wasip2/release/wassette_gkeep.wasm \
  --env GOOGLE_KEEP_TOKEN --net-allow keep.googleapis.com
```

## Kubernetes

Host a single Wassette instance serving multiple components from a mounted folder.

### Manifests

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: wassette-policy
data:
  policy.yaml: |
    version: "1.0"
    description: "Shared policy for all components"
    permissions:
      network:
        allow:
          - host: "keep.googleapis.com"
      environment:
        allow:
          - key: "GOOGLE_KEEP_TOKEN"
---
apiVersion: v1
kind: Secret
metadata:
  name: wassette-secrets
stringData:
  GOOGLE_KEEP_TOKEN: "ya29.xxx"
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: wassette
spec:
  replicas: 1
  selector:
    matchLabels:
      app: wassette
  template:
    metadata:
      labels:
        app: wassette
    spec:
      containers:
        - name: wassette
          image: ghcr.io/microsoft/wassette:latest
          args:
            - serve
            - --streamable-http
            - --policy
            - /config/policy.yaml
            - --load
            - /components/
          ports:
            - containerPort: 9001
          envFrom:
            - secretRef:
                name: wassette-secrets
          readinessProbe:
            httpGet:
              path: /ready
              port: 9001
          livenessProbe:
            httpGet:
              path: /health
              port: 9001
          volumeMounts:
            - name: policy
              mountPath: /config
            - name: components
              mountPath: /components
      volumes:
        - name: policy
          configMap:
            name: wassette-policy
        - name: components
          emptyDir: {}
      initContainers:
        - name: fetch-components
          image: ghcr.io/oras-project/oras:v1.2.2
          command: ["/bin/sh", "-c"]
          args:
            - |
              oras pull -o /components oci://ghcr.io/schonhoffer/wassette-gkeep:latest
          volumeMounts:
            - name: components
              mountPath: /components
---
apiVersion: v1
kind: Service
metadata:
  name: wassette
spec:
  selector:
    app: wassette
  ports:
    - port: 9001
      targetPort: 9001
```

The init container pulls `.wasm` files from OCI before Wassette starts. To add more components, add more `oras pull` lines in the init container.

### OpenClaw Config

Point OpenClaw at the Wassette service using the streamable HTTP endpoint:

```json
{
  "mcpServers": {
    "wassette": {
      "url": "http://wassette.default.svc.cluster.local:9001/mcp"
    }
  }
}
```

If OpenClaw is outside the cluster, expose the service via an Ingress or LoadBalancer and use the external URL instead.

## Permissions

The component requires:
- Network access to `keep.googleapis.com`
- Environment variable `GOOGLE_KEEP_TOKEN` (OAuth2 bearer token)

See [policy.yaml](policy.yaml) for the Wassette permission grants.
