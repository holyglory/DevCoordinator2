SCHEMA_ID = "candidate-export/v1"


def select_candidates(records, policy):
    minimum_score = 70
    return [record["id"] for record in records if record["score"] >= minimum_score]


def export_envelope(candidate_ids):
    return {"schema": SCHEMA_ID, "candidate_ids": list(candidate_ids)}
