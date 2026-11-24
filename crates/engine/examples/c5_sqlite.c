// Lane C5: C2's statements on SQLite 3.54.0 with the same per-row work as crates/engine/examples/c2_user.rs.
// usage: c5_sqlite <fixture.sql> <queries.txt> <db file> <query> <mode: rows|step|count>
//  rows:  every value is made a string (malloc and copy, as Value::to_string does) and hashed with SipHash-1-3, bytes then 0xff,
//         a wrapping sum of the row hashes, so the hash equals the Rust harness's for equal rows in any order
//  step:  sqlite3_step alone; count: SELECT count(*) around the statement
#include <sqlite3.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <sys/resource.h>
static double now(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec+t.tv_nsec*1e-9;}
static inline uint64_t rotl(uint64_t x,int b){return (x<<b)|(x>>(64-b));}
#define RND v0+=v1;v1=rotl(v1,13);v1^=v0;v0=rotl(v0,32);v2+=v3;v3=rotl(v3,16);v3^=v2;v0+=v3;v3=rotl(v3,21);v3^=v0;v2+=v1;v1=rotl(v1,17);v1^=v2;v2=rotl(v2,32);
static uint64_t sip13(const unsigned char*in,size_t n){
  uint64_t v0=0x736f6d6570736575ULL,v1=0x646f72616e646f6dULL,v2=0x6c7967656e657261ULL,v3=0x7465646279746573ULL,m;
  size_t i=0;for(;i+8<=n;i+=8){memcpy(&m,in+i,8);v3^=m;RND;v0^=m;}
  uint64_t b=((uint64_t)n)<<56;for(size_t k=0;i+k<n;k++)b|=((uint64_t)in[i+k])<<(8*k);
  v3^=b;RND;v0^=b;v2^=0xff;RND;RND;RND;return v0^v1^v2^v3;}
int main(int argc,char**argv){
  if(argc<6){fprintf(stderr,"usage\n");return 2;}
  FILE*f=fopen(argv[1],"r");char*line=0;size_t cap=0;
  sqlite3*db;remove(argv[3]);if(sqlite3_open(argv[3],&db)){fprintf(stderr,"%s\n",sqlite3_errmsg(db));return 1;}
  sqlite3_exec(db,"BEGIN",0,0,0);
  while(getline(&line,&cap,f)>0){char*e=0;if(sqlite3_exec(db,line,0,0,&e)){fprintf(stderr,"fixture: %s\n",e);return 1;}}
  fclose(f);sqlite3_exec(db,"COMMIT",0,0,0);
  f=fopen(argv[2],"r");int want=atoi(argv[4]);char sql[4096]={0};
  while(getline(&line,&cap,f)>0){if(atoi(line)==want){char*p=strchr(line,'|');p=strchr(p+1,'|');strncpy(sql,p+1,sizeof sql-1);sql[strcspn(sql,"\n")]=0;break;}}
  fclose(f);
  char*c=strstr(sql," -- ");if(c)*c=0;
  char q[4400];if(!strcmp(argv[5],"count"))snprintf(q,sizeof q,"SELECT count(*) FROM (%s)",sql);else snprintf(q,sizeof q,"%s",sql);
  double t0=now();sqlite3_stmt*s;if(sqlite3_prepare_v2(db,q,-1,&s,0)){fprintf(stderr,"prepare: %s\n",sqlite3_errmsg(db));return 1;}
  double t1=now();uint64_t rows=0,sum=0;int rc;int cols=sqlite3_column_count(s);unsigned char buf[8192];
  int hash=!strcmp(argv[5],"rows");
  while((rc=sqlite3_step(s))==SQLITE_ROW){rows++;
    if(hash){size_t n=0;for(int k=0;k<cols;k++){const unsigned char*t=sqlite3_column_text(s,k);int len=sqlite3_column_bytes(s,k);
      char*copy=malloc((size_t)len+1);if(len)memcpy(copy,t,(size_t)len);copy[len]=0;
      memcpy(buf+n,copy,(size_t)len);n+=(size_t)len;buf[n++]=0xff;free(copy);}
      sum+=sip13(buf,n);}
    else if(!strcmp(argv[5],"count"))sum=sqlite3_column_int64(s,0);}
  double t2=now();struct rusage ru;getrusage(RUSAGE_SELF,&ru);
  printf("sqlite %s query %d mode %s: %llu rows %s %016llx prepare %.1f ms run %.1f ms user %.2f s sys %.2f s maxrss %ld MB rc %d%s\n",sqlite3_libversion(),want,argv[5],(unsigned long long)rows,
    !strcmp(argv[5],"count")?"count":"hash",(unsigned long long)sum,(t1-t0)*1e3,(t2-t1)*1e3,ru.ru_utime.tv_sec+ru.ru_utime.tv_usec*1e-6,ru.ru_stime.tv_sec+ru.ru_stime.tv_usec*1e-6,ru.ru_maxrss/1024,rc,rc==SQLITE_DONE?"":" NOT DONE");
  sqlite3_finalize(s);sqlite3_close(db);remove(argv[3]);return rc==SQLITE_DONE?0:1;}
