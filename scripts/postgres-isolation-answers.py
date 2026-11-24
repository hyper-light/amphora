#!/usr/bin/env python3
"""What a real PostgreSQL 19 server answers several sessions' interleaved statements, message by message
(docs/design/postgres.md, section 8; T-PG.14.1, T-PG.15.1). Reads a list of steps
(crates/amphora/tests/isolation/steps.txt and recheck-steps.txt), each on one line, in order:

    # name           an interleaving's name
    s1|sql           session s1 runs sql, and its answer is read
    s2>sql           session s2 sends sql, which must wait for another session's transaction: the
                     next step runs once pg_stat_activity shows it waiting on a lock
    s2<              session s2's outstanding statement's answer is read
    s2+              session s2's outstanding statement still waits
    s2!sql           session s2 sends sql, whose wait its deadlock check refuses: its answer is read
    s2!              session s2's outstanding statement's, which its deadlock check refuses
    s2~sql           session s2 sends sql, whose wait lock_timeout refuses: its answer is read

and writes `@ session sql` (`@ s2> sql`, and `@ s2< -` for one with no sql), then `<type> <hex body>` a
message the server answered. It asks for no password. The sessions are those the steps open in a
database made for the run.

usage: postgres-isolation-answers.py <host> <port> <steps> > answers.txt
"""
import socket
import struct
import sys
import time

DATABASE = "isolation_answers"


class Client:
    def __init__(self, host, port, database):
        self.sock = socket.create_connection((host, port))
        self.pid = None
        body = struct.pack("!i", 196608) + b"user\0postgres\0database\0%s\0\0" % database.encode()
        self.sock.sendall(struct.pack("!i", len(body) + 4) + body)
        while True:
            kind, body = self.read()
            if kind == b"R" and struct.unpack("!i", body[:4])[0] != 0:
                raise SystemExit("the server asked for a password")
            if kind == b"K":
                self.pid = struct.unpack("!i", body[:4])[0]
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

    def send(self, sql):
        body = sql.encode() + b"\0"
        self.sock.sendall(b"Q" + struct.pack("!i", len(body) + 4) + body)

    def answer(self):
        messages = []
        while True:
            kind, body = self.read()
            messages.append((kind.decode(), body.hex()))
            if kind == b"Z":
                return messages

    def query(self, sql):
        self.send(sql)
        return self.answer()


def waits(admin, pid):
    found = admin.query(
        "select count(*) from pg_stat_activity where pid = %d and wait_event_type = 'Lock'" % pid
    )
    return any(kind == "D" and bytes.fromhex(body)[-1:] == b"1" for kind, body in found)


host, port, path = sys.argv[1], int(sys.argv[2]), sys.argv[3]
admin = Client(host, port, "postgres")
admin.query("drop database if exists %s" % DATABASE)
admin.query("create database %s" % DATABASE)
sessions = {}
for line in open(path, encoding="utf-8").read().splitlines():
    if line.startswith("#") or not line:
        continue
    name = line[:2]
    rest = line[2:]
    if name not in sessions:
        sessions[name] = Client(host, port, DATABASE)
    client = sessions[name]
    if rest.startswith("|"):
        sql = rest[1:]
        print("@ %s %s" % (name, sql))
        messages = client.query(sql)
    elif rest.startswith(">"):
        sql = rest[1:]
        print("@ %s> %s" % (name, sql))
        client.send(sql)
        while not waits(admin, client.pid):
            time.sleep(0.01)
        messages = []
    elif rest == "<":
        print("@ %s< -" % name)
        messages = client.answer()
    elif rest == "+":
        print("@ %s+ -" % name)
        time.sleep(0.2)
        while not waits(admin, client.pid):
            time.sleep(0.01)
        messages = []
    elif rest[:1] in "!~" and rest[1:]:
        print("@ %s%s %s" % (name, rest[0], rest[1:]))
        messages = client.query(rest[1:])
    elif rest == "!":
        print("@ %s! -" % name)
        messages = client.answer()
    else:
        raise SystemExit("not a step: %s" % line)
    for kind, body in messages:
        print("%s %s" % (kind, body))
