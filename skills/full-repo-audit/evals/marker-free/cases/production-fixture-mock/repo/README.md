# Product catalog

## Requirements

- `CatalogService.list_products()` is the production catalog read. It must
  return `repository.list_active()` so current product changes are visible and
  repository failures reach the caller.
- `onboarding_preview()` deliberately returns a small, deterministic set of
  example products for the disconnected first-run tour. Those examples must
  never supply the production catalog read.
