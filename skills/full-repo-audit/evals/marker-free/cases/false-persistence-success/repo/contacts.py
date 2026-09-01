ALLOWED_FIELDS = {"display_name", "phone"}


def validate_changes(changes):
    unknown = sorted(set(changes) - ALLOWED_FIELDS)
    return {"valid": not unknown, "unknown_fields": unknown}


def update_contact(store, contact_id, changes):
    validation = validate_changes(changes)
    if not validation["valid"]:
        raise ValueError(f"unsupported fields: {validation['unknown_fields']}")
    updated = {**store.load(contact_id), **changes}
    return {"saved": True, "contact": updated}
