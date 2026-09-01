from decimal import Decimal

from invoice import CURRENCY_CODE, invoice_total


def test_invoice_total_returns_decimal():
    result = invoice_total([{"unit_price": "9.50", "quantity": 2}])
    assert isinstance(result, Decimal)


def test_currency_code_is_usd():
    assert CURRENCY_CODE == "USD"
