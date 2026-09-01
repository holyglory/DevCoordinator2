from decimal import Decimal


CURRENCY_CODE = "USD"


def invoice_total(lines):
    total = Decimal("0")
    for line in lines:
        quantity = line["quantity"]
        if quantity <= 0:
            raise ValueError("quantity must be positive")
        total += Decimal(line["unit_price"]) * quantity
    return total
