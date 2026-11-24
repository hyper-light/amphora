#!/usr/bin/env python3
"""What a real PostgreSQL 19 server answers pgbench's TPC-B statements, message by message (docs/design/postgres.md,
section 11, unit 9; T-PG.9.1, T-PG.9.3). Reads a list of statements (crates/amphora/tests/tpcb/statements.txt or
listener.txt: `session|sql`, `$$..$$` is a quoted literal as PostgreSQL writes it), runs each line on its session over
the wire protocol in order, in a database made for the run, and writes every backend message it answered:
`@ session sql`, then `<type> <hex body>` a line. A list that makes no table is given pgbench's own (pgbench.c,
createTables). usage: postgres-tpcb-answers.py <host> <port> <statements> > answers.txt
"""
import socket
import struct
import sys

DDL = [
    "create table pgbench_history(tid int,bid int,aid int,delta int,mtime timestamp,filler char(22))",
    "create table pgbench_tellers(tid int not null,bid int,tbalance int,filler char(84))",
    "create table pgbench_accounts(aid int not null,bid int,abalance int,filler char(84))",
    "create table pgbench_branches(bid int not null,bbalance int,filler char(88))",
    "alter table pgbench_branches add primary key (bid)",
    "alter table pgbench_tellers add primary key (tid)",
    "alter table pgbench_accounts add primary key (aid)",
]


class Client:
    def __init__(self, host, port, database):
        self.sock = socket.create_connection((host, port))
        body = struct.pack("!i", 196608) + b"user\0postgres\0database\0%s\0\0" % database.encode()
        self.sock.sendall(struct.pack("!i", len(body) + 4) + body)
        while True:
            kind, body = self.read()
            if kind == b"R" and struct.unpack("!i", body[:4])[0] != 0:
                raise SystemExit("the server asked for a password")
            if kind == b"Z":
                return

    def read(self):
        head = self.exact(5)
        length = struct.unpack("!i", head[1:])[0]
        return head[:1], self.exact(length - 4)

    def exact(self, n):
        data = b""
        while len(data) < n:
            more = self.sock.recv(n - len(data))
            if not more:
                raise SystemExit("the server closed the connection")
            data += more
        return data

    def query(self, sql):
        body = sql.encode() + b"\0"
        self.sock.sendall(b"Q" + struct.pack("!i", len(body) + 4) + body)
        messages = []
        while True:
            kind, body = self.read()
            messages.append((kind.decode(), body.hex()))
            if kind == b"Z":
                return messages


DATABASE = "tpcb_answers"
host, port, path = sys.argv[1], int(sys.argv[2]), sys.argv[3]
lines = open(path, encoding="utf-8").read().splitlines()
admin = Client(host, port, "postgres")
admin.query("drop database if exists %s" % DATABASE)
admin.query("create database %s" % DATABASE)
setup = Client(host, port, DATABASE)
if not any("create table" in line for line in lines):
    for statement in DDL:
        setup.query(statement)
sessions = {"s1": Client(host, port, DATABASE), "s2": Client(host, port, DATABASE)}
for line in lines:
    session, sql = line.split("|", 1)
    print("@ %s %s" % (session, sql))
    for kind, body in sessions[session].query(sql):
        print("%s %s" % (kind, body))
