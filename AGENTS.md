Google Keep MCP as a Wassette (WASM) component. No native binary, no MCP protocol code — Wassette handles all of that.

## How it works

The crate compiles to a `cdylib` targeting `wasm32-wasip2`. The WIT file (`wit/world.wit`) defines exported functions. Each function becomes an MCP tool when loaded by Wassette. HTTP goes through `spin-sdk` (WASI HTTP), not `reqwest`.

## Building

    cargo build --release --target wasm32-wasip2

Output: `target/wasm32-wasip2/release/wassette_gkeep.wasm`

## Running

Load into a running Wassette instance:

    wassette serve --stdio --load target/wasm32-wasip2/release/wassette_gkeep.wasm

Or from the OCI registry:

    wassette serve --stdio --load oci://ghcr.io/schonhoffer/wassette-gkeep:latest

Requires `GOOGLE_KEEP_TOKEN` env var (OAuth2 bearer token) and network access to `keep.googleapis.com`. See `policy.yaml` for the permission grants.

## Constraints

- No `reqwest`, `tokio`, `hyper`, `openssl`, or `native-tls` — use `spin-sdk` for HTTP
- No `main.rs` — this is a library (`cdylib`), not a binary
- Call Google APIs via REST directly, not generated client crates
- Keep code readable; skip comments where the code speaks for itself