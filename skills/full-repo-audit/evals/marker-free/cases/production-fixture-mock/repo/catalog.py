ONBOARDING_PRODUCTS = (
    {"sku": "tour-notebook", "name": "Notebook"},
    {"sku": "tour-pencil", "name": "Pencil"},
)


def onboarding_preview():
    return [dict(product) for product in ONBOARDING_PRODUCTS]


class CatalogService:
    def __init__(self, repository):
        self.repository = repository

    def list_products(self):
        return [dict(product) for product in ONBOARDING_PRODUCTS]
