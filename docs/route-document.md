# Atomic Route Document Contract (edge ↔ daemon)

Defined now so Phase 1 foundations do not contradict it; implemented in
Phase 5. Informed by the proven legacy edge publication mechanism, trimmed.

## Principles

- The daemon owns desired route state and publishes it as **one complete
  JSON snapshot** — never a diff or a partial update.
- Publication is atomic: write temp file, fsync, rename into place (or the
  socket-push equivalent ending in an atomic replace on the edge side).
- The edge persists the last valid document plus a `last-known-good` copy
  and keeps serving from it across daemon restarts. A malformed, oversized,
  or partially-written document never clears currently served routes.
- Size cap 2 MiB. Integrity: `payload_sha256` over the canonical payload
  bytes; mismatch → reject, keep serving previous.
- Grants bind to immutable `deployment_id`, never to a mutable domain or
  port.

## Shape (route schema 1)

```json
{
  "schema": 1,
  "payload_sha256": "<hex>",
  "generation": 42,
  "published_at": "2026-08-22T12:00:00Z",
  "domain": "<base domain from instance configuration>",
  "routes": [
    {
      "deployment_id": "d…",
      "domain": "<fqdn>",
      "port": 12345,
      "scheme": "http",
      "auth": "public" | "authenticated"
    }
  ],
  "access": {
    "owners": ["<identity>"],
    "grants": [{"identity": "<identity>", "deployment_id": "d…", "role": "access|viewer|operator|administrator"}]
  }
}
```

- `generation` increases monotonically; the edge ignores documents with a
  generation lower than the one it serves.
- Domains and identities are instance data: they appear only in the
  published document on the host, never in repository content.
- The edge enforces `auth` and grants per route; the daemon never handles
  public sessions.
