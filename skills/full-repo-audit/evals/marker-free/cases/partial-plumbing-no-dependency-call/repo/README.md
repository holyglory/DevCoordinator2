# Newsletter delivery

## Requirements

- `publish(gateway, subscribers, message)` must send the constructed payload
  through `gateway.deliver(payload)` and return the provider receipt.
- Delivery errors must reach the caller; the function must not report success
  before the provider accepts the payload.
- `preview_delivery(subscribers, message)` is an intentionally side-effect-free
  preview. It formats the same payload but must not contact a gateway.
