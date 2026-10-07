#!/usr/bin/env python3
"""Run: python3 scripts/benchmark-workflow-summary.py

Synthetic, in-memory SQLite warm-query benchmark, not an end-to-end app latency test.
Includes SQLite retrieval and Python row allocation; excludes Rust serde/transport/disk fsync.
Reads the real canonical schema and production SQL. The pre-optimization summary SQL stays
below as an explicit regression baseline. No user database or repository outputs are written.
"""

import json
import pathlib
import re
import sqlite3
import statistics
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
STORAGE = ROOT / "crates/core/src/storage"
REPOSITORY = STORAGE / "workflow_execution_repository"
SCHEMA = (STORAGE / "canonical_schema.sql").read_text()
V69, V70 = SCHEMA.split("-- Durable organization recovery metadata, schema v70.", 1)


def queries(source):
    return re.findall(r'"(SELECT\s.*?|WITH recent .*?)"', source, re.S)


FULL = queries((REPOSITORY / "delivery.rs").read_text().split("pub fn runtime_snapshot(")[1].split("pub fn ")[0])
FINAL = queries((REPOSITORY / "runtime_summary.rs").read_text())
assert len(FULL) == 5 and len(FINAL) == 6, "SQL layout changed; review this benchmark"
INITIAL = list(FINAL)
INITIAL[1] = """SELECT COALESCE(MAX(sequence),0),
 COALESCE(MAX(CASE WHEN kind='members_changed' THEN sequence END),0)
 FROM workflow_mail_events WHERE instance_id=?1"""
INITIAL[2] = """WITH recent AS (
 SELECT sequence FROM workflow_mail_events
 WHERE instance_id=?1 AND sequence>?2 ORDER BY sequence DESC LIMIT 512
 ), preferences AS (
 SELECT MAX(event.sequence) AS sequence FROM workflow_mail_events event
 JOIN workflow_instance_bindings binding
 ON binding.instance_id=event.instance_id AND binding.node_id=event.target_node_id
 WHERE event.instance_id=?1
 AND event.kind IN ('member_model_changed','member_permissions_changed')
 GROUP BY event.target_node_id,event.kind
 ) SELECT sequence,input_id,message_id,source_node_id,target_node_id,kind,created_at
 FROM workflow_mail_events WHERE sequence IN (
 SELECT sequence FROM recent UNION SELECT sequence FROM preferences) ORDER BY sequence"""
INITIAL[3] = """SELECT mail.node_id,COUNT(*) FROM workflow_mail_messages mail
 JOIN workflow_instance_bindings binding ON binding.instance_id=mail.instance_id
 AND binding.node_id=mail.node_id AND binding.conversation_id=mail.recipient_conversation_id
 WHERE mail.instance_id=?1 AND mail.mail_status='pending'
 GROUP BY mail.node_id ORDER BY mail.node_id"""
INITIAL[4] = """SELECT input.conversation_id,MAX(event.sequence)
 FROM workflow_mail_events event JOIN workflow_mail_inputs input
 ON input.input_id=event.input_id AND input.instance_id=event.instance_id
 WHERE event.instance_id=?1 AND input.conversation_id IS NOT NULL
 AND input.delivery_id IS NOT NULL
 GROUP BY input.conversation_id ORDER BY input.conversation_id"""


