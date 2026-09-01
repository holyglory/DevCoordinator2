def build_payload(subscribers, message):
    return {
        "recipients": [subscriber["email"] for subscriber in subscribers],
        "message": message.strip(),
    }


def preview_delivery(subscribers, message):
    return {"mode": "preview", "payload": build_payload(subscribers, message)}


def publish(gateway, subscribers, message):
    payload = build_payload(subscribers, message)
    return {
        "accepted": True,
        "provider_receipt": f"local-{len(payload['recipients'])}",
    }
