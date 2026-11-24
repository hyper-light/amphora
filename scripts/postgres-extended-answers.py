#!/usr/bin/env python3
"""What a real PostgreSQL 19 server answers the extended protocol's messages, message by message
(docs/design/postgres.md, section 11, unit 10; T-PG.10.1). Reads a list of steps
(crates/amphora/tests/extended/steps.txt), sends each step's messages at once on its session, in a database made for
the run, and writes the bytes it sent and every backend message they were answered with: `@ session step`, then
`> <hex of the bytes sent>`, then `<type> <hex body>` a line.

A step is `session: message ~ message ...`, its messages' parts between `|`:
  P|statement|sql|parameter types' OIDs        B|portal|statement|parameter formats|parameters|result formats
  DS|statement  DP|portal  CS|statement  CP|portal  E|portal|row limit  S  H  Q|sql  R|type|hex body
  d|data, with Python's string escapes        c        f|reason
Lists are separated by commas; a parameter is `n` (NULL), `t:text` or `x:hex`. A step without Sync or Query ends in a
Flush, and is answered once each of its messages is.

usage: postgres-extended-answers.py <host> <port> <steps> > answers.txt
"""
import importlib.util
import os
import struct
import sys

sys.argv, ARGV = [sys.argv[0]], sys.argv


def message(tag, body):
    return tag.encode() + struct.pack("!i", len(body) + 4) + body


def text(value):
    return value.encode() + b"\0"


def formats(codes):
    codes = [int(code) for code in codes.split(",") if code]
    return struct.pack("!H", len(codes)) + b"".join(struct.pack("!h", code) for code in codes)


def parameters(values):
    values = values.split(",") if values else []
    body = struct.pack("!H", len(values))
    for value in values:
        if value == "n":
            body += struct.pack("!i", -1)
            continue
        kind, given = value.split(":", 1)
        data = given.encode() if kind == "t" else bytes.fromhex(given)
        body += struct.pack("!i", len(data)) + data
    return body


def encode(step):
    """A message's bytes, and what ends its answer: a set of message types for each."""
    kind, *parts = step.split("|")
    if kind == "P":
        name, sql, oids = parts
        oids = [int(oid) for oid in oids.split(",") if oid]
        body = text(name) + text(sql) + struct.pack("!H", len(oids)) + b"".join(struct.pack("!I", oid) for oid in oids)
        return message("P", body), ["1"]
    if kind == "B":
        portal, statement, given, values, results = parts
        return message("B", text(portal) + text(statement) + formats(given) + parameters(values) + formats(results)), ["2"]
    if kind in ("DS", "DP"):
        return message("D", kind[1].encode() + text(parts[0])), ["t", "Tn"] if kind == "DS" else ["Tn"]
    if kind in ("CS", "CP"):
        return message("C", kind[1].encode() + text(parts[0])), ["3"]
    if kind == "E":
        return message("E", text(parts[0]) + struct.pack("!i", int(parts[1]))), ["CsI"]
    if kind == "S":
        return message("S", b""), ["Z"]
    if kind == "H":
        return message("H", b""), []
    if kind == "Q":
        return message("Q", text(parts[0])), ["q"]
    if kind == "d":
        return message("d", parts[0].encode().decode("unicode_escape").encode("latin-1")), []
    if kind == "c":
        return message("c", b""), []
    if kind == "f":
        return message("f", text(parts[0])), []
    if kind == "R":
        tag, body = parts
        ends = {"S": ["Z"], "Q": ["q"], "H": [], "P": ["1"], "B": ["2"], "C": ["3"], "D": ["Tn"], "E": ["CsI"]}
        return message(tag, bytes.fromhex(body)), ends[tag]
    raise SystemExit("no such message: %s" % step)


def answers(client, steps):
    sent, ends = b"", []
    for step in steps:
        data, end = encode(step)
        sent += data
        ends += end
    client.sock.sendall(sent)
    messages = []
    while ends:
        kind, body = client.read()
        kind = kind.decode()
        messages.append((kind, body.hex()))
        # Every message from an error to Sync is skipped, the error's own step's or one before it.
        if (kind == "E" and ends[0] not in ("q", "Z")) or (kind == "Z" and ends[0] not in ("q", "Z")):
            while ends and ends[0] != "Z":
                ends.pop(0)
            if kind == "Z":
                ends.pop(0)
        elif kind in ends[0] or (ends[0] == "q" and kind == "Z"):
            ends.pop(0)
    return sent, messages


def main():
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "postgres-wire.py")
    spec = importlib.util.spec_from_file_location("postgres_wire", path)
    wire = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(wire)
    host, port, steps = ARGV[1], int(ARGV[2]), ARGV[3]
    database = "extended_answers"
    admin = wire.Client(host, port, "postgres")
    admin.query("drop database if exists %s" % database)
    admin.query("create database %s" % database)
    sessions = {name: wire.Client(host, port, database) for name in ("s1", "s2")}
    for client in sessions.values():
        client.sock.settimeout(30)
    for line in open(steps, encoding="utf-8").read().splitlines():
        if not line or line.startswith("#"):
            continue
        name, rest = line.split(": ", 1)
        try:
            sent, messages = answers(sessions[name], rest.split(" ~ "))
        except TimeoutError:
            raise SystemExit("no answer: %s" % line)
        print("@ %s %s" % (name, rest))
        print("> %s" % sent.hex())
        for kind, body in messages:
            print("%s %s" % (kind, body))


main()
