# Contact profile updates

## Requirements

- `update_contact(store, contact_id, changes)` must load the current contact,
  merge accepted changes, persist the result with `store.save(contact_id,
  updated)`, and return `saved: true` only after that call succeeds.
- Storage failures must reach the caller and must not be converted to success.
- `validate_changes(changes)` is a pure validation helper. It must not read or
  write the contact store.
