#include <stdio.h>
#include <stdlib.h>
#include <sqlite3.h>
#include <unistd.h>
static int row(void *p,int n,char **v,char **names) {
  (void)p;
  for(int i=0;i<n;i++) printf("%s=%s ", names[i], v[i]?v[i]:"NULL");
  puts(""); return 0;
}
static void sql(sqlite3 *db,const char *q) {
  char *error=0;
  printf("SQL: %s\n", q);
  int rc=sqlite3_exec(db,q,row,0,&error);
  printf("rc=%d error=%s\n", rc,error?error:"");
  sqlite3_free(error);
  if(rc!=SQLITE_OK) exit(2);
}
int main(int argc,char **argv) {
  sqlite3 *reader=0,*writer=0,*fresh=0;
  if(argc!=2 || access(argv[1], F_OK)==0) return 3;
  printf("SQLite version=%s\n", sqlite3_libversion());
  if(sqlite3_open(argv[1],&writer)!=SQLITE_OK) return 4;
  sql(writer,"PRAGMA journal_mode=WAL; CREATE TABLE messages(content TEXT); CREATE VIRTUAL TABLE evidence USING fts5(content,tokenize='trigram'); CREATE TRIGGER message_insert AFTER INSERT ON messages BEGIN INSERT INTO evidence(rowid,content) VALUES(NEW.rowid,NEW.content); END; CREATE TRIGGER message_update AFTER UPDATE ON messages BEGIN DELETE FROM evidence WHERE rowid=OLD.rowid; INSERT INTO evidence(rowid,content) VALUES(NEW.rowid,NEW.content); END; INSERT INTO messages VALUES('confirmed checkpoint');");
  if(sqlite3_open_v2(argv[1],&reader,SQLITE_OPEN_READONLY,0)!=SQLITE_OK) return 5;
  sql(reader,"PRAGMA quick_check");
  sql(writer,"UPDATE messages SET content='committed terminal'");
  sql(reader,"SELECT content FROM messages");
  sql(reader,"PRAGMA quick_check");
  if(sqlite3_open_v2(argv[1],&fresh,SQLITE_OPEN_READONLY,0)!=SQLITE_OK) return 6;
  sql(fresh,"PRAGMA quick_check");
  sqlite3_close(fresh);
  sql(reader,"SELECT content FROM evidence WHERE evidence MATCH 'committed'");
  sql(reader,"PRAGMA quick_check");
  sqlite3_close(reader);
  sqlite3_close(writer);
  return 0;
}
