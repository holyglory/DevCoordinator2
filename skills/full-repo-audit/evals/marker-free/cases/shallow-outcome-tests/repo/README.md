# Invoice arithmetic

## Requirements

- `invoice_total(lines)` must multiply each line's decimal unit price by its
  positive integer quantity and return their exact decimal sum.
- Non-positive quantities must raise `ValueError`.
- Tests must prove representative totals, multiple-line accumulation, and the
  invalid-quantity behavior; checking only the result type is insufficient.
- `test_currency_code_is_usd` protects an intentionally fixed wire-contract
  identifier. That narrow constant assertion is useful and should remain.
