#!/usr/bin/env python3
"""Simulate rule engine logic in Python to verify expected zkVM output.

This mirrors the Rust guest logic exactly, so we can confirm the proof
journal matches the expected results.
"""

def main():
    # ── Test records (same as generate_rule_engine_input) ────────────────
    records = [
        # Normal records (0-4)
        {"id": 0x00, "int_fields": [50, 42, 10], "str_fields": [b"normal_op", b"clean"]},
        {"id": 0x01, "int_fields": [50, 42, 10], "str_fields": [b"normal_op", b"clean"]},
        {"id": 0x02, "int_fields": [50, 42, 10], "str_fields": [b"normal_op", b"clean"]},
        {"id": 0x03, "int_fields": [50, 42, 10], "str_fields": [b"normal_op", b"clean"]},
        {"id": 0x04, "int_fields": [50, 42, 10], "str_fields": [b"normal_op", b"clean"]},
        # Attack records (5-6)
        {"id": 0x05, "int_fields": [200, 42, -5], "str_fields": [b"attack_vector_1", b"suspicious_payload"]},
        {"id": 0x06, "int_fields": [200, 42, -5], "str_fields": [b"attack_vector_1", b"suspicious_payload"]},
        # Borderline records (7-8)
        {"id": 0x07, "int_fields": [101, 99, 0], "str_fields": [b"attack_scan", b"normal"]},
        {"id": 0x08, "int_fields": [101, 99, 0], "str_fields": [b"attack_scan", b"normal"]},
        # Outlier record (9)
        {"id": 0x09, "int_fields": [500, 1, 100], "str_fields": [b"benign", b"suspicious_activity"]},
    ]

    # ── Glob match (same algorithm as Rust guest) ────────────────────────
    def glob_match(pattern, text):
        px, tx = 0, 0
        star_px, star_tx = None, 0
        while tx < len(text):
            if px < len(pattern) and (pattern[px:px+1] == b'?' or pattern[px:px+1] == text[tx:tx+1]):
                px += 1
                tx += 1
            elif px < len(pattern) and pattern[px:px+1] == b'*':
                star_px = px
                star_tx = tx
                px += 1
            elif star_px is not None:
                px = star_px + 1
                star_tx += 1
                tx = star_tx
            else:
                return False
        while px < len(pattern) and pattern[px:px+1] == b'*':
            px += 1
        return px == len(pattern)

    # ── Evaluate conditions ──────────────────────────────────────────────
    def eval_condition(cond, record):
        ctype, fidx, cmp_op, int_val, str_val = cond
        if ctype == 0:  # int_cmp
            val = record["int_fields"][fidx]
            if cmp_op == 0: return val == int_val
            if cmp_op == 1: return val != int_val
            if cmp_op == 2: return val > int_val
            if cmp_op == 3: return val >= int_val
            if cmp_op == 4: return val < int_val
            if cmp_op == 5: return val <= int_val
        elif ctype == 1:  # str_eq
            return record["str_fields"][fidx] == str_val
        elif ctype == 2:  # str_contains
            return str_val in record["str_fields"][fidx]
        elif ctype == 3:  # str_prefix
            return record["str_fields"][fidx].startswith(str_val)
        elif ctype == 4:  # str_suffix
            return record["str_fields"][fidx].endswith(str_val)
        elif ctype == 5:  # str_glob
            return glob_match(str_val, record["str_fields"][fidx])
        return False

    def eval_rule(conditions, combine, record):
        if not conditions:
            return True
        if combine == 0:  # AND
            return all(eval_condition(c, record) for c in conditions)
        else:  # OR
            return any(eval_condition(c, record) for c in conditions)

    # ── Rules ────────────────────────────────────────────────────────────
    rules = [
        {
            "name": "Rule 0: AND(int_field[0] > 100, str_field[0] glob 'attack*')",
            "conditions": [
                (0, 0, 2, 100, b""),       # int_field[0] > 100
                (5, 0, 0, 0, b"attack*"),  # str_field[0] glob "attack*"
            ],
            "combine": 0,
        },
        {
            "name": "Rule 1: OR(int_field[1] == 42, int_field[2] < 0)",
            "conditions": [
                (0, 1, 0, 42, b""),  # int_field[1] == 42
                (0, 2, 4, 0, b""),   # int_field[2] < 0
            ],
            "combine": 1,
        },
        {
            "name": "Rule 2: AND(str_field[1] contains 'suspicious')",
            "conditions": [
                (2, 1, 0, 0, b"suspicious"),  # str_field[1] contains "suspicious"
            ],
            "combine": 0,
        },
    ]

    # ── Evaluate rules ───────────────────────────────────────────────────
    print("=" * 70)
    print("RULE ENGINE VERIFICATION")
    print("=" * 70)
    print(f"\nTotal records: {len(records)}")
    print()

    rule_matches = []
    for ri, rule in enumerate(rules):
        matches = []
        for i, rec in enumerate(records):
            matched = eval_rule(rule["conditions"], rule["combine"], rec)
            matches.append(matched)

        rule_matches.append(matches)
        matched_ids = [i for i, m in enumerate(matches) if m]

        print(f"{rule['name']}")
        print(f"  Matching records: {matched_ids}")
        print(f"  Count: {len(matched_ids)}")
        print()

    # ── Aggregations ─────────────────────────────────────────────────────
    print("-" * 70)
    print("AGGREGATIONS")
    print("-" * 70)
    print()

    # Agg 0: Sum of int_field[0] filtered by rule 0
    filtered = [records[i]["int_fields"][0] for i in range(len(records)) if rule_matches[0][i]]
    print(f"Agg 0: Sum of int_field[0] where Rule 0 matches")
    print(f"  Filtered values: {filtered}")
    print(f"  Sum: {sum(filtered)}")
    print(f"  Count: {len(filtered)}")
    print()

    # Agg 1: Max of int_field[1] over all records
    all_vals = [rec["int_fields"][1] for rec in records]
    print(f"Agg 1: Max of int_field[1] over all records")
    print(f"  All values: {all_vals}")
    print(f"  Max: {max(all_vals)}")
    print(f"  Count: {len(all_vals)}")
    print()

    # ── Summary ──────────────────────────────────────────────────────────
    print("=" * 70)
    print("EXPECTED PROOF OUTPUT")
    print("=" * 70)
    print(f"  total_records:          10")
    print(f"  rule_results[0].count:  {sum(rule_matches[0])}  (records {[i for i,m in enumerate(rule_matches[0]) if m]})")
    print(f"  rule_results[1].count:  {sum(rule_matches[1])}  (records {[i for i,m in enumerate(rule_matches[1]) if m]})")
    print(f"  rule_results[2].count:  {sum(rule_matches[2])}  (records {[i for i,m in enumerate(rule_matches[2]) if m]})")
    print(f"  agg_results[0].value:   {sum(filtered)}  (sum of int_field[0] for rule 0 matches)")
    print(f"  agg_results[0].count:   {len(filtered)}")
    print(f"  agg_results[1].value:   {max(all_vals)}  (max of int_field[1] across all)")
    print(f"  agg_results[1].count:   {len(all_vals)}")

if __name__ == "__main__":
    main()
