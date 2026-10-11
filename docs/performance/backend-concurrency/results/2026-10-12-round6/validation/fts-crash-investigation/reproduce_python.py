"""Compare the host Python SQLite against minimal.c, using only a temporary fixture."""
import pathlib
import sqlite3
import tempfile


def sql(connection, query):
    print("SQL: " + query)
    print(connection.execute(query).fetchall())


with tempfile.TemporaryDirectory() as directory:
    path = pathlib.Path(directory) / "minimal.sqlite"
    print("SQLite version=" + sqlite3.sqlite_version)
    writer = sqlite3.connect(path, isolation_level=None)
    for query in [
        "PRAGMA journal_mode=WAL",
        "CREATE TABLE messages(content TEXT)",
        "CREATE VIRTUAL TABLE evidence USING fts5(content,tokenize='trigram')",
        "CREATE TRIGGER message_insert AFTER INSERT ON messages BEGIN "
        "INSERT INTO evidence(rowid,content) VALUES(NEW.rowid,NEW.content); END",
        "CREATE TRIGGER message_update AFTER UPDATE ON messages BEGIN "
        "DELETE FROM evidence WHERE rowid=OLD.rowid; "
        "INSERT INTO evidence(rowid,content) VALUES(NEW.rowid,NEW.content); END",
        "INSERT INTO messages VALUES('confirmed checkpoint')",
    ]:
        sql(writer, query)
    reader = sqlite3.connect("file:" + str(path) + "?mode=ro", uri=True, isolation_level=None)
    sql(reader, "PRAGMA quick_check")
    sql(writer, "UPDATE messages SET content='committed terminal'")
    sql(reader, "SELECT content FROM messages")
    sql(reader, "PRAGMA quick_check")
    fresh = sqlite3.connect("file:" + str(path) + "?mode=ro", uri=True, isolation_level=None)
    sql(fresh, "PRAGMA quick_check")
    fresh.close()
    sql(reader, "SELECT content FROM evidence WHERE evidence MATCH 'committed'")
    sql(reader, "PRAGMA quick_check")
    reader.close()
    writer.close()