def fixture(event_count):
    c = sqlite3.connect(":memory:")
    c.executescript(V69)
    c.execute("INSERT INTO workflow_instances(instance_id,template_id,template_revision,name,color,revision,updated_at,last_request_json,definition_json) VALUES('org','template',1,'Benchmark','#123456',1,1,'{}',?)", [json.dumps({"schemaVersion": 1, "id": "org"})])
    c.executemany("INSERT INTO conversations(id,title,created_at,updated_at) VALUES(?,'',1,1)", [(f"c-{i}",) for i in range(32)])
    c.executemany("INSERT INTO workflow_instance_bindings(instance_id,node_id,conversation_id) VALUES('org',?,?)", [(f"n-{i}", f"c-{i}") for i in range(32)])
    payload = json.dumps({"content": "x" * 4096, "messages": [{"id": "mail", "content": "x" * 4096}]})
    input_count = event_count // 4
    c.executemany("INSERT INTO workflow_mail_inputs(input_id,instance_id,execution_version,node_id,conversation_id,input_json,status,delivery_id,created_at,updated_at) VALUES(?,'org','epoch',?,?,?,'applied',?,1,1)", [(f"i-{i}", f"n-{i % 32}", f"c-{i % 32}", payload, f"d-{i}") for i in range(input_count)])
    c.executemany("INSERT INTO workflow_mail_messages(message_id,instance_id,execution_version,node_id,recipient_conversation_id,mail_status,message_json,input_id,created_at) VALUES(?,'org','epoch',?,?,?,'{}',?,1)", [(f"m-{i}", f"n-{i % 32}", f"c-{i % 32}", "pending" if i >= input_count - 32 else "processed", f"i-{i}") for i in range(input_count)])
    c.executemany("INSERT INTO workflow_mail_events(instance_id,input_id,target_node_id,kind,created_at) VALUES('org',?,?,?,1)", [(f"i-{i // 4}", f"n-{(i // 4) % 32}", "members_changed" if i % 1000 == 0 else "member_model_changed" if i % 500 == 1 else "member_permissions_changed" if i % 500 == 2 else ["queued", "applied", "completed", "run_completed"][i % 4]) for i in range(event_count)])
    c.commit()
    return c


def execute(c, sqls):
    result = []
    for sql in sqls:
        args = ("org", 0) if "?2" in sql else ("org",)
        if "json_each(?2)" in sql:
            args = ("org", json.dumps([row[1] for row in result[2] if row[1] is not None]))
        result.append(c.execute(sql, args).fetchall())
    return result


def median_ms(fn):
    fn()
    times = []
    for _ in range(7):
        started = time.perf_counter()
        fn()
        times.append(1000 * (time.perf_counter() - started))
    return round(statistics.median(times), 3)


def writes(c):
    c.executemany("INSERT INTO workflow_mail_inputs(input_id,instance_id,execution_version,node_id,conversation_id,input_json,status,created_at,updated_at) VALUES(?,'org','epoch','n-0','c-0','{}','pending',1,1)", [(f"bind-{i}",) for i in range(100)])
    c.executemany("INSERT INTO workflow_mail_events(instance_id,input_id,kind,created_at) VALUES('org',?,'queued',1)", [(f"bind-{i}",) for i in range(100)])
    c.commit()

    def append():
        c.execute("BEGIN")
        c.executemany("INSERT INTO workflow_mail_events(instance_id,input_id,kind,created_at) VALUES('org',?,'run_completed',1)", [("i-0",)] * 100)
        c.rollback()

    def bind():
        c.execute("BEGIN")
        c.executemany("UPDATE workflow_mail_inputs SET delivery_id=? WHERE input_id=?", [(f"bd-{i}", f"bind-{i}") for i in range(100)])
        c.rollback()

    return {"append_100_events_ms": median_ms(append), "bind_100_inputs_ms": median_ms(bind)}


def main():
    print(f"SQLite {sqlite3.sqlite_version}; memory / warm median of 7; 32 members; 4 events/input; 8 KiB JSON/input")
    print("SQL retrieval only, includes Python row allocation; no Rust serde, transport or disk fsync")
    for event_count in [1_000, 10_000, 100_000]:
        c = fixture(event_count)
        expected = execute(c, INITIAL)
        old_ms = median_ms(lambda: execute(c, FULL))
        initial_ms = median_ms(lambda: execute(c, INITIAL))
        start = time.perf_counter()
        c.executescript("BEGIN;" + V70 + "COMMIT;")
        migration_ms = round(1000 * (time.perf_counter() - start), 3)
        actual = execute(c, FINAL)
        assert actual == expected, "projection semantics changed"
        final_ms = median_ms(lambda: execute(c, FINAL))
        print(json.dumps({"events": event_count, "inputs": event_count // 4, "old_full_sql_ms": old_ms, "initial_summary_sql_ms": initial_ms, "final_summary_sql_ms": final_ms, "v70_indexes_backfill_ms": migration_ms, **writes(c)}))
        if event_count == 100_000:
            for i, sql in enumerate(FINAL):
                args = ("org", 0) if "?2" in sql else ("org",)
                print(f"final SQL {i}: " + " | ".join(row[3] for row in c.execute("EXPLAIN QUERY PLAN " + sql, args)))
        c.close()


if __name__ == "__main__":
    main()
